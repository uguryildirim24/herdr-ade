//! The ticker: one background loop per projects root.
//!
//! Everything it does is "check on an interval, compare with last time, act".
//! It exits on request through a stop file, never through signals.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::coordinator;
use crate::herdr::{Agent, Herdr, Pane};
use crate::paths::Ctx;
use crate::project::{self, Project, Status};
use crate::steps::{self, Memory};
use crate::{inbox, thread, threads};

const TICK: Duration = crate::pi::doctor::READINESS_CACHE_TTL;
const STOP_WAIT: Duration = Duration::from_secs(60);
// A replacement never leaves a second ticker waiting behind a blocked pass.
const REPLACE_WAIT: Duration = Duration::from_millis(500);
// Bound each in-flight step, not the whole pass. The pre-progress ticker
// took 123 seconds to finish the pass in the observed failed install; leave
// enough room for that first replacement too.
const INSTALL_REPLACE_WAIT: Duration = Duration::from_secs(180);
const LOG_CAP: u64 = 1_000_000;

pub(crate) fn lock_path(root: &Path) -> PathBuf {
    root.join(".ticker.lock")
}

fn stop_path(root: &Path) -> PathBuf {
    root.join(".ticker.stop")
}

fn log_path(root: &Path) -> PathBuf {
    root.join(".ticker.log")
}

fn wake_path(root: &Path) -> PathBuf {
    root.join(".ticker.wake")
}

fn progress_path(root: &Path) -> PathBuf {
    root.join(".ticker.progress")
}

fn recovery_path(root: &Path) -> PathBuf {
    root.join(".ticker.recovery")
}

fn clean_exit_path(root: &Path) -> PathBuf {
    root.join(".ticker.clean-exit")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct Progress {
    pid: u32,
    started: String,
    sequence: u64,
    step: String,
    #[serde(default)]
    updated: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Health {
    observed: String,
    projects: usize,
    failures: Vec<String>,
    #[serde(default)]
    status: Vec<String>,
    // Partial passes publish gaps immediately but only a completed pass logs
    // the state transition. Keep the last logged set across stops/restarts.
    #[serde(default)]
    logged: Option<(Vec<String>, Vec<String>)>,
}

thread_local! {
    // Validation keeps only stamps, not a second copy of lane history. Failed
    // snapshots are retried, including repairs which do not change the directory.
    static OBSERVATION_RECORDS: std::cell::RefCell<crate::record_cache::Records<()>> =
        std::cell::RefCell::new(crate::record_cache::Records::default());
}

fn health_path(root: &Path) -> PathBuf {
    root.join(".ticker.health")
}

impl Health {
    fn begin(root: &Path, projects: usize) -> Self {
        let previous: Self = read_evidence(&health_path(root)).unwrap_or_default();
        Self {
            observed: project::now(),
            projects,
            logged: previous
                .logged
                .or(Some((previous.failures, previous.status))),
            ..Self::default()
        }
    }

    fn closed(&mut self, root: &Path, log: &Log, project: &Project) {
        let detail = format!(
            "{}: coordinator closed; reviews skipped until open",
            project.dir().display()
        );
        if !self.status.contains(&detail) {
            self.status.push(detail);
        }
        self.publish_partial(root, log);
    }

    fn failure(&mut self, root: &Path, log: &Log, detail: String) {
        if !self.failures.contains(&detail) {
            self.failures.push(detail);
        }
        self.publish_partial(root, log);
    }

    fn publish_partial(&self, root: &Path, log: &Log) {
        // Publish immediately: a later blocked step must not hide this failure.
        // Only a completed pass can clear prior failures; ensure and partial
        // passes have not reobserved the records which produced them.
        let mut evidence = read_evidence::<Health>(&health_path(root)).unwrap_or_default();
        evidence.observed = self.observed.clone();
        evidence.projects = self.projects;
        evidence.logged = self.logged.clone();
        for failure in &self.failures {
            if !evidence.failures.contains(failure) {
                evidence.failures.push(failure.clone());
            }
        }
        for status in &self.status {
            if !evidence.status.contains(status) {
                evidence.status.push(status.clone());
            }
        }
        evidence.write(root, log);
    }

    fn log_changes(&self, log: &Log) {
        let before: BTreeSet<_> = self
            .logged
            .iter()
            .flat_map(|(failures, status)| failures.iter().chain(status))
            .collect();
        let current: BTreeSet<_> = self.failures.iter().chain(&self.status).collect();
        let mut changes: Vec<_> = current.difference(&before).map(|s| (*s).clone()).collect();
        changes.extend(before.difference(&current).map(|s| format!("cleared: {s}")));
        if !changes.is_empty() {
            log.line(&format!(
                "observation state changed: {}",
                changes.join("; ")
            ));
        }
    }

    fn publish(&self, root: &Path, log: &Log) {
        self.log_changes(log);
        let mut evidence = self.clone();
        evidence.logged = Some((self.failures.clone(), self.status.clone()));
        evidence.write(root, log);
    }

    fn finish_partial(root: &Path, log: &Log) {
        if let Ok(evidence) = read_evidence::<Health>(&health_path(root)) {
            evidence.publish(root, log);
        }
    }

    fn write(&self, root: &Path, log: &Log) {
        if let Err(error) = project::write_json(&health_path(root), self) {
            log.line(&format!("could not publish root health: {error:#}"));
        }
    }
}

fn read_evidence<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} does not parse", path.display()))
}

/// A held lock is process ownership, not proof of progress. Old progress
/// records still load, but without a timestamp their responsiveness is unknown.
fn progress_health(root: &Path, info: &Info) -> (Option<bool>, String) {
    let progress = match read_evidence::<Progress>(&progress_path(root)) {
        Ok(progress) if progress.pid == info.pid && progress.started == info.started => progress,
        Ok(_) => {
            return (
                None,
                "responsiveness unknown: progress belongs to another run".into(),
            );
        }
        Err(error) => return (None, format!("responsiveness unknown: {error:#}")),
    };
    let detail = format!("sequence {}, step {}", progress.sequence, progress.step);
    let Ok(updated) = progress.updated.parse::<jiff::Timestamp>() else {
        return (
            None,
            format!("responsiveness unknown: {detail}; no progress timestamp"),
        );
    };
    let age = jiff::Timestamp::now()
        .as_second()
        .saturating_sub(updated.as_second());
    if age > INSTALL_REPLACE_WAIT.as_secs() as i64 {
        (
            Some(false),
            format!("stalled: {detail}; no progress for {age}s; lock still held"),
        )
    } else if age < 0 {
        (
            None,
            format!("responsiveness unknown: {detail}; progress timestamp is in the future"),
        )
    } else {
        (
            Some(true),
            format!("recent progress: {detail}; observed {age}s ago"),
        )
    }
}

pub(crate) fn health_report(root: &Path) -> (Option<bool>, String) {
    let (mut healthy, mut detail) = match lock_state(root) {
        LockState::Free => (None, "not running (lock released)".into()),
        LockState::Unknown(error) => (Some(false), format!("ownership unknown: {error}")),
        LockState::Held(info) => progress_health(root, &info),
    };
    match read_evidence::<Health>(&health_path(root)) {
        Ok(health) => {
            detail.push_str(&format!(
                "; last observation {}: {} project(s)",
                health.observed, health.projects
            ));
            if !health.status.is_empty() {
                detail.push_str(&format!("; {}", health.status.join("; ")));
            }
            if !health.failures.is_empty() {
                healthy = Some(false);
                detail.push_str(&format!(
                    "; missing observations: {}",
                    health.failures.join("; ")
                ));
            }
        }
        Err(error) => {
            if healthy == Some(true) {
                healthy = None;
            }
            detail.push_str(&format!("; observations unknown: {error:#}"));
        }
    }
    (healthy, detail)
}

fn current_progress(root: &Path, info: &Info) -> Option<Progress> {
    let progress: Progress = project::read_json(&progress_path(root))?;
    (progress.pid == info.pid && progress.started == info.started).then_some(progress)
}

fn poll_request_path(project: &Project) -> PathBuf {
    project.state_dir().join("poll-now.json")
}

fn poll_requests(project: &Project) -> std::collections::BTreeSet<String> {
    project::read_json(&poll_request_path(project)).unwrap_or_default()
}

/// Wakes the existing ticker and marks this machine due in its normal courier
/// pass. Thread commands never open their own SSH polling path.
pub(crate) fn request_remote_poll(root: &Path, project: &Project, machine: &str) -> Result<()> {
    {
        let _lock = project.lock()?;
        let mut requests = poll_requests(project);
        requests.insert(machine.to_string());
        project::write_json(&poll_request_path(project), &requests)?;
    }
    project::write_atomic(&wake_path(root), b"poll\n")
}

fn clear_poll_request(project: &Project, machine: &str) {
    let Ok(_lock) = project.lock() else {
        return;
    };
    let mut requests = poll_requests(project);
    requests.remove(machine);
    if requests.is_empty() {
        let _ = std::fs::remove_file(poll_request_path(project));
    } else {
        let _ = project::write_json(&poll_request_path(project), &requests);
    }
}

/// What the lock holder writes into the lock file, for `ticker status` and
/// `doctor`. The pid is for display only; nothing signals it.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub(crate) struct Info {
    pub(crate) version: String,
    pub(crate) pid: u32,
    pub(crate) root: String,
    /// Stable working directory inherited by the loop itself. Child commands
    /// are also given this directory explicitly.
    pub(crate) cwd: String,
    pub(crate) started: String,
    /// Where the ticker resolves its tools from its own environment, which may
    /// differ from the user's shell.
    pub(crate) tools: Vec<(String, String)>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum LockState {
    Free,
    Held(Info),
    Unknown(String),
}

/// Probes the lock without keeping it. The file is never created here.
pub(crate) fn lock_state(root: &Path) -> LockState {
    let path = lock_path(root);
    let mut file = match File::options().read(true).write(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return LockState::Free,
        Err(error) => return LockState::Unknown(format!("{}: {error}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => LockState::Free,
        Err(std::fs::TryLockError::Error(error)) => {
            LockState::Unknown(format!("{}: {error}", path.display()))
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            let mut text = String::new();
            let _ = file.read_to_string(&mut text);
            LockState::Held(serde_json::from_str(&text).unwrap_or_default())
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum StartAction {
    Spawn,
    Nothing,
    StopThenSpawn,
}

#[derive(Debug, PartialEq)]
enum StopOutcome {
    Stopped,
    Pending,
}

/// A healthy ticker of the same version is never replaced. A different
/// version must release the lock before a new ticker may start.
fn decide_start(lock: &LockState, my_version: &str) -> StartAction {
    match lock {
        LockState::Free => StartAction::Spawn,
        LockState::Held(info) if crate::build::same_commit(&info.version, my_version) => {
            StartAction::Nothing
        }
        LockState::Held(_) => StartAction::StopThenSpawn,
        LockState::Unknown(_) => StartAction::Nothing,
    }
}

/// Ensures a ticker is running without waiting for a running one to stop.
/// `review` must not block while replacing a ticker: the ticker's own
/// pass calls `advance`, so waiting here would deadlock against the ticker
/// waiting on `advance`'s lock. Thread starts only ensure; explicit `ticker
/// start` and installation replace a stale-version ticker when it can stop.
pub(crate) fn ensure(ctx: &Ctx) -> Result<()> {
    let root = &ctx.root;
    if !ctx.detached_ticker || install_in_progress(ctx) {
        return Ok(());
    }
    let (slugs, errors) = project::list_slugs_with_errors(root);
    if !errors.is_empty() {
        let log = Log {
            path: log_path(root),
        };
        let mut health = Health::begin(root, slugs.len());
        for error in errors {
            health.failure(root, &log, format!("{error:#}"));
        }
        Health::finish_partial(root, &log);
        // Do not turn failed discovery into an empty-root success. Even with
        // no readable projects, start the normal loop so it can retry.
    } else if slugs.is_empty() && lock_state(root) == LockState::Free {
        return Ok(());
    }
    ensure_free(
        root,
        std::env::var_os("HERDR_ADE_TICKER_SUPERVISOR").is_some(),
        spawn,
    )?;
    let (healthy, detail) = health_report(root);
    if healthy == Some(false) {
        // Degraded discovery must stay visible without blocking healthy
        // projects' commands or declaring the process dead.
        eprintln!("ticker health: {detail}");
    }
    Ok(())
}

fn ensure_free(
    root: &Path,
    supervised: bool,
    spawn_ticker: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    match lock_state(root) {
        LockState::Unknown(error) => bail!("ticker ownership unknown: {error}"),
        LockState::Held(info) => {
            let (healthy, detail) = progress_health(root, &info);
            if healthy != Some(true) {
                // Lane starts and retries call ensure too. Report missing
                // progress without turning a held lock into a work refusal.
                eprintln!("ticker ensure: {detail}; keeping the existing lock holder");
            }
            return Ok(());
        }
        LockState::Free => {}
    }
    {
        // Only the launchd interval records an unattended outage. The lock,
        // not the interval or this snapshot, still decides who runs the loop.
        if supervised {
            if !clean_exit_path(root).exists() {
                project::write_atomic(&recovery_path(root), b"")?;
            }
            let _ = std::fs::remove_file(clean_exit_path(root));
        }
        let _ = std::fs::remove_file(stop_path(root));
        spawn_ticker(root)?;
    }
    Ok(())
}

/// Starts the detached loop, including before the first project is created.
/// A successful spawn requires the child to publish its running lock.
pub(crate) fn start(ctx: &Ctx) -> Result<()> {
    if std::env::var_os("HERDR_ADE_INSTALL_TICKER").is_some() {
        return start_for_install(ctx);
    }
    // The installer owns replacement while holding its lock. Never compete
    // with it, or claim a running loop when its old holder has already left.
    if install_in_progress(ctx) {
        if !matches!(lock_state(&ctx.root), LockState::Held(info) if info.pid != 0 && !info.version.is_empty())
        {
            bail!(
                "ticker start deferred: installation in progress and no running ticker is confirmed"
            );
        }
        return Ok(());
    }
    if start_inner(ctx)? {
        bail!(
            "ticker replacement pending: old ticker still holds the lock; lock state: {:?}",
            lock_state(&ctx.root)
        );
    }
    Ok(())
}

/// The installer owns the install lock, so it is the only caller allowed to
/// replace a ticker while installing. A stalled step leaves its stop request
/// active, so the next start can launch the installed build.
pub(crate) fn start_for_install(ctx: &Ctx) -> Result<()> {
    start_for_install_with_wait(ctx, INSTALL_REPLACE_WAIT)
}

fn start_for_install_with_wait(ctx: &Ctx, wait: Duration) -> Result<()> {
    if start_inner_with_wait(ctx, wait, true)? {
        let step = match lock_state(&ctx.root) {
            LockState::Held(info) => current_progress(&ctx.root, &info)
                .map(|progress| progress.step)
                .unwrap_or_else(|| "unknown (ticker has no progress record)".into()),
            LockState::Free => "unknown (lock released)".into(),
            LockState::Unknown(error) => error,
        };
        bail!(
            "ticker replacement timed out: stalled after {} seconds without progress in step {step}; stop request remains active; lock state: {:?}",
            wait.as_secs(),
            lock_state(&ctx.root)
        );
    }
    Ok(())
}

fn install_in_progress(ctx: &Ctx) -> bool {
    crate::harness::install_in_progress(&ctx.config_dir)
}

fn start_inner(ctx: &Ctx) -> Result<bool> {
    start_inner_with_wait(ctx, REPLACE_WAIT, false)
}

fn start_inner_with_wait(ctx: &Ctx, wait: Duration, install: bool) -> Result<bool> {
    let root = &ctx.root;
    if !ctx.detached_ticker {
        return Ok(false);
    }
    std::fs::create_dir_all(root)?;
    let mut state = lock_state(root);
    // Initialization has acquired the lock but has not published the image.
    // Wait for evidence, without stopping this concurrent starter as stale.
    let deadline = Instant::now() + Duration::from_secs(5);
    while matches!(&state, LockState::Held(info) if info.pid == 0 || info.version.is_empty()) {
        if Instant::now() >= deadline {
            bail!("ticker startup not confirmed: lock holder has not published its running image");
        }
        std::thread::sleep(Duration::from_millis(25));
        state = lock_state(root);
    }
    if let LockState::Unknown(error) = &state {
        bail!("ticker ownership unknown: {error}");
    }
    match decide_start(&state, crate::VERSION) {
        StartAction::Nothing => {
            // A concurrent starter may have won just after the old holder's
            // stop request. Keep the new holder alive.
            let _ = std::fs::remove_file(stop_path(root));
            Ok(false)
        }
        StartAction::Spawn => {
            // A leftover stop file would make the new ticker exit at once.
            let _ = std::fs::remove_file(stop_path(root));
            spawn(root)?;
            Ok(false)
        }
        StartAction::StopThenSpawn => replace(root, wait, install),
    }
}

unsafe extern "C" {
    fn setsid() -> i32;
}

/// `ticker run`, detached: null stdio and a new session, so it does not die
/// with the process group of whatever started it (an agent's shell tool).
fn spawn_command(binary: &Path, root: &Path) -> Command {
    let mut command = Command::new(binary);
    command.arg("--root").arg(root).args(["ticker", "run"]);
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Pane variables belong to whoever started us, not to the ticker: every
    // project carries its own recorded socket.
    for key in [
        "HERDR_SOCKET_PATH",
        "HERDR_SESSION",
        "HERDR_PANE_ID",
        "HERDR_TAB_ID",
        "HERDR_WORKSPACE_ID",
        "HERDR_ADE_TICKER_SUPERVISOR",
    ] {
        command.env_remove(key);
    }
    command
}

fn detached_command(root: &Path) -> Result<Command> {
    use std::os::unix::process::CommandExt;
    let binary = std::env::current_exe().context("could not find this binary's own path")?;
    let mut command = spawn_command(&binary, root);
    // SAFETY: setsid is async-signal-safe and touches no memory.
    unsafe {
        command.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    Ok(command)
}

fn spawn(root: &Path) -> Result<()> {
    let mut command = detached_command(root)?;
    // Keep child initialization errors instead of discarding them with stdio.
    command.stderr(
        File::options()
            .create(true)
            .append(true)
            .open(log_path(root))?,
    );
    spawn_and_confirm(command, root)
}

fn spawn_and_confirm(mut command: Command, root: &Path) -> Result<()> {
    let mut child = command.spawn().context("could not start the ticker")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if matches!(lock_state(root), LockState::Held(info) if info.pid != 0 && crate::build::same_commit(&info.version, crate::VERSION))
        {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            bail!(
                "ticker startup failed: child exited {status} without a running ticker; see {}",
                log_path(root).display()
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "ticker startup not confirmed: no running lock within 5 seconds; see {}",
                log_path(root).display()
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Ask the old ticker to leave, but never start a contender behind it. If an
/// install times out, keep the request so the old ticker exits at its next
/// boundary; a later start can then launch the installed build.
fn replace(root: &Path, wait: Duration, install: bool) -> Result<bool> {
    match request_stop_with_progress(root, wait, install)? {
        StopOutcome::Stopped => {
            spawn(root)?;
            Ok(false)
        }
        StopOutcome::Pending => {
            if matches!(lock_state(root), LockState::Held(info) if crate::build::same_commit(&info.version, crate::VERSION))
            {
                let _ = std::fs::remove_file(stop_path(root));
                Ok(false)
            } else {
                // Ordinary starts must not leave the root unwatched. An
                // installer instead leaves the request in place on timeout:
                // after the old pass exits, a subsequent start can take over.
                if !install {
                    let _ = std::fs::remove_file(stop_path(root));
                }
                Ok(true)
            }
        }
    }
}

/// Leaves a durable stop request and waits up to `wait` for its holder.
fn request_stop(root: &Path, wait: Duration) -> Result<StopOutcome> {
    request_stop_with_progress(root, wait, false)
}

fn request_stop_with_progress(
    root: &Path,
    wait: Duration,
    track_progress: bool,
) -> Result<StopOutcome> {
    let start = Instant::now();
    request_stop_with_progress_on(
        root,
        wait,
        track_progress,
        || start.elapsed(),
        std::thread::sleep,
        lock_state,
    )
}

// The clock and poll boundary are supplied so progress/deadline interactions
// can be exercised without racing the scheduler or sleeping in tests.
fn request_stop_with_progress_on(
    root: &Path,
    wait: Duration,
    track_progress: bool,
    now: impl Fn() -> Duration,
    mut pause: impl FnMut(Duration),
    mut state: impl FnMut(&Path) -> LockState,
) -> Result<StopOutcome> {
    if state(root) == LockState::Free {
        let _ = std::fs::remove_file(stop_path(root));
        return Ok(StopOutcome::Stopped);
    }
    std::fs::write(stop_path(root), b"")?;
    let mut deadline = now() + wait;
    let mut observed: Option<Progress> = None;
    loop {
        match state(root) {
            LockState::Free => {
                let _ = std::fs::remove_file(stop_path(root));
                return Ok(StopOutcome::Stopped);
            }
            LockState::Held(info) if track_progress => {
                if let Some(progress) = current_progress(root, &info) {
                    if observed.as_ref().is_some_and(|old| {
                        old.pid == progress.pid
                            && old.started == progress.started
                            && old.sequence < progress.sequence
                    }) {
                        deadline = now() + wait;
                    }
                    observed = Some(progress);
                }
            }
            LockState::Held(_) => {}
            LockState::Unknown(error) => bail!("ticker ownership unknown: {error}"),
        }
        if now() >= deadline {
            break;
        }
        pause(Duration::from_millis(25));
    }
    Ok(StopOutcome::Pending)
}

/// Asks the running ticker to exit and waits for the lock to be released.
pub(crate) fn stop(root: &Path) -> Result<()> {
    match request_stop(root, STOP_WAIT)? {
        StopOutcome::Stopped => Ok(()),
        StopOutcome::Pending => bail!(
            "ticker stop pending: the current step exceeded {} seconds; the stop request remains active",
            STOP_WAIT.as_secs()
        ),
    }
}

pub(crate) fn status(root: &Path) -> Result<()> {
    crate::output::insert(
        "observation",
        serde_json::to_value(crate::harness::observation(root))?,
    );
    let (_, detail) = health_report(root);
    println!("ticker: {detail}");
    match lock_state(root) {
        LockState::Free | LockState::Unknown(_) => {}
        LockState::Held(info) => {
            println!("  version: {}", info.version);
            println!("  pid:     {}", info.pid);
            println!("  root:    {}", info.root);
            println!("  started: {}", info.started);
            for (tool, path) in &info.tools {
                println!("  {tool:<6} {path}");
            }
            if !crate::build::same_commit(&info.version, crate::VERSION) {
                println!(
                    "  note: this binary is {}; `ticker start` replaces the running one",
                    crate::VERSION
                );
            }
        }
    }
    Ok(())
}

/// Where a tool resolves from this process's own `PATH`.
fn which(tool: &str, path_var: &str) -> String {
    if tool.contains('/') {
        return tool.to_string();
    }
    std::env::split_paths(path_var)
        .map(|dir| dir.join(tool))
        .find(|candidate| candidate.is_file())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(not found)".to_string())
}

pub(crate) struct Log {
    path: PathBuf,
}

impl Log {
    pub(crate) fn line(&self, text: &str) {
        let Ok(mut file) = File::options().create(true).append(true).open(&self.path) else {
            return;
        };
        let _ = writeln!(file, "{} {}", project::now(), text.replace('\n', " "));
        // Size cap: keep the newer half.
        if file.metadata().map(|m| m.len()).unwrap_or(0) > LOG_CAP
            && let Ok(mut reader) = File::open(&self.path)
        {
            let mut tail = Vec::new();
            if reader.seek(SeekFrom::End(-((LOG_CAP / 2) as i64))).is_ok()
                && reader.read_to_end(&mut tail).is_ok()
            {
                let start = tail.iter().position(|b| *b == b'\n').map_or(0, |i| i + 1);
                let _ = project::write_atomic(&self.path, &tail[start..]);
            }
        }
    }
}

fn mark_clean_exit(root: &Path, log: &Log) {
    if let Err(error) = project::write_atomic(&clean_exit_path(root), b"") {
        log.line(&format!(
            "could not mark intentional ticker exit: {error:#}"
        ));
    }
}

/// The loop stays available while sessions are absent, including on first setup.
/// It exits only for a stop request or when another ticker holds the lock.
pub(crate) fn run(ctx: &Ctx) -> Result<()> {
    std::fs::create_dir_all(&ctx.root)?;
    // `ticker run` can also be invoked directly. Move the loop itself off the
    // caller's possibly disposable worktree, then give every external command
    // the same explicit projects root.
    std::env::set_current_dir(&ctx.root).with_context(|| {
        format!(
            "could not move the ticker to the projects root {}",
            ctx.root.display()
        )
    })?;
    let runner = crate::runner::CwdRunner::new(ctx.runner, &ctx.root);
    let stable = Ctx {
        env: ctx.env,
        root: ctx.root.clone(),
        config_dir: ctx.config_dir.clone(),
        runner: &runner,
        detached_ticker: ctx.detached_ticker,
    };
    let ctx = &stable;
    let root = &ctx.root;
    let mut lock = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path(root))?;
    if lock.try_lock().is_err() {
        return Ok(());
    }
    let path_var = ctx.env.var("PATH").unwrap_or("").to_string();
    let info = Info {
        version: crate::VERSION.to_string(),
        pid: std::process::id(),
        root: root.display().to_string(),
        cwd: root.display().to_string(),
        started: project::now(),
        tools: ["herdr", "git", "gh", "ssh", "scp", "rsync"]
            .iter()
            .map(|tool| {
                let name = if *tool == "herdr" {
                    ctx.env.herdr_bin()
                } else {
                    tool.to_string()
                };
                (tool.to_string(), which(&name, &path_var))
            })
            .collect(),
    };
    let log = Log {
        path: log_path(root),
    };
    let mut memory = Memory::new(ctx);
    let _records = crate::record_cache::Cache::new();

    // Publish only after acquiring the lock, before the first pass: that
    // pass may block, but this process is already running the new image.
    lock.set_len(0)?;
    lock.write_all(serde_json::to_string_pretty(&info)?.as_bytes())?;
    lock.flush()?;
    log.line(&format!(
        "ticker {} started (pid {})",
        info.version, info.pid
    ));
    // A later unexpected exit must not inherit an earlier intentional exit.
    let _ = std::fs::remove_file(clean_exit_path(root));
    if recovery_path(root).exists() {
        let _ = std::fs::remove_file(recovery_path(root));
        let notice = format!(
            "ticker_unavailable: project transitions are queued; the ticker was restarted by launchd StartInterval at {}.",
            info.started
        );
        log.line(&notice);
        let (slugs, errors) = project::list_slugs_with_errors(root);
        let mut health = Health::begin(root, slugs.len());
        for error in errors {
            health.failure(root, &log, format!("{error:#}"));
        }
        for slug in slugs {
            if let Some(project) = load_for_tick(root, &slug, &mut health, &log)
                && project.status() == Status::Active
                && let Err(error) =
                    inbox::write(&project, "ticker-unavailable", "ticker", &notice, "")
            {
                health.failure(root, &log, format!("{slug}: recovery notice: {error:#}"));
            }
        }
        Health::finish_partial(root, &log);
    }
    if let Err(error) = crate::journey::ticker_started(root, &info.started) {
        log.line(&format!(
            "first-pass install evidence unavailable: {error:#}"
        ));
    }
    let mut progress = Progress {
        pid: info.pid,
        started: info.started.clone(),
        ..Progress::default()
    };
    let mut step = |name: &str| {
        progress.sequence += 1;
        progress.step = name.to_string();
        progress.updated = jiff::Timestamp::now().to_string();
        if let Err(error) = project::write_json(&progress_path(root), &progress) {
            log.line(&format!("could not publish ticker progress: {error:#}"));
        }
        !stop_path(root).exists()
    };
    if tick_with_steps(ctx, &log, &mut memory, &mut step).is_none() {
        log.line("stop file found; exiting");
        mark_clean_exit(root, &log);
        return Ok(());
    }

    loop {
        // Sleep in short slices so a stop request is honoured promptly.
        let wake = Instant::now() + TICK;
        while Instant::now() < wake {
            if stop_path(root).exists() {
                log.line("stop file found; exiting");
                mark_clean_exit(root, &log);
                return Ok(());
            }
            if wake_path(root).exists() {
                let _ = std::fs::remove_file(wake_path(root));
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if tick_with_steps(ctx, &log, &mut memory, &mut step).is_none() {
            log.line("stop file found; exiting");
            mark_clean_exit(root, &log);
            return Ok(());
        }
    }
}

/// One pass over every active project. Cheap work (state, prompts, tokens)
/// comes first for every project, then slow work (copies, launches), so one
/// slow project does not delay the others' sidebar. Returns whether any
/// project's session was reachable. A failure in one project never stops the
/// others.
#[cfg(test)]
pub(crate) fn tick(ctx: &Ctx, log: &Log, memory: &mut Memory) -> bool {
    tick_with_steps(ctx, log, memory, &mut |_| true).unwrap_or(false)
}

// None means a stop was requested before the next step began.
fn tick_with_steps(
    ctx: &Ctx,
    log: &Log,
    memory: &mut Memory,
    step: &mut impl FnMut(&str) -> bool,
) -> Option<bool> {
    let mut health = Health::begin(&ctx.root, 0);
    if !step("project discovery") {
        return None;
    }
    let (_awake, slept) = match crate::awake::enter(&ctx.root, true) {
        Ok(clock) => clock,
        Err(error) => {
            health.failure(
                &ctx.root,
                log,
                format!("awake clock: {error:#}; deferring pass"),
            );
            Health::finish_partial(&ctx.root, log);
            return Some(false);
        }
    };
    memory.tick += 1;
    if slept {
        for machine in memory.machines.values_mut() {
            machine.skip_until_tick = 0;
        }
    }
    memory.machine_views.clear();
    // Import box seals before local timers or waiting starts are considered.
    // A dark wake must not turn missing connectivity into lane state changes.
    let (slugs, errors) = project::list_slugs_with_errors(&ctx.root);
    health.projects = slugs.len();
    for error in errors {
        health.failure(&ctx.root, log, format!("{error:#}"));
    }
    let mut projects = Vec::new();
    for slug in slugs {
        let Some(project) = load_for_tick(&ctx.root, &slug, &mut health, log) else {
            continue;
        };
        // A partial lane list must not look like an idle project. Defer only
        // this project; do not mutate unreadable records or create new work.
        let (_, errors) = OBSERVATION_RECORDS.with(|records| {
            records
                .borrow_mut()
                .read(thread::threads_dir(&project), |id| {
                    thread::load(&project, id).map(|_| ()).with_context(|| {
                        format!(
                            "could not observe {}",
                            thread::threads_dir(&project)
                                .join(format!("{id}.toml"))
                                .display()
                        )
                    })
                })
        });
        let errors: Vec<_> = errors
            .into_iter()
            .filter(|error| !is_not_found(error))
            .collect();
        if !errors.is_empty() {
            for error in errors {
                health.failure(
                    &ctx.root,
                    log,
                    format!(
                        "{}: lane records: {error:#}",
                        thread::threads_dir(&project).display()
                    ),
                );
            }
            continue;
        }
        projects.push(project);
    }
    let project_refs: Vec<&Project> = projects.iter().collect();
    if !step("machine phase") {
        return None;
    }
    for error in machine_passes_with_steps(ctx, &project_refs, memory, log, step)? {
        health.failure(&ctx.root, log, format!("{error:#}"));
    }
    // Courier failure holds only that machine's actions. Local observation,
    // accepted seals, reviews and goal checks still run in the same project.
    let mut reachable = Vec::new();
    let mut closed_projects = Vec::new();
    let mut readiness = BTreeMap::new();
    for project in &projects {
        for lane in thread::list_live(project)
            .into_iter()
            .filter(|lane| lane.is_remote())
        {
            if let Some(machine) = memory.machines.get(lane.machine_route())
                && crate::remote::is_unreachable(&machine.outage.last_error)
            {
                readiness.insert(
                    crate::adapters::dependency_key(lane.machine_route(), &lane.launch),
                    Err(machine.outage.last_error.clone()),
                );
            }
        }
    }
    for discovered in &projects {
        let slug = discovered.slug.clone();
        if !step(&format!("cheap project {slug}")) {
            return None;
        }
        let Some(project) = load_for_tick(&ctx.root, &slug, &mut health, log) else {
            continue;
        };
        if project.status() != Status::Active {
            // Pausing a project stops its work, not cleanup of already
            // accepted completions. Sweep its idle tabs on the first pass too.
            if let Err(error) = threads::park_completed(ctx, &project) {
                log.line(&format!("{slug}: parked-lane sweep: {error:#}"));
            }
            continue;
        }
        match coordinator_binding(&project) {
            Ok(None) => {
                health.closed(&ctx.root, log, &project);
                closed_projects.push(project);
                continue;
            }
            Ok(Some(_)) => {}
            Err(error) => {
                health.failure(&ctx.root, log, format!("{error:#}"));
                continue;
            }
        }
        resume_provider_starts(ctx, &project, &mut readiness, |error| {
            health.failure(&ctx.root, log, format!("{slug}: {error:#}"));
        });
        match tick_cheap_observed(
            ctx,
            &project,
            memory.tick == 1 || memory.tick.is_multiple_of(8),
            &mut |detail| health.failure(&ctx.root, log, detail),
        ) {
            Ok(Some(seen)) => reachable.push((project, seen)),
            Ok(None) => {}
            Err(error) => health.failure(
                &ctx.root,
                log,
                format!("{}: {error:#}", project.dir().display()),
            ),
        }
    }
    // Courier imports sealed events and their report artifacts together.
    // Check reviews now, before remote state, launches, or plan work can
    // delay them. A lost local coordinator socket must not hide a box seal.
    for discovered in &projects {
        let slug = &discovered.slug;
        if !step(&format!("reviews {slug}")) {
            return None;
        }
        let Some(project) = load_for_tick(&ctx.root, slug, &mut health, log) else {
            continue;
        };
        if project.status() != Status::Active {
            continue;
        }
        let closed = match coordinator_binding(&project) {
            Ok(record) => record.is_none(),
            Err(error) => {
                health.failure(&ctx.root, log, format!("{error:#}"));
                continue;
            }
        };
        if closed {
            health.closed(&ctx.root, log, &project);
            // Seals remain facts even when no coordinator is open. Consume an
            // arrived verdict, but otherwise skip the session-dependent pass.
            match crate::review::list(&project) {
                Ok(reviews) if reviews.iter().any(|review| sealed_review(&project, review)) => {}
                Ok(_) => continue,
                Err(error) => {
                    health.failure(&ctx.root, log, format!("{slug}: reviews: {error:#}"));
                    continue;
                }
            }
        }
        if let Err(error) = crate::review::tick_observed(ctx, &project, |review| {
            if closed && !sealed_review(&project, review) {
                return false;
            }
            // A missing remote verdict is not evidence of reviewer loss.
            // Other repositories' local piles still advance below it.
            !review
                .reviewer
                .as_deref()
                .and_then(|id| thread::load(&project, id).ok())
                .is_some_and(|lane| {
                    lane.is_remote()
                        && memory
                            .machines
                            .get(lane.machine_route())
                            .is_some_and(|machine| !machine.outage.last_error.is_empty())
                        && crate::events::latest_done_event(
                            &crate::events::for_thread(&project, &lane.id),
                            &lane.id,
                            lane.attempt.max(1),
                        )
                        .is_none()
                })
        }) {
            // Checking an arrived verdict may expose the next pile. Its
            // session-dependent start waits for open, rather than becoming a
            // recurring observation error. All other errors stay loud.
            if !closed
                || error.to_string()
                    != format!(
                        "the herdr session of `{slug}` is not reachable; run `open {slug}` first"
                    )
            {
                health.failure(&ctx.root, log, format!("{slug}: reviews: {error:#}"));
            }
        }
    }
    if !step("slow phase") {
        return None;
    }
    for (project, seen) in &reachable {
        if !step(&format!("slow project {}", project.slug)) {
            return None;
        }
        if load_for_tick(&ctx.root, &project.slug, &mut health, log).is_none() {
            continue;
        }
        let (errors, completed) = tick_slow_with_steps(ctx, project, seen, memory, step);
        for error in errors {
            health.failure(&ctx.root, log, format!("{}: {error:#}", project.slug));
        }
        if !completed {
            return None;
        }
    }
    // A closed local coordinator cannot invalidate a live courier snapshot.
    // Keep remote lanes progressing without running local/session work or
    // starting another review for this project.
    for project in &closed_projects {
        let herdr = Herdr::new(ctx.env.herdr_bin(), "", ctx.runner);
        let mut errors = Vec::new();
        let completed =
            tick_remote_with_steps(ctx, project, &herdr, memory, &mut true, &mut errors, step);
        for error in errors {
            health.failure(&ctx.root, log, format!("{}: {error:#}", project.slug));
        }
        if !completed {
            return None;
        }
    }
    health.publish(&ctx.root, log);
    if let Err(error) = crate::journey::ticker_pass(&ctx.root, &health.failures) {
        log.line(&format!(
            "could not publish first-pass install evidence: {error:#}"
        ));
    }
    if !step("pass complete") {
        return None;
    }
    Some(!reachable.is_empty() || !memory.machines.is_empty())
}

fn sealed_review(project: &Project, review: &crate::review::Review) -> bool {
    !review.phase.closed()
        && review
            .reviewer
            .as_deref()
            .and_then(|id| thread::load(project, id).ok())
            .is_some_and(|lane| {
                crate::review::sealed(&crate::events::for_thread(project, &lane.id), &lane)
                    .is_some()
            })
}

fn load_for_tick(root: &Path, slug: &str, health: &mut Health, log: &Log) -> Option<Project> {
    match Project::load(root, slug) {
        Ok(project) => Some(project),
        // An authorized deletion can race any phase. It is absence, not a
        // broken observation, and no writer here may recreate that project.
        Err(error) if is_not_found(&error) => None,
        Err(error) => {
            health.failure(root, log, format!("{error:#}"));
            None
        }
    }
}

fn is_not_found(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    })
}

fn record_failed_observation(entries: &[(Project, Vec<thread::Thread>)], detail: &str, log: &Log) {
    let attempted = project::now();
    for (project, threads) in entries {
        for lane in threads {
            if let Err(error) = thread::update(project, &lane.id, |record| {
                record.observation_attempted = attempted.clone();
                record.observation_source = "courier".into();
                record.observation_error = steps::short_error(detail);
            }) {
                log.line(&format!("{error:#}"));
            }
        }
    }
}

fn clear_missing_box_panes(entries: &[(Project, Vec<thread::Thread>)], machine: &str, log: &Log) {
    for (project, _) in entries {
        let mut state = crate::events::remote_state(project, machine);
        if !state.missing.is_empty() || !state.missing_identity.is_empty() {
            state.missing.clear();
            state.missing_identity.clear();
            if let Err(error) = crate::events::save_remote_state(project, machine, &state) {
                log.line(&format!("{error:#}"));
            }
        }
    }
}

fn clear_lost_connections(entries: &[(Project, Vec<thread::Thread>)], log: &Log) {
    for (project, threads) in entries {
        for lane in threads {
            if let Err(error) = thread::update(project, &lane.id, |record| {
                if record.failure_class == crate::contracts::FailureClass::LostConnection {
                    record.failure_class = crate::contracts::FailureClass::Unknown;
                    record.last_failure.clear();
                    record.error.clear();
                }
            }) {
                log.line(&format!("{error:#}"));
            }
        }
    }
}

/// One courier pass per saved machine with lanes on each ticker tick.
/// The cadence lives per machine in `Memory`, not per project, and every
/// project with lanes on that machine shares the one SSH trip.
#[cfg(test)]
fn machine_passes(
    ctx: &Ctx,
    projects: &[&Project],
    memory: &mut Memory,
    log: &Log,
) -> Vec<anyhow::Error> {
    machine_passes_with_steps(ctx, projects, memory, log, &mut |_| true).unwrap_or_default()
}

fn machine_passes_with_steps(
    ctx: &Ctx,
    projects: &[&Project],
    memory: &mut Memory,
    log: &Log,
    step: &mut impl FnMut(&str) -> bool,
) -> Option<Vec<anyhow::Error>> {
    let now = jiff::Timestamp::now();
    let mut by_machine: BTreeMap<String, Vec<(Project, Vec<thread::Thread>)>> = BTreeMap::new();
    for project in projects {
        let remote: Vec<_> = thread::list_live(project)
            .into_iter()
            .filter(|t| {
                t.is_remote()
                    && !t.parked
                    && (matches!(t.status, thread::Status::Open | thread::Status::Starting)
                        || (t.status == thread::Status::Failed
                            && t.error.starts_with("brief_delivery_failed:"))
                        || t.recovery_pending)
            })
            .collect();
        let mut seen_machines: Vec<String> = Vec::new();
        for t in &remote {
            if !seen_machines
                .iter()
                .any(|m| m.as_str() == t.machine_route())
            {
                seen_machines.push(t.machine_route().to_string());
            }
        }
        for machine in seen_machines {
            let threads: Vec<thread::Thread> = remote
                .iter()
                .filter(|t| t.machine_route() == machine)
                .cloned()
                .collect();
            by_machine
                .entry(machine)
                .or_default()
                .push(((*project).clone(), threads));
        }
    }
    // Forget a machine once none of this pass's projects needs it. A resolved
    // offline lane must not keep deferring otherwise reachable projects.
    memory
        .machines
        .retain(|machine, _| by_machine.contains_key(machine));
    let mut errors = Vec::new();
    for (machine, entries) in by_machine {
        if !step(&format!("machine {machine}")) {
            return None;
        }
        let forced = entries
            .iter()
            .any(|(project, _)| poll_requests(project).contains(&machine));
        if forced {
            let tick = memory.tick;
            let entry = memory.machines.entry(machine.clone()).or_default();
            entry.last_poll_tick = tick;
            entry.skip_until_tick = 0;
        } else if !memory.machine_is_due(&machine) {
            continue;
        }
        let projects: Vec<&Project> = entries.iter().map(|(project, _)| project).collect();
        let outcome = steps::courier(ctx, &projects, &machine);
        if let Err(error) = &outcome
            && steps::courier_lookup_failed(error)
        {
            let detail = format!("{error:#}");
            errors.push(anyhow::anyhow!("{machine}: {detail}"));
            record_failed_observation(&entries, &detail, log);
            clear_missing_box_panes(&entries, &machine, log);
            // This is local configuration evidence, not evidence about the
            // connection. Leave no remote view for the slow pass and replace
            // any stale connection classification with unknown.
            clear_lost_connections(&entries, log);
            for (project, _) in &entries {
                clear_poll_request(project, &machine);
            }
            continue;
        }
        if let Err(error) = &outcome
            && crate::remote::is_unreachable(&format!("{error:#}"))
        {
            let detail = format!("{error:#}");
            let event = memory.record_machine(&machine, Some(&detail), now);
            write_machine_outages(&entries, &machine, event.as_ref(), memory, &mut errors);
            memory.machine_views.insert(machine, Err(detail));
            continue;
        }
        if let Err(error) = &outcome
            && format!("{error:#}").contains("version_skew:")
        {
            let detail = format!("{error:#}");
            clear_missing_box_panes(&entries, &machine, log);
            clear_lost_connections(&entries, log);
            record_failed_observation(&entries, &detail, log);
            memory.machine_views.insert(machine, Err(detail));
            continue;
        }
        // A failed check (or a box server that could not supply both lists)
        // interrupts a consecutive pane-absence streak.
        if !outcome
            .as_ref()
            .is_ok_and(|view| view.agents.is_some() && view.panes.is_some())
        {
            clear_missing_box_panes(&entries, &machine, log);
        }
        let reason = outcome.as_ref().err().map(|e| format!("{e:#}"));
        if let Some(reason) = &reason {
            record_failed_observation(&entries, reason, log);
        }
        let event = memory.record_machine(&machine, reason.as_deref(), now);
        // After the configured outage period, type one unreachable BLOCKED per
        // open box lane, then stay quiet until the machine answers again
        // (SPEC-remote §4.3). A failed pass never invents GONE.
        if matches!(&event, Some(steps::OutageEvent::Down)) {
            for (project, threads) in &entries {
                for lane in threads {
                    let detail = format!("the link to machine {machine} is unreachable");
                    log.line(&detail);
                    let line = format!("BLOCKED {} machine {machine} unreachable", lane.id);
                    if let Err(error) = steps::type_remote_line(ctx, project, &line) {
                        log.line(&format!("{error:#}"));
                    }
                }
            }
        }
        // A successful courier is direct evidence that the connection works.
        // Clear persisted lost-connection state even after a ticker restart,
        // when the in-memory outage tracker cannot emit `Recovered`.
        if outcome.is_ok() {
            clear_lost_connections(&entries, log);
            match threads::dependency_machine(ctx, &machine) {
                Ok(dependency_machine) => {
                    for (_, lanes) in &entries {
                        for lane in lanes {
                            errors.extend(
                                crate::adapters::machine_reconnected(
                                    &ctx.root,
                                    &dependency_machine,
                                    &lane.launch,
                                )
                                .err(),
                            );
                        }
                    }
                }
                Err(error) => errors.push(error),
            }
        }
        write_machine_outages(&entries, &machine, event.as_ref(), memory, &mut errors);
        for (project, _) in &entries {
            clear_poll_request(project, &machine);
        }
        memory
            .machine_views
            .insert(machine, outcome.map_err(|e| format!("{e:#}")));
    }
    Some(errors)
}

fn write_machine_outages(
    entries: &[(Project, Vec<thread::Thread>)],
    machine: &str,
    event: Option<&steps::OutageEvent>,
    memory: &Memory,
    errors: &mut Vec<anyhow::Error>,
) {
    let mut seen = BTreeSet::new();
    for (project, _) in entries {
        if seen.insert(project.state_dir())
            && let Err(error) = steps::write_machine_outage(project, machine, event, memory)
        {
            errors.push(error.context(format!("{}: machine outage", project.slug)));
        }
    }
}

#[cfg(test)]
pub(crate) fn tick_for_test(ctx: &Ctx, memory: &mut Memory) -> bool {
    let dir = std::env::temp_dir().join(format!("hp-test-log-{}", std::process::id()));
    tick(ctx, &Log { path: dir }, memory)
}

/// What the cheap pass saw, handed to the slow pass so herdr is asked once.
pub(crate) struct Seen {
    socket: String,
    agents: Vec<Agent>,
    panes: Vec<Pane>,
    /// The session answered, the project has at least two recorded local
    /// panes, and every one of them is missing: herdr was restarted.
    session_lost: bool,
}

/// Both passes for one project; `Ok(false)` when its session is unreachable.
#[cfg(test)]
pub(crate) fn tick_project(ctx: &Ctx, project: &Project) -> Result<bool> {
    tick_project_with(ctx, project, &mut Memory::new(ctx))
}

#[cfg(test)]
pub(crate) fn tick_project_with(ctx: &Ctx, project: &Project, memory: &mut Memory) -> Result<bool> {
    let (_awake, slept) = crate::awake::enter(&ctx.root, true)?;
    if slept {
        for machine in memory.machines.values_mut() {
            machine.skip_until_tick = 0;
        }
    }
    memory.machine_views.clear();
    let log = Log {
        path: std::env::temp_dir().join(format!("hp-test-log-{}", std::process::id())),
    };
    for error in machine_passes(ctx, &[project], memory, &log) {
        log.line(&format!("{error:#}"));
    }
    resume_provider_starts(ctx, project, &mut BTreeMap::new(), |_| {});
    match tick_cheap(ctx, project, true)? {
        Some(seen) => match tick_slow(ctx, project, &seen, memory).into_iter().next() {
            Some(error) => Err(error),
            None => Ok(true),
        },
        None => Ok(false),
    }
}

/// Retry placement without using the routing retry or creating duplicate
/// panes. One readiness result is shared across projects for this pass.
pub(crate) fn resume_provider_starts(
    ctx: &Ctx,
    project: &Project,
    cache: &mut BTreeMap<(String, String), Result<(), String>>,
    mut report: impl FnMut(anyhow::Error),
) -> bool {
    let mut reachable = true;
    for lane in thread::list_live(project) {
        if lane.provider_wait_started.is_empty()
            || lane.partial.as_deref() == Some("freeze")
            || !matches!(lane.status, thread::Status::Starting | thread::Status::Open)
        {
            continue;
        }
        if let Err(error) = crate::plan::check_attempt_prerequisites(project, &lane.id) {
            if !format!("{error:#}").starts_with("plan_prerequisite:") {
                report(error);
            }
            continue;
        }
        let machine = if lane.is_remote() {
            lane.machine_route()
        } else {
            crate::contracts::MACHINE_LOCAL
        };
        let key = crate::adapters::dependency_key(machine, &lane.launch);
        let ready = cache.entry(key).or_insert_with(|| {
            let result = if lane.is_remote() {
                threads::box_launch_ready_for(ctx, machine, &lane.launch)
            } else {
                crate::doctor::recipe_ready_local(ctx, &lane.launch)
            };
            result.map_err(|error| format!("{error:#}"))
        });
        if let Err(error) = ready {
            // Installation skew is deferred placement, not provider failure:
            // no auth notice, expiry, or evidence of a dead lane.
            if error.contains("version_skew:") {
                continue;
            }
            if crate::remote::is_unreachable(error) {
                reachable = false;
                continue;
            }
            let dependency_machine = match threads::dependency_machine(ctx, machine) {
                Ok(machine) => machine,
                Err(error) => {
                    report(error);
                    continue;
                }
            };
            let machine = dependency_machine.as_str();
            if let Err(error) =
                crate::adapters::notify_auth(&ctx.root, project, machine, &lane.launch)
            {
                report(error);
            }
            if !crate::adapters::reset_pending(&ctx.root, machine, &lane.launch)
                && thread::seconds_since(&lane.provider_wait_started, jiff::Timestamp::now())
                    >= 3600
            {
                let reason = format!(
                    "provider_wait_expired: recipe `{}` on `{}` was not ready after one hour; {}",
                    lane.launch.recipe_id,
                    lane.machine_route(),
                    lane.error
                );
                if let Err(error) = threads::fail_start(
                    ctx,
                    project,
                    &lane.id,
                    &reason,
                    crate::adapters::dependency_failure_class(&ctx.root, machine, &lane.launch),
                    false,
                ) {
                    report(error);
                }
            }
            continue;
        }
        if lane.status == thread::Status::Open {
            if let Err(error) = thread::update(project, &lane.id, |t| {
                t.provider_wait_started.clear();
                t.startup_wait_started = project::now();
                t.error = "provider ready".into();
            }) {
                report(error);
            }
            continue;
        }
        // Keep the timestamp until placement succeeds; a failed provision
        // must not restart the one-hour provider clock.
        match threads::resume_provider_start(ctx, project, &lane.id) {
            Ok(()) => {
                if let Err(error) = thread::update(project, &lane.id, |t| {
                    t.provider_wait_started.clear();
                    t.error = "provider ready".into();
                }) {
                    report(error);
                }
            }
            Err(error) => {
                if crate::remote::is_unreachable(&format!("{error:#}")) {
                    reachable = false;
                    report(error);
                    continue;
                }
                if let Err(cleanup) = threads::fail_start(
                    ctx,
                    project,
                    &lane.id,
                    &format!("placement after provider became ready failed: {error:#}"),
                    crate::contracts::FailureClass::Unknown,
                    false,
                ) {
                    report(cleanup);
                }
                report(error);
            }
        }
    }
    reachable
}

/// State, pending prompts, group and tokens for a set of threads that live in
/// one herdr server (the local session, or one remote machine).
struct Pass {
    recorded_panes: usize,
    missing_panes: usize,
    error: Option<anyhow::Error>,
}

/// A pane still not ready after the recorded window is a startup failure.
/// Keep its screen visible and notify a ready local coordinator.
fn startup_failure(input: &LaunchPass<'_>, thread: &thread::Thread, detail: &str) -> Result<()> {
    let screen = threads::startup_screen(input.herdr, &thread.pane_id);
    let reason = format!("agent_not_ready: screen: {screen}; herdr: {detail}");
    thread::update(input.project, &thread.id, |t| {
        t.status = thread::Status::Failed;
        t.prompt_pending = false;
        t.startup_wait_started.clear();
        t.error = reason.clone();
        t.failure_class = crate::contracts::FailureClass::Unknown;
        t.last_group = thread::Group::WaitingOnYou.token().into();
        t.start_notices.push(steps::Notice {
            line: format!(
                "FAILED {}: {reason} — next: {}",
                thread.id,
                threads::retry_command(&input.project.slug, &thread.id)
            ),
            submitted: false,
        });
    })?;
    Ok(())
}

fn connection_error(screen: &str) -> bool {
    let screen = screen.to_ascii_lowercase();
    [
        "websocket closed",
        "unable to reach the model provider",
        "connection lost",
        "your computer went to sleep",
        "connection error",
        "network error",
        "failed to connect",
        "provider unavailable",
        "service unavailable",
        "stream disconnected",
        "error communicating with provider",
        "failed to fetch",
        "stream error",
        "connection reset by peer",
    ]
    .iter()
    .any(|text| screen.contains(text))
}

/// Close only the delivered follow-ups overtaking this exact seal. An unknown
/// machine/git observation leaves the seal pending, never presumed unchanged.
pub(crate) fn restore_unchanged_seal(
    ctx: &Ctx,
    project: &Project,
    lane: &thread::Thread,
) -> Result<()> {
    let events = crate::events::checked_for_thread(project, &lane.id)?;
    let Some(event) = crate::events::latest_done_event(&events, &lane.id, lane.attempt.max(1))
    else {
        return Ok(());
    };
    let pending: Vec<_> = lane
        .follow_ups
        .iter()
        .filter(|f| {
            f.attempt == lane.attempt.max(1)
                && (matches!(
                    f.state,
                    thread::FollowUpState::Queued | thread::FollowUpState::Uncertain
                ) || f.state == thread::FollowUpState::Delivered && f.after_seal == event.id)
        })
        .collect();
    if pending.is_empty()
        || pending.iter().any(|f| {
            f.state != thread::FollowUpState::Delivered
            // The first idle snapshot can predate prompt delivery. Give the
            // agent a full observation window before accepting idle again.
            || thread::seconds_since(&f.delivered_at, jiff::Timestamp::now()) < 30
        })
    {
        return Ok(());
    }
    let done = event.payload.done.as_ref().expect("latest done");
    if done.sha.is_empty() || lane.worktree_path.is_empty() {
        return Ok(());
    }
    let folder = &lane.worktree_path;
    let unchanged = if lane.is_remote() {
        let profile = crate::remote::machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            lane.machine_route(),
        )?;
        let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
        let script = crate::remote::with_path(
            &machine.path,
            &format!(
                "cd {} && git rev-parse HEAD && git status --porcelain --untracked-files=all",
                crate::remote::quote(folder)
            ),
        );
        let output = crate::remote::ssh(
            ctx.runner,
            &profile.target,
            &script,
            None,
            Duration::from_secs(20),
        )?;
        output.success()
            && output.stdout.lines().next() == Some(done.sha.as_str())
            && output.stdout.lines().count() == 1
    } else {
        let git = crate::repo::Git::new(ctx.runner, folder).with_timeout(Duration::from_secs(20));
        git.run(&["rev-parse", "HEAD"])? == done.sha
            && git
                .stdout(&["status", "--porcelain", "--untracked-files=all"])?
                .is_empty()
    };
    if unchanged {
        thread::update_checked(project, &lane.id, |current| {
            // A prompt or re-seal arriving during the git check must not
            // borrow this confirmation. Leave it for a fresh observation.
            if current.attempt != lane.attempt
                || current.pane_id != lane.pane_id
                || current.follow_ups != lane.follow_ups
                || crate::events::latest_done_event(
                    &crate::events::checked_for_thread(project, &lane.id)?,
                    &lane.id,
                    lane.attempt.max(1),
                )
                .is_none_or(|latest| latest.id != event.id)
            {
                return Ok(());
            }
            if current.review_after == event.id {
                current.review_after.clear();
            }
            for follow_up in &mut current.follow_ups {
                if follow_up.attempt == lane.attempt.max(1)
                    && follow_up.state == thread::FollowUpState::Delivered
                    && follow_up.after_seal == event.id
                    && thread::seconds_since(&follow_up.delivered_at, jiff::Timestamp::now()) >= 30
                {
                    follow_up.state = thread::FollowUpState::Closed;
                    follow_up.closed_at = project::now();
                }
            }
            current.start_notices.push(steps::Notice {
                line: format!(
                    "{} answered the post-seal note without changes; confirmed existing seal {}.",
                    lane.id, event.id
                ),
                submitted: false,
            });
            Ok(())
        })?;
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(default)]
struct ProgressThresholds {
    stall_minutes: u64,
    no_commit_minutes: u64,
}

impl Default for ProgressThresholds {
    fn default() -> Self {
        Self {
            stall_minutes: 20,
            no_commit_minutes: 90,
        }
    }
}

#[derive(Default)]
struct ProgressNotices {
    stalled: Option<i64>,
    no_commit: Option<i64>,
}

fn reset_progress(record: &mut thread::Thread) {
    record.progress_since.clear();
    record.progress_screen.clear();
    record.progress_head.clear();
    record.progress_pane.clear();
    record.stall_notified = false;
    record.no_commit_since.clear();
    record.no_commit_notified = false;
}

fn progress_notices(
    record: &mut thread::Thread,
    observation: &steps::LaneProgress,
    now: jiff::Timestamp,
    config: &ProgressThresholds,
) -> ProgressNotices {
    let mut notices = ProgressNotices::default();
    if record.progress_since.is_empty()
        || record.progress_pane != observation.pane
        || record.progress_screen != observation.screen
        || record.progress_head != observation.head
    {
        record.progress_pane = observation.pane.clone();
        record.progress_screen = observation.screen.clone();
        record.progress_since = now.to_string();
        record.stall_notified = false;
    } else {
        let minutes = thread::seconds_since(&record.progress_since, now) / 60;
        if minutes >= config.stall_minutes as i64 && !record.stall_notified {
            notices.stalled = Some(minutes);
            record.stall_notified = true;
        }
    }
    if record.repo.is_empty() {
        record.no_commit_since.clear();
        record.no_commit_notified = false;
    } else if record.no_commit_since.is_empty() || record.progress_head != observation.head {
        record.no_commit_since = now.to_string();
        record.no_commit_notified = false;
    } else {
        let minutes = thread::seconds_since(&record.no_commit_since, now) / 60;
        if minutes >= config.no_commit_minutes as i64 && !record.no_commit_notified {
            notices.no_commit = Some(minutes);
            record.no_commit_notified = true;
        }
    }
    record.progress_head = observation.head.clone();
    notices
}

fn progress_notice_lines(notices: &ProgressNotices, lane: &str, pane: &str) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(minutes) = notices.stalled {
        lines.push(format!("STALLED {lane} has shown no new output and no new commit for {minutes} min in {pane}; check it, then ha thread prompt, ha thread retry --reason, or cancel."));
    }
    if let Some(minutes) = notices.no_commit {
        lines.push(format!(
            "{lane} has worked {minutes} min with no commit; check it is still on its task."
        ));
    }
    lines
}

fn resume_session(
    ctx: &Ctx,
    project: &Project,
    herdr: &Herdr<'_>,
    agents: &[Agent],
    t: &thread::Thread,
    recent: Vec<String>,
    prompt: &str,
) -> Result<()> {
    let can_resume = |record: &thread::Thread| {
        record.attempt == t.attempt
            && record.pane_id == t.pane_id
            && record.status == thread::Status::Open
            && !record.connection_waiting
            && record.connection_resumes == t.connection_resumes
    };
    if !can_resume(&thread::load(project, &t.id)?) {
        return Ok(());
    }
    let mut claimed = false;
    thread::update(project, &t.id, |record| {
        if !can_resume(record) {
            return;
        }
        record.connection_waiting = true;
        record.error = "connection_resume_uncertain: delivery may have taken effect; reconcile the bound pane before resending".into();
        record.connection_resumes = recent;
        claimed = true;
    })?;
    if !claimed {
        return Ok(());
    }
    match threads::send_lane_input(ctx, project, &t.id, Some(prompt), Some((herdr, agents, t))) {
        Ok(_) => Ok(()),
        Err(error) => {
            let saved = thread::load(project, &t.id)?;
            if !threads::follow_up_pending_for_seal(&saved, None) {
                thread::update(project, &t.id, |record| {
                    if record.attempt == t.attempt && record.pane_id == t.pane_id {
                        record.connection_waiting = false;
                        record.error.clear();
                    }
                })?;
            }
            Err(error)
        }
    }
}

/// The first prompt carries queued corrections too. Confirm them only with
/// activity/receipt evidence, never merely because a PTY paste was staged.
fn acknowledge_first_prompt(project: &Project, submitted: &thread::Thread) -> Result<()> {
    let after_seal = crate::events::latest_done_event(
        &crate::events::for_thread(project, &submitted.id),
        &submitted.id,
        submitted.attempt.max(1),
    )
    .map(|event| event.id.clone())
    .unwrap_or_default();
    thread::update(project, &submitted.id, |record| {
        if record.attempt != submitted.attempt || record.pane_id != submitted.pane_id {
            return;
        }
        record.prompt_pending = false;
        record.error.clear();
        for follow_up in &mut record.follow_ups {
            if follow_up.attempt == record.attempt.max(1)
                && follow_up.carried_from_attempt > 0
                && follow_up.state == thread::FollowUpState::Uncertain
                && follow_up.queued_at <= record.brief_submitted_at
            {
                follow_up.state = thread::FollowUpState::Delivered;
                follow_up.delivered_at = project::now();
                follow_up.after_seal = after_seal.clone();
                record.connection_waiting = false;
                record.connection_resumes.clear();
                record.failure_class = crate::contracts::FailureClass::Unknown;
                record.provider_failure_kind = None;
                if !follow_up.waiting_event.is_empty() {
                    record.answered_waiting_event = follow_up.waiting_event.clone();
                }
            }
        }
    })?;
    Ok(())
}

/// Machine transport supplies facts; this pass alone owns lane transitions.
#[derive(Clone, Copy)]
pub(crate) struct ObservationView<'a> {
    pub(crate) machine_id: &'a str,
    pub(crate) threads: &'a [thread::Thread],
    pub(crate) agents: &'a [Agent],
    pub(crate) panes: &'a [Pane],
    pub(crate) boot_id: &'a str,
    pub(crate) now: jiff::Timestamp,
}

#[derive(Clone, Copy, PartialEq)]
enum Presence {
    Present,
    Absent,
    Unavailable,
}

struct LaneObservation {
    presence: Presence,
    live: thread::Live,
    identity: String,
    rebooted: bool,
}

impl LaneObservation {
    fn read(
        lane: &thread::Thread,
        view: ObservationView<'_>,
        herdr: &Herdr,
        rebooted: bool,
    ) -> Self {
        let mut live = thread::live_state(lane, view.agents, view.panes, view.now);
        if lane.is_remote() && live.agent_state.as_deref() == Some("blocked") {
            live.state_secs = live.state_secs.max(thread::BLOCKED_DEBOUNCE_SECS);
        }
        let presence = if !live.pane_exists {
            Presence::Absent
        } else if live.agent_state.is_some() {
            Presence::Present
        } else if !view.agents.iter().any(|a| a.pane_id == lane.pane_id)
            && thread::can_check_process_gone(lane, view.now)
            && herdr
                .pane_process_info(&lane.pane_id)
                .is_ok_and(|info| info.agent_gone(&lane.pane_id))
        {
            Presence::Absent
        } else {
            // A terminal with no registered agent is not a dead process.
            Presence::Unavailable
        };
        Self {
            presence,
            live,
            identity: format!(
                "{}:{}:{}:{}",
                lane.attempt, lane.workspace_id, lane.tab_id, lane.pane_id
            ),
            rebooted,
        }
    }
}

pub(crate) fn observation_pass(
    ctx: &Ctx,
    project: &Project,
    view: ObservationView<'_>,
) -> Vec<anyhow::Error> {
    let remote = !view.machine_id.is_empty();
    let mut state = if remote {
        crate::events::remote_state(project, view.machine_id)
    } else {
        Default::default()
    };
    let boot_changed =
        !state.boot_id.is_empty() && !view.boot_id.is_empty() && state.boot_id != view.boot_id;
    if !view.boot_id.is_empty() {
        state.boot_id = view.boot_id.into();
    }
    if boot_changed {
        state.gone.clear();
        state.missing.clear();
        state.missing_identity.clear();
        state.pending_gone = view
            .threads
            .iter()
            .filter(|lane| lane.launch_attempts > 0)
            .map(|lane| lane.id.clone())
            .collect();
    }
    let whole_session_missing = !remote
        && project.coordinator().is_some_and(|c| {
            !view
                .agents
                .iter()
                .any(|a| coordinator::agent_matches(&c, a))
                && !view.panes.iter().any(|p| coordinator::pane_matches(&c, p))
        })
        && view.threads.iter().any(|t| !t.pane_id.is_empty())
        && view.threads.iter().all(|t| {
            t.pane_id.is_empty()
                || !thread::live_state(t, view.agents, view.panes, view.now).pane_exists
        });
    let mut errors = Vec::new();
    for lane in view.threads {
        if lane.pane_id.is_empty() {
            state.missing.remove(&lane.id);
            state.missing_identity.remove(&lane.id);
            state.gone.remove(&lane.id);
            state.pending_gone.remove(&lane.id);
            continue;
        }
        let result = (|| -> Result<()> {
            let current = thread::load(project, &lane.id)?;
            let owns = |record: &thread::Thread| {
                matches!(
                    record.status,
                    thread::Status::Open | thread::Status::Starting
                ) && record.attempt == lane.attempt
                    && record.pane_id == lane.pane_id
                    && record.tab_id == lane.tab_id
                    && record.workspace_id == lane.workspace_id
            };
            if !owns(&current) {
                return Ok(());
            }
            let herdr = Herdr::new(
                ctx.env.herdr_bin(),
                project.coordinator().map(|c| c.socket).unwrap_or_default(),
                ctx.runner,
            )
            .on_machine(lane.machine_route());
            let observation = LaneObservation::read(
                &current,
                view,
                &herdr,
                state.pending_gone.contains(&lane.id),
            );
            if state
                .attention_identity
                .get(&lane.id)
                .is_some_and(|old| old != &observation.identity)
            {
                state.gone.remove(&lane.id);
                state.pending_gone.remove(&lane.id);
                state.missing.remove(&lane.id);
                state.missing_identity.remove(&lane.id);
            }
            state
                .attention_identity
                .insert(lane.id.clone(), observation.identity.clone());
            let eligible = !current.pane_id.is_empty()
                && thread::can_check_gone(&current, view.now)
                && !threads::attempt_sealed(project, &current);
            let rebooted = eligible
                && observation.rebooted
                && observation.presence != Presence::Absent
                && state.pending_gone.contains(&lane.id);
            let gone = if !eligible || observation.presence != Presence::Absent {
                state.missing.remove(&lane.id);
                state.missing_identity.remove(&lane.id);
                if !eligible {
                    state.gone.remove(&lane.id);
                    state.pending_gone.remove(&lane.id);
                }
                rebooted
            } else if remote {
                if state.missing_identity.get(&lane.id) != Some(&observation.identity) {
                    state.missing.insert(lane.id.clone(), 0);
                    state
                        .missing_identity
                        .insert(lane.id.clone(), observation.identity);
                }
                let count = state.missing.entry(lane.id.clone()).or_default();
                *count = count.saturating_add(1);
                *count >= 2 // Only successful snapshots spend the absence budget.
            } else {
                !whole_session_missing
            };
            if gone {
                if remote {
                    crate::events::save_remote_state(project, view.machine_id, &state)?;
                }
                // A concurrent placement can postdate either machine's snapshot.
                let current = thread::load(project, &lane.id)?;
                if !owns(&current)
                    || !thread::can_check_gone(&current, view.now)
                    || threads::attempt_sealed(project, &current)
                {
                    return Ok(());
                }
                if !rebooted {
                    match herdr.pane_list() {
                        Ok(panes)
                            if panes.iter().any(|pane| {
                                pane.pane_id == current.pane_id
                                    && pane.tab_id == current.tab_id
                                    && pane.workspace_id == current.workspace_id
                                    && (!remote || pane.cwd == current.cwd)
                            }) =>
                        {
                            if !observation.live.pane_exists
                                || !herdr.agent_list().is_ok_and(|agents| {
                                    !agents.iter().any(|a| a.pane_id == current.pane_id)
                                })
                                || !herdr
                                    .pane_process_info(&current.pane_id)
                                    .is_ok_and(|info| info.agent_gone(&current.pane_id))
                            {
                                state.missing.remove(&lane.id);
                                state.missing_identity.remove(&lane.id);
                                return Ok(());
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            state.missing.insert(lane.id.clone(), 1);
                            return Err(anyhow::anyhow!(
                                "{}: recheck missing pane: {error}",
                                lane.id
                            ));
                        }
                    }
                }
                let detail = if rebooted {
                    "the machine rebooted before this attempt sealed"
                } else {
                    "the pane or agent is gone without a report"
                };
                if threads::fail_start_checked(
                    ctx,
                    project,
                    &lane.id,
                    detail,
                    crate::contracts::FailureClass::ProcessGone,
                    !current.launch.recipe_id.is_empty(),
                    Some(&current),
                )?
                .is_some()
                {
                    state.gone.insert(lane.id.clone());
                    if rebooted {
                        threads::close_pane(ctx, project, &current)?;
                    }
                }
                state.missing.remove(&lane.id);
                state.missing_identity.remove(&lane.id);
                state.pending_gone.remove(&lane.id);
                return Ok(());
            }
            if current.parked
                || current.recovery_pending
                || !current.provider_wait_started.is_empty()
            {
                return Ok(());
            }
            let group = thread::group(&current, &observation.live, view.now);
            let blocked = observation.live.agent_state.as_deref() == Some("blocked");
            if current.last_group == group.token() && (!blocked || current.last_state == "blocked")
            {
                return Ok(());
            }
            // A missing snapshot can already have selected WaitingOnYou. Claim
            // the blocked state and its notice together, before transport.
            thread::update(project, &lane.id, |record| {
                if !owns(record) {
                    return;
                }
                if record.status == thread::Status::Open
                    && record.startup_wait_started.is_empty()
                    && blocked
                    && group == thread::Group::WaitingOnYou
                    && (record.last_group != group.token() || record.last_state != "blocked")
                {
                    if record.last_state != "blocked" {
                        record.last_state = "blocked".into();
                        record.last_state_change = project::now();
                    }
                    record.start_notices.push(steps::Notice {
                        line: format!("BLOCKED {} needs input in {}; inspect the current question; no keys were sent.", record.id, record.pane_id),
                        submitted: false,
                    });
                }
                record.last_group = group.token().into();
            })?;
            Ok(())
        })();
        errors.extend(result.err());
    }
    if remote {
        errors.extend(crate::events::save_remote_state(project, view.machine_id, &state).err());
    }
    errors
}

fn thread_pass(
    input: &LaunchPass<'_>,
    prefix: &str,
    hashes: Option<&std::collections::BTreeMap<String, String>>,
    refresh_tokens: bool,
    box_progress: Option<&std::collections::BTreeMap<(String, String), steps::LaneProgress>>,
) -> Result<Pass> {
    thread_pass_observed(input, prefix, hashes, refresh_tokens, box_progress, None)
}

fn thread_pass_observed(
    input: &LaunchPass<'_>,
    prefix: &str,
    hashes: Option<&std::collections::BTreeMap<String, String>>,
    refresh_tokens: bool,
    box_progress: Option<&std::collections::BTreeMap<(String, String), steps::LaneProgress>>,
    remote_view: Option<ObservationView<'_>>,
) -> Result<Pass> {
    let ctx = input.ctx;
    let thresholds: ProgressThresholds =
        crate::config::Document::read(&ctx.config_dir)?.section("ticker")?;
    let project = input.project;
    let herdr = input.herdr;
    let threads = input.threads;
    let agents = input.agents;
    let panes = input.panes;
    let slug = &project.slug;
    let now = jiff::Timestamp::now();
    let mut pass = Pass {
        recorded_panes: 0,
        missing_panes: 0,
        error: None,
    };
    let view = remote_view.unwrap_or(ObservationView {
        machine_id: threads
            .first()
            .filter(|t| t.is_remote())
            .map_or("", |t| t.machine_route()),
        threads,
        agents,
        panes,
        boot_id: "",
        now,
    });
    pass.error = observation_pass(ctx, project, view).into_iter().next();
    for t in threads {
        if !t.pane_id.is_empty() {
            let current = thread::load(project, &t.id)?;
            if current.attempt != t.attempt
                || current.pane_id != t.pane_id
                || current.status != t.status
            {
                continue;
            }
        }
        if t.status == thread::Status::Failed {
            // Late activity or a receipt can settle an uncertain delivery. No
            // replacement or second paste is needed, even after its deadline.
            if t.error.starts_with("brief_delivery_failed:")
                && (t.bootstrap == "acknowledged"
                    || thread::live_state(t, agents, panes, now)
                        .agent_state
                        .as_deref()
                        .is_some_and(|state| matches!(state, "working" | "blocked")))
            {
                thread::update(project, &t.id, |current| {
                    if current.status == thread::Status::Failed
                        && current.attempt == t.attempt
                        && current.pane_id == t.pane_id
                    {
                        current.status = thread::Status::Open;
                        current.prompt_pending = false;
                        current.error.clear();
                        current.last_group = "working".into();
                    }
                })?;
                acknowledge_first_prompt(project, t)?;
                continue;
            }
            let records = thread::list_live(project);
            if let Some(agent) = thread::recoverable_agent(t, &records, agents, panes) {
                thread::update_checked(project, &t.id, |current| {
                    if current.status == thread::Status::Failed
                        && current.attempt == t.attempt
                        && current.pane_id == t.pane_id
                        && !thread::list_live(project).iter().any(|other| {
                            other.id != current.id
                                && other.status != thread::Status::Resolved
                                && other.pane_id == current.pane_id
                        })
                    {
                        current.status = thread::Status::Open;
                        // The ready-window failure cleared the undelivered
                        // brief. Restore it now that the agent is registered.
                        if current.bootstrap != "acknowledged" {
                            current.prompt_pending = true;
                        }
                        current.error.clear();
                        current.recovery_pending = false;
                        current.failure_class = crate::contracts::FailureClass::Unknown;
                        current.agent_name = agent.name.clone();
                        current.last_state = agent.agent_status.clone();
                        current.last_state_change = project::now();
                        current.last_group = "working".into();
                        current.startup_wait_started.clear();
                    }
                    Ok(())
                })?;
            }
            continue;
        }
        if t.parked || t.recovery_pending || !t.provider_wait_started.is_empty() {
            continue;
        }
        if t.status == thread::Status::Starting && t.startup_wait_started.is_empty() {
            if thread::seconds_since(&t.created, now) >= thread::STARTING_TIMEOUT_SECS {
                threads::fail_start(
                    ctx,
                    project,
                    &t.id,
                    "still starting after five minutes",
                    crate::contracts::FailureClass::Unknown,
                    false,
                )?;
            }
            continue;
        }
        // No recorded terminal is not a missing process. Placement owns this
        // lane; a startup timestamp alone supplies no identity to reconcile.
        if t.pane_id.is_empty() {
            continue;
        }
        let mut live = thread::live_state(t, agents, panes, now);
        if !t.pane_id.is_empty() {
            pass.recorded_panes += 1;
            pass.missing_panes += usize::from(!live.pane_exists);
        }
        let state = live.agent_state.clone().unwrap_or_default();
        if state != t.last_state {
            live.state_secs = 0;
        }
        // A remote thread is polled on each ticker tick, so `blocked` at a poll
        // already counts: there is no finer clock to debounce against.
        if t.is_remote() && state == "blocked" {
            live.state_secs = live.state_secs.max(thread::BLOCKED_DEBOUNCE_SECS);
        }

        // Herdr drops the name when `agent start` times out, but leaves the
        // process in its pane. Reclaim only an unnamed agent in our exact
        // terminal before delivering the pending brief.
        let unnamed = if !t.startup_wait_started.is_empty() && live.agent_state.is_none() {
            agents.iter().find(|agent| {
                agent.name.is_empty()
                    && agent.pane_id == t.pane_id
                    && agent.tab_id == t.tab_id
                    && agent.workspace_id == t.workspace_id
                    && agent.cwd == t.cwd
            })
        } else {
            None
        };
        if let Some(agent) = unnamed {
            if crate::herdr::ready_state(&agent.agent_status) {
                herdr.agent_rename(&t.pane_id, &t.agent_name)?;
                let process = herdr
                    .pane_process_info(&t.pane_id)
                    .ok()
                    .and_then(|info| info.identity(&t.launch.kind));
                let socket = project.coordinator().map(|c| c.socket).unwrap_or_default();
                thread::update(project, &t.id, |record| {
                    thread::bind_identity(record, &socket, agent, process);
                })?;
            }
            live.agent_state = Some(agent.agent_status.clone());
        }
        let state = live.agent_state.clone().unwrap_or_default();
        let ready = live
            .agent_state
            .as_deref()
            .is_some_and(crate::herdr::ready_state);
        if !t.startup_wait_started.is_empty() || (t.bootstrap == "resuming" && ready) {
            if state == "blocked" && !t.trust_answered {
                // A refused trust check leaves the normal ready-window failure
                // visible; only Claude itself may persist an accepted dialog.
                if let Err(error) = crate::claude_trust::answer(input.ctx, project, t, herdr) {
                    pass.error = pass
                        .error
                        .or(Some(error.context(format!("{}: trust dialog", t.id))));
                }
            }
            if ready {
                if t.is_remote() && !t.tab_id.is_empty() {
                    herdr.tab_rename(&t.tab_id, &t.id)?;
                }
                thread::update(project, &t.id, |t| {
                    t.startup_wait_started.clear();
                    t.status = thread::Status::Open;
                    if t.bootstrap == "resuming" {
                        t.bootstrap = "acknowledged".into();
                        t.prompt_pending = false;
                    }
                })?;
            } else if (state == "blocked" || (state.is_empty() && live.pane_exists))
                && (t.launch_attempts > 0 || thread::process_bound_to_pane(t))
                && !thread::in_start_window(t, now)
                && !(state.is_empty()
                    && !agents.iter().any(|a| a.pane_id == t.pane_id)
                    && herdr
                        .pane_process_info(&t.pane_id)
                        .is_ok_and(|info| info.agent_gone(&t.pane_id)))
            {
                let detail = if state == "blocked" {
                    "still blocked at the end of its ready window"
                } else {
                    "agent state unknown at the end of its ready window"
                };
                startup_failure(input, t, detail)?;
                continue;
            }
        }
        // Dependency evidence parks only affected actions. Keep the exact live
        // session; a shared readiness probe precedes any in-place resume.
        if state == "idle"
            && t.status == thread::Status::Open
            && !t.prompt_pending
            && t.bootstrap == "acknowledged"
            && !t.connection_waiting
            && t.report_hash.is_empty()
            && crate::events::latest_event(
                &crate::events::for_thread(project, &t.id),
                &t.id,
                t.attempt.max(1),
            )
            .is_none()
            && let Ok(screen) = herdr.pane_read_text(&t.pane_id, "visible")
            && let Some((detail, evidence)) =
                crate::adapters::terminal_dependency(&screen).or_else(|| {
                    connection_error(&screen).then(|| {
                        (
                            screen.clone(),
                            crate::adapters::DependencyEvidence {
                                kind: "connectivity".into(),
                                reset_at: None,
                            },
                        )
                    })
                })
        {
            let machine = if t.is_remote() {
                t.machine_route()
            } else {
                crate::contracts::MACHINE_LOCAL
            };
            let dependency_machine = threads::dependency_machine(ctx, machine)?;
            let machine = dependency_machine.as_str();
            if t.provider_failure_kind.as_deref() != Some(&evidence.kind) {
                crate::adapters::observe_dependency(
                    &ctx.root, machine, &t.launch, &detail, &evidence,
                )?;
                thread::update(project, &t.id, |record| {
                    record.provider_failure_kind = Some(evidence.kind.clone());
                    record.failure_class = match evidence.kind.as_str() {
                        "auth" | "quota" => crate::contracts::FailureClass::Provider,
                        "connectivity" => crate::contracts::FailureClass::LostConnection,
                        _ => crate::contracts::FailureClass::Unknown,
                    };
                })?;
            }
            crate::adapters::notify_auth(&ctx.root, project, machine, &t.launch)?;
            let ready = if t.is_remote() {
                threads::box_launch_ready_for(ctx, machine, &t.launch)
            } else {
                crate::doctor::recipe_ready_local(ctx, &t.launch)
            };
            if ready.is_err() {
                continue;
            }
            let recent: Vec<_> = t
                .connection_resumes
                .iter()
                .filter(|at| thread::seconds_since(at, now) < 3600)
                .cloned()
                .collect();
            if recent.len() >= 3 {
                thread::update(project, &t.id, |record| {
                    record.connection_resumes = recent;
                    record.connection_waiting = true;
                    record.last_group = thread::Group::WaitingOnYou.token().into();
                })?;
                crate::inbox::write(
                    project,
                    "lane-dependency",
                    &t.id,
                    "Dependency recovery paused after three in-place resumes within an hour; diagnose this lane without restarting its work budget.",
                    "",
                )?;
                continue;
            }
            if recent
                .last()
                .is_some_and(|at| thread::seconds_since(at, now) < 60)
            {
                continue;
            }
            resume_session(
                ctx,
                project,
                herdr,
                agents,
                t,
                recent,
                "The dependency is reachable again. Continue the current task in this session; do not start a new attempt or repeat completed work or uncertain effects.",
            )?;
            continue;
        }
        if state == "working"
            && (t.provider_failure_kind.is_some()
                || t.error.starts_with("connection_resume_uncertain:"))
        {
            thread::update(project, &t.id, |record| {
                record.provider_failure_kind = None;
                if record.error.starts_with("connection_resume_uncertain:") {
                    record.connection_waiting = false;
                    record.error.clear();
                    record.connection_resumes.push(project::now());
                }
            })?;
        }
        let mut delivered = false;
        if t.bootstrap == "resuming" && ready {
            // Readiness established the resumed conversation's receipt above.
            // Drain its persisted correction through the ordinary next pass.
            continue;
        }
        if t.prompt_pending && (ready || t.brief_submitted || t.bootstrap == "acknowledged") {
            // The CLI and ticker can observe the same ready agent. Serialize
            // the first prompt and recheck its attempt before either sends it.
            let _prompt_lock = thread::prompt_lock(project, &t.id)?;
            let current = thread::load(project, &t.id)?;
            if current.attempt == t.attempt
                && current.pane_id == t.pane_id
                && current.status == thread::Status::Open
                && current.prompt_pending
            {
                if current.brief_submitted || current.bootstrap == "acknowledged" {
                    // A capped wait can finish before activity is observed.
                    // Never replay an ambiguous paste, but bound the wait so
                    // a command lost before submission cannot hang forever.
                    if current.bootstrap == "acknowledged"
                        || matches!(state.as_str(), "working" | "blocked")
                    {
                        acknowledge_first_prompt(project, &current)?;
                    } else if current.brief_submitted_at.is_empty() {
                        // Historical staged submissions have no timestamp. Give
                        // them a full observation window after installation.
                        thread::update(project, &t.id, |record| {
                            record.brief_submitted_at = project::now();
                        })?;
                    } else if ready
                        && thread::seconds_since(&current.brief_submitted_at, now).max(0) as u64
                            * 1000
                            >= brief_delivery_timeout(&current.launch)
                    {
                        let screen = threads::startup_screen(herdr, &t.pane_id);
                        let mut detail = format!(
                            "brief_delivery_failed: no activity or bootstrap receipt since {}; last submission: {}; screen: {screen}. Check the pane, then thread retry --reason",
                            current.brief_submitted_at,
                            if current.error.is_empty() {
                                "outcome unknown"
                            } else {
                                &current.error
                            },
                        );
                        let undelivered: Vec<_> = current
                            .follow_ups
                            .iter()
                            .enumerate()
                            .filter(|(_, f)| {
                                f.attempt == current.attempt.max(1)
                                    && matches!(
                                        f.state,
                                        thread::FollowUpState::Queued
                                            | thread::FollowUpState::Uncertain
                                    )
                            })
                            .map(|(index, _)| format!("follow-up {}", index + 1))
                            .collect();
                        if !undelivered.is_empty() {
                            detail.push_str(&format!(
                                "; delivery not established for {}",
                                undelivered.join(", ")
                            ));
                        }
                        let mut failed = false;
                        thread::update(project, &t.id, |record| {
                            if record.attempt == current.attempt
                                && record.pane_id == current.pane_id
                                && record.status == thread::Status::Open
                                && record.prompt_pending
                                && record.bootstrap != "acknowledged"
                            {
                                // Keep the process and worktree for diagnosis.
                                // Explicit retry stays on the selected recipe.
                                record.status = thread::Status::Failed;
                                record.error = detail.clone();
                                record.failure_class = crate::contracts::FailureClass::Unknown;
                                record.last_group = thread::Group::WaitingOnYou.token().into();
                                failed = true;
                            }
                        })?;
                        if failed {
                            inbox::write(project, "brief-delivery", &t.id, &detail, "")?;
                        }
                        continue;
                    }
                } else {
                    // Staging is not proof of delivery. Persist its deadline
                    // before calling herdr, including interruption before send.
                    let remote_prefix;
                    let prefix = if current.is_remote() {
                        let machine = crate::remote::declaration_for_route(
                            ctx.runner,
                            &ctx.env.herdr_bin(),
                            &ctx.config_dir,
                            current.machine_route(),
                        )?;
                        remote_prefix = format!(
                            "{} --root {}",
                            crate::remote::quote(&machine.ade_bin),
                            crate::remote::quote(&machine.root)
                        );
                        &remote_prefix
                    } else {
                        prefix
                    };
                    let prompt = thread::launch_prompt(prefix, slug, &current);
                    thread::update(project, &t.id, |record| {
                        record.brief_submitted = true;
                        record.brief_submitted_at = project::now();
                        for follow_up in &mut record.follow_ups {
                            if follow_up.attempt == current.attempt.max(1)
                                && follow_up.carried_from_attempt > 0
                                && follow_up.state == thread::FollowUpState::Queued
                                && current.follow_ups.contains(follow_up)
                            {
                                follow_up.state = thread::FollowUpState::Uncertain;
                            }
                        }
                    })?;
                    match herdr.agent_prompt_wait_started(
                        &t.pane_id,
                        &prompt,
                        thread::agent_start_timeout(&current.launch)
                            .min(crate::herdr::AGENT_START_TIMEOUT.as_millis() as u64),
                    ) {
                        Ok(()) => {
                            delivered = true;
                            acknowledge_first_prompt(project, &current)?;
                        }
                        Err(error) if crate::threads::prompt_refused_before_submission(&error) => {
                            // Nothing was typed; registration can disappear between
                            // the list and the prompt. A later pass may submit it.
                            thread::update(project, &t.id, |record| {
                                if record.attempt == current.attempt
                                    && record.pane_id == current.pane_id
                                {
                                    record.brief_submitted = false;
                                    record.brief_submitted_at.clear();
                                    for follow_up in &mut record.follow_ups {
                                        if follow_up.attempt == current.attempt.max(1)
                                            && follow_up.carried_from_attempt > 0
                                            && follow_up.state == thread::FollowUpState::Uncertain
                                        {
                                            follow_up.state = thread::FollowUpState::Queued;
                                        }
                                    }
                                }
                            })?;
                        }
                        Err(error) => {
                            // A timeout may happen before the remote API ever
                            // receives the request. Retain the transport evidence
                            // while waiting for activity, not an infinite latch.
                            thread::update(project, &t.id, |record| {
                                if record.attempt == current.attempt
                                    && record.pane_id == current.pane_id
                                    && record.prompt_pending
                                {
                                    record.error = format!("brief_delivery_pending: {error}");
                                }
                            })?;
                            if !matches!(error.code.as_str(), "timeout" | "agent_prompt_stalled") {
                                pass.error = pass
                                    .error
                                    .or(Some(anyhow::anyhow!("{}: brief prompt: {error}", t.id)));
                            }
                        }
                    }
                }
            }
        } else if !t.prompt_pending
            && (t.kind == thread::Kind::Adopted || t.bootstrap == "acknowledged")
            && (ready || crate::threads::can_steer(t, &state))
        {
            loop {
                match crate::threads::send_lane_input(
                    ctx,
                    project,
                    &t.id,
                    None,
                    Some((herdr, agents, t)),
                ) {
                    Ok(crate::threads::PromptOutcome::Sent { .. }) => delivered = true,
                    Ok(crate::threads::PromptOutcome::Queued { .. }) => break,
                    Err(error) => {
                        pass.error = pass
                            .error
                            .or(Some(error.context(format!("{}: queued prompt", t.id))));
                        break;
                    }
                }
            }
        }

        // A report written this tick counts for the group at once; the copy
        // home follows. Otherwise a finished thread would show as Idle for one
        // tick before it shows as Ready for review.
        let fresh_hash = match hashes {
            Some(hashes) => hashes.get(&t.id).cloned(),
            None => thread::local_report_hash(t),
        };
        let current = thread::load(project, &t.id)?;
        let report_hash = fresh_hash.unwrap_or_else(|| current.report_hash.clone());
        let after = thread::Thread {
            report_hash,
            ..current
        };
        // In the tick that delivers a prompt the agent still reads as idle; it
        // has just been given work, so it is Working, not Idle.
        let group = if delivered {
            thread::Group::Working
        } else {
            thread::group(&after, &live, now)
        };
        // Observation failures do not advance a clock or manufacture a stall.
        if after.status == thread::Status::Open
            && group == thread::Group::Working
            && state == "working"
            && !delivered
        {
            let observation = if t.is_remote() {
                box_progress
                    .and_then(|progress| progress.get(&(slug.clone(), t.id.clone())))
                    .filter(|progress| progress.pane == t.pane_id)
                    .cloned()
            } else if live.pane_exists {
                herdr
                    .pane_read_text(&t.pane_id, "detection")
                    .ok()
                    .and_then(|screen| {
                        let head = if t.repo.is_empty() {
                            Some(String::new())
                        } else {
                            let folder = if t.worktree_path.is_empty() {
                                &t.cwd
                            } else {
                                &t.worktree_path
                            };
                            crate::repo::Git::new(ctx.runner, folder)
                                .with_timeout(Duration::from_secs(5))
                                .run(&["rev-parse", &format!("refs/heads/{}", t.branch)])
                                .ok()
                        }?;
                        Some(steps::LaneProgress {
                            pane: t.pane_id.clone(),
                            screen: thread::sha256_hex(screen.as_bytes()),
                            head,
                        })
                    })
            } else {
                None
            };
            if let Some(observation) = observation
                && (t.repo.is_empty() || !observation.head.is_empty())
            {
                let mut preview = after.clone();
                let due = progress_notices(&mut preview, &observation, now, &thresholds);
                if preview.progress_since != after.progress_since
                    || preview.progress_screen != after.progress_screen
                    || preview.progress_pane != after.progress_pane
                    || preview.progress_head != after.progress_head
                    || preview.no_commit_since != after.no_commit_since
                    || due.stalled.is_some()
                    || due.no_commit.is_some()
                {
                    let mut notices = ProgressNotices::default();
                    thread::update(project, &t.id, |record| {
                        if record.attempt == t.attempt && record.pane_id == t.pane_id {
                            notices = progress_notices(record, &observation, now, &thresholds);
                        }
                    })?;
                    for notice in progress_notice_lines(&notices, &t.id, &t.pane_id) {
                        let sent = if let Some(coordinator) = project.coordinator() {
                            agents.iter().any(|agent| {
                                coordinator::agent_matches(&coordinator, agent)
                                    && agent.promptable()
                            }) && steps::deliver_coordinator_prompt(
                                project,
                                herdr,
                                &coordinator.pane_id,
                                &notice,
                            )
                            .unwrap_or(false)
                        } else {
                            false
                        };
                        // A transport error must not consume the one-shot notice.
                        if !sent {
                            thread::update(project, &t.id, |record| {
                                record.start_notices.push(steps::Notice {
                                    line: notice,
                                    submitted: false,
                                });
                            })?;
                        }
                    }
                }
            }
        } else if t.status == thread::Status::Open
            && (state != "working" || group != thread::Group::Working)
            && !t.progress_since.is_empty()
        {
            thread::update(project, &t.id, reset_progress)?;
        }
        // A delivered correction may simply acknowledge the existing seal. Only
        // restore it after the agent has returned to idle and the sealed git
        // state has been checked on the lane's own machine.
        if !delivered
            && crate::herdr::ready_state(&state)
            && let Err(error) = restore_unchanged_seal(ctx, project, &after)
        {
            pass.error = pass
                .error
                .or(Some(error.context(format!("{}: seal check", t.id))));
        }
        if t.is_remote() || delivered || state != t.last_state || group.token() != t.last_group {
            thread::update(project, &t.id, |t| {
                if state != t.last_state {
                    t.last_state = state.clone();
                    t.last_state_change = project::now();
                }
                if t.is_remote() {
                    let observed = project::now();
                    t.last_observed = observed.clone();
                    t.observation_attempted = observed;
                    t.observation_source = "courier".into();
                    t.observation_error.clear();
                }
                if t.failure_class == crate::contracts::FailureClass::ProcessGone {
                    t.failure_class = crate::contracts::FailureClass::Unknown;
                    t.error.clear();
                }
                t.last_group = group.token().to_string();
            })?;
        }
        if live.pane_exists && (refresh_tokens || group.token() != t.last_group) {
            threads::report_thread_tokens(herdr, t, slug, group);
        }
    }
    Ok(pass)
}

fn brief_delivery_timeout(launch: &crate::contracts::Launch) -> u64 {
    if launch.ready_timeout_ms == 0 {
        thread::STARTING_TIMEOUT_SECS as u64 * 1000
    } else {
        launch.ready_timeout_ms
    }
}

struct LaunchPass<'a> {
    ctx: &'a Ctx<'a>,
    project: &'a Project,
    herdr: &'a Herdr<'a>,
    threads: &'a [thread::Thread],
    agents: &'a [Agent],
    panes: &'a [Pane],
}

/// Exercise the ordinary launch and delivery passes in startup regression tests.
#[cfg(test)]
pub(crate) fn launch_thread_now(ctx: &Ctx, project: &Project, id: &str) -> Result<()> {
    launch_thread_with_wait(ctx, project, id, Duration::from_secs(20))
}

#[cfg(test)]
pub(crate) fn launch_thread_with_wait(
    ctx: &Ctx,
    project: &Project,
    id: &str,
    wait: Duration,
) -> Result<()> {
    let lane = thread::load(project, id)?;
    if lane.status != thread::Status::Open || !lane.prompt_pending {
        return Ok(());
    }
    let socket = project
        .coordinator()
        .context("project has no coordinator")?
        .socket;
    let herdr = Herdr::new(ctx.env.herdr_bin(), &socket, ctx.runner);
    let remote = herdr.on_machine(lane.machine_route());
    let agents = remote.agent_list()?;
    let panes = remote.pane_list()?;
    if !panes.iter().any(|pane| {
        pane.pane_id == lane.pane_id
            && pane.tab_id == lane.tab_id
            && pane.workspace_id == lane.workspace_id
    }) {
        // A newly created tab may not appear in the server's list yet.
        // Leave its pending launch for the next observation.
        return Ok(());
    }
    let threads = [lane];
    let mut errors = Vec::new();
    launch_pass(
        &LaunchPass {
            ctx,
            project,
            herdr: &herdr,
            threads: &threads,
            agents: &agents,
            panes: &panes,
        },
        &mut true,
        false,
        &mut errors,
    );
    // A successful start normally returns a ready agent. Read fresh state and
    // deliver its brief before returning to the coordinator.
    let deadline = Instant::now() + wait;
    let prefix = coordinator::current_prefix(&ctx.root)?;
    loop {
        let current = thread::load(project, id)?;
        if current.status != thread::Status::Open
            || !current.prompt_pending
            || current.launch_attempts == 0
        {
            break;
        }
        let agents = remote.agent_list()?;
        let panes = remote.pane_list()?;
        let pass = thread_pass(
            &LaunchPass {
                ctx,
                project,
                herdr: &remote,
                threads: &[current],
                agents: &agents,
                panes: &panes,
            },
            &prefix,
            None,
            false,
            None,
        )?;
        if let Some(error) = pass.error {
            errors.push(error);
            break;
        }
        if !thread::load(project, id)?.prompt_pending || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    match errors.into_iter().next() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Launches pending threads whose pane is at a shell prompt. Local starts stay
/// one per pass; independent box starts are submitted as one parallel batch.
/// Prompts still go only to agents listed before this launch pass.
fn launch_pass(
    pass: &LaunchPass<'_>,
    may_start: &mut bool,
    one_at_a_time: bool,
    errors: &mut Vec<anyhow::Error>,
) -> bool {
    let now = jiff::Timestamp::now();
    let mut pending = Vec::new();
    for t in pass.threads {
        if t.status != thread::Status::Open
            || !t.prompt_pending
            || !t.provider_wait_started.is_empty()
        {
            continue;
        }
        // The preceding state pass may have failed this start while the
        // courier snapshot still describes the old, open record.
        if t.is_remote()
            && !thread::load(pass.project, &t.id)
                .is_ok_and(|fresh| fresh.status == thread::Status::Open)
        {
            continue;
        }
        let live = thread::live_state(t, pass.agents, pass.panes, now);
        if live.agent_state.is_some()
            || !live.pane_exists
            || (t.launch_attempts > 0 && !t.startup_wait_started.is_empty())
        {
            continue;
        }
        if t.launch_attempts >= thread::MAX_LAUNCH_ATTEMPTS {
            // The pane is listed but agent state is absent. Bounded launch
            // attempts stop here; no evidence says the process is gone, and a
            // late agent registration can still receive the pending brief.
            continue;
        }
        if one_at_a_time && !*may_start {
            continue;
        }
        // A deferred start must satisfy today's plan before agent submission.
        if let Err(error) = crate::plan::check_attempt_prerequisites(pass.project, &t.id) {
            if !format!("{error:#}").starts_with("plan_prerequisite:") {
                errors.push(error.context(format!("{}: plan gate", t.id)));
            }
            continue;
        }
        // Disk can fill after placement. Keep the attempt queued until it recovers.
        let disk = if t.is_remote() {
            crate::remote::machine_profile(
                pass.ctx.runner,
                &pass.ctx.env.herdr_bin(),
                &pass.ctx.config_dir,
                t.machine_route(),
            )
            .and_then(|profile| {
                crate::doctor::check_start_disk(pass.ctx, Some(&profile), Some(&t.repo))
            })
        } else {
            crate::doctor::check_start_disk(pass.ctx, None, Some(&t.repo))
        };
        #[cfg(test)]
        let disk = disk.or_else(|error| {
            if !pass.ctx.runner.is_real() && format!("{error:#}").contains("no rule for `") {
                Ok(())
            } else {
                Err(error)
            }
        });
        if let Err(error) = disk {
            let message = format!("{error:#}");
            if crate::remote::is_unreachable(&message) {
                errors.push(error);
                continue;
            }
            errors.extend(
                thread::update(pass.project, &t.id, |record| {
                    if message.starts_with("disk_low:") {
                        record.startup_wait_started.clear();
                    }
                    record.error = message.clone();
                })
                .err(),
            );
            continue;
        }
        // Provider-bridge credentials can expire between placement and start.
        // The adapter's readiness driver, not its agent-kind name, chooses the
        // extra check; command probes were already run during placement.
        let readiness = if t.launch.kind.is_empty() || t.error == "provider ready" {
            Ok(())
        } else {
            crate::adapters::declaration(&pass.ctx.config_dir, &t.launch.kind).and_then(|adapter| {
                if adapter.doctor.readiness != "pi" {
                    return Ok(());
                }
                if t.is_remote() {
                    crate::threads::box_launch_ready_for(pass.ctx, t.machine_route(), &t.launch)
                } else {
                    crate::doctor::recipe_ready_local(pass.ctx, &t.launch)
                }
            })
        };
        if let Err(error) = readiness {
            let message = format!("{error:#}");
            if crate::remote::is_unreachable(&message) {
                errors.push(error);
                continue;
            }
            if message.contains("pi_not_ready") {
                errors.extend(
                    thread::update(pass.project, &t.id, |record| {
                        if record.provider_wait_started.is_empty() {
                            record.provider_wait_started = project::now();
                        }
                        record.error = format!("waiting for provider: {message}");
                    })
                    .err(),
                );
            } else {
                let class = crate::pi_ade::failure_class(&error);
                errors.extend(
                    threads::fail_start(pass.ctx, pass.project, &t.id, &message, class, true)
                        .err()
                        .map(|cleanup| cleanup.context(format!("{}: failed-start cleanup", t.id))),
                );
            }
            continue;
        }
        let launch_record = match threads::resume_launch_record(pass.ctx, pass.project, t) {
            Ok(record) => record,
            Err(error) => {
                errors.push(error.context(format!("{}: resume session", t.id)));
                continue;
            }
        };
        // The CLI and the ticker can race on the same newly placed pane.
        // Claim the launch under the record lock before issuing `agent start`.
        let mut claimed = false;
        match thread::update_checked(pass.project, &t.id, |current| {
            if current.attempt != t.attempt
                || current.pane_id != t.pane_id
                || current.status != thread::Status::Open
                || !current.prompt_pending
                || (current.launch_attempts > 0 && !current.startup_wait_started.is_empty())
            {
                return Ok(());
            }
            current.launch_attempts += 1;
            if current.error == "provider ready" || current.error.starts_with("disk_low:") {
                current.error.clear();
            }
            current.trust_answered = false;
            // Placement can wait on the courier or readiness. The agent gets
            // its full ready window only when its launch is submitted.
            current.startup_wait_started = project::now();
            claimed = true;
            Ok(())
        }) {
            Ok(_) if claimed => {
                pending.push(launch_record);
                if one_at_a_time {
                    *may_start = false;
                    break;
                }
            }
            Ok(_) => {}
            Err(error) => errors.push(error.context(format!("{}: launch record", t.id))),
        }
    }
    let Some(first) = pending.first() else {
        return false;
    };
    let parent = pass
        .project
        .coordinator()
        .filter(|_| !first.is_remote())
        .map(|coordinator| coordinator.pane_id);
    let args: Vec<_> = pending
        .iter()
        .map(|t| {
            if t.bootstrap == "resuming" {
                crate::adapters::resume_args(&t.launch, t.identity.agent_session.as_deref())
                    .unwrap_or_else(|| t.launch.args.clone())
            } else {
                t.launch.args.clone()
            }
        })
        .collect();
    let starts: Vec<_> = pending
        .iter()
        .zip(&args)
        .map(|(t, args)| crate::herdr::AgentStart {
            name: &t.agent_name,
            kind: &t.launch.kind,
            pane: &t.pane_id,
            agent_args: args,
            launch_bin: None,
            parent: parent.as_deref(),
            // `agent start` need not hold the ticker for the whole observation
            // window: subsequent passes watch the pane for the remaining time.
            ready_timeout_ms: thread::agent_start_timeout(&t.launch)
                .min(crate::herdr::AGENT_START_TIMEOUT.as_millis() as u64),
        })
        .collect();
    let herdr = pass.herdr.on_machine(first.machine_route());
    let outcomes = herdr.agent_start_many(&starts);
    let socket = pass
        .project
        .coordinator()
        .map(|coordinator| coordinator.socket)
        .unwrap_or_default();
    for (t, outcome) in pending.into_iter().zip(outcomes) {
        let launched = outcome.map_err(anyhow::Error::from).and_then(|agent| {
            let process = herdr
                .pane_process_info(&t.pane_id)
                .ok()
                .and_then(|info| info.identity(&t.launch.kind));
            thread::update(pass.project, &t.id, |record| {
                let mut bound = agent.clone();
                if bound.workspace_id.is_empty() {
                    bound.workspace_id = record.workspace_id.clone();
                }
                if bound.tab_id.is_empty() {
                    bound.tab_id = record.tab_id.clone();
                }
                if bound.pane_id.is_empty() {
                    bound.pane_id = record.pane_id.clone();
                }
                if bound.cwd.is_empty() {
                    bound.cwd = record.cwd.clone();
                }
                record.workspace_id = bound.workspace_id.clone();
                record.tab_id = bound.tab_id.clone();
                record.pane_id = bound.pane_id.clone();
                record.cwd = bound.cwd.clone();
                // Keep the launch claim until the ready pass observes the agent.
                // Registration can precede exec, especially on a remote box.
                thread::bind_identity(record, &socket, &bound, process);
            })?;
            Ok(())
        });
        if let Err(error) = launched {
            if crate::remote::is_unreachable(&format!("{error:#}")) {
                // Undo only this submission's claim. A lost connection is
                // not evidence of a failed launch and spends no retry.
                errors.extend(
                    thread::update(pass.project, &t.id, |record| {
                        record.launch_attempts = t.launch_attempts;
                        record.startup_wait_started = t.startup_wait_started.clone();
                        record.trust_answered = t.trust_answered;
                        record.error = t.error.clone();
                    })
                    .err(),
                );
                errors.push(error);
                continue;
            }
            if error.to_string().contains("agent_not_ready")
                || error.to_string().contains("timeout")
            {
                if let Err(e) =
                    thread::update(pass.project, &t.id, |t| t.status = thread::Status::Starting)
                {
                    errors.push(e.context(format!("{}: startup record", t.id)));
                }
                // This can clear by itself. The pane and pending brief remain
                // bound until the recipe's ready window has elapsed.
                continue;
            }
            errors.extend(
                thread::update(pass.project, &t.id, |t| t.startup_wait_started.clear())
                    .err()
                    .map(|e| e.context(format!("{}: launch record", t.id))),
            );
            errors.extend(
                threads::fail_start(
                    pass.ctx,
                    pass.project,
                    &t.id,
                    &format!("{error:#}"),
                    crate::contracts::FailureClass::Unknown,
                    false,
                )
                .err(),
            );
            errors.push(error.context(format!("{}: launch", t.id)));
        }
    }
    true
}

fn open_threads(project: &Project, remote: bool) -> Vec<thread::Thread> {
    thread::list_live(project)
        .into_iter()
        .filter(|t| {
            t.is_remote() == remote
                && !t.parked
                && (matches!(t.status, thread::Status::Open | thread::Status::Starting)
                    || (t.status == thread::Status::Failed
                        && (!remote || t.error.starts_with("brief_delivery_failed:"))))
        })
        .collect()
}

pub(crate) fn socket_inode(path: &std::path::Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map_or(0, |meta| meta.ino())
}

/// No binding or an empty socket is a closed coordinator, not a lost session.
/// Read strictly: an unreadable binding is still an observation gap.
pub(crate) fn coordinator_binding(project: &Project) -> Result<Option<project::Coordinator>> {
    let path = project.state_dir().join("coordinator.json");
    match read_evidence::<project::Coordinator>(&path) {
        Ok(record) if record.socket.is_empty() => Ok(None),
        Ok(record) => Ok(Some(record)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Returns `Ok(None)` for a closed or unreachable session. Without a live
/// observation, nothing is ever reported as gone.
#[cfg(test)]
fn tick_cheap(ctx: &Ctx, project: &Project, refresh_tokens: bool) -> Result<Option<Seen>> {
    tick_cheap_observed(ctx, project, refresh_tokens, &mut |_| {})
}

fn tick_cheap_observed(
    ctx: &Ctx,
    project: &Project,
    refresh_tokens: bool,
    unavailable: &mut impl FnMut(String),
) -> Result<Option<Seen>> {
    let binding = project.coordinator_lock()?;
    let Some(record) = coordinator_binding(project)? else {
        return Ok(None);
    };
    if !Path::new(&record.socket).exists() {
        unavailable(format!(
            "{}: session unavailable: coordinator socket {} is absent",
            project.dir().display(),
            record.socket
        ));
        return Ok(None);
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, ctx.runner);
    let agents = match herdr.agent_list() {
        Ok(agents) => agents,
        Err(error) => {
            unavailable(format!(
                "session unavailable: {}: agent list: {error:#}",
                record.socket
            ));
            return Ok(None);
        }
    };
    let panes = match herdr.pane_list() {
        Ok(panes) => panes,
        Err(error) => {
            unavailable(format!(
                "session unavailable: {}: pane list: {error:#}",
                record.socket
            ));
            return Ok(None);
        }
    };
    let slug = &project.slug;
    let prefix = coordinator::current_prefix(&ctx.root)?;
    let mut first_error = None;
    // A replacement binding must carry its existing lanes with it. Pickup
    // checks agent identity on each machine before touching parent metadata.
    let state = steps::load_state(project);
    if !state.lanes_parented_to.is_empty() && state.lanes_parented_to != record.pane_id {
        match crate::threads::relink_binding(ctx, project, &record.pane_id) {
            Ok(()) => {
                let mut state = steps::load_state(project);
                state.lanes_parented_to = record.pane_id.clone();
                steps::save_state(project, &state)?;
            }
            Err(error) => eprintln!(
                "{}: re-link coordinator lanes will retry: {error:#}",
                project.slug
            ),
        }
    }

    // The recorded name can be gone while the agent keeps running in the
    // bound pane: `agent start` drops it when interactive readiness times out,
    // and a live server handoff drops it on respawn. Put it back, so the
    // coordinator can be woken, and record the repair in its own record.
    let agent = match coordinator::restore_agent_name(project, &herdr, &record, &agents) {
        Ok(agent) => agent,
        Err(error) => {
            first_error = Some(error);
            agents
                .iter()
                .find(|a| coordinator::agent_matches(&record, a))
                .cloned()
        }
    };

    // The coordinator: deliver a pending priming prompt, refresh its tokens.
    if let Some(agent) = &agent {
        if record.last_agent_seen_at.is_empty() {
            project.update_coordinator(|c| c.last_agent_seen_at = project::now())?;
        }
        // One priming line per binding. Transport is not the receipt: an ADE
        // binding clears `prime_pending` only on its `ha context` receipt, and
        // a submitted line is never re-sent on a timer (SPEC-ADE D14).
        if record.prime_pending && !record.prime_sent && agent.ready() {
            let prompt = if record.launch_attempts > 1 {
                format!(
                    "Run `{prefix} context {slug}` and continue the task you were working on; do not repeat completed work."
                )
            } else {
                coordinator::priming_prompt(&prefix, slug)
            };
            if let Err(error) =
                coordinator::deliver_or_defer(project, &herdr, &record, agent, &prompt, false)
            {
                first_error = Some(error.context("priming prompt"));
            }
        }
        if refresh_tokens {
            coordinator::report_tokens(&herdr, slug, &record.pane_id);
        }
    }

    let pane_alive = panes.iter().any(|p| coordinator::pane_matches(&record, p));
    let inode = socket_inode(Path::new(&record.socket));
    if pane_alive && inode != 0 && record.server_socket_inode != inode {
        project.update_coordinator(|c| c.server_socket_inode = inode)?;
    }
    drop(binding);

    // A shared incident can originate in another lane while this coordinator
    // has no terminal error. Recheck it even when its goal is already queued
    // or disposed; otherwise the notice outbox has no path out of the hold.
    if record.closed_by_rolf_at.is_empty()
        && agent.as_ref().is_some_and(Agent::promptable)
        && crate::adapters::dependency_waiting(
            &ctx.root,
            crate::contracts::MACHINE_LOCAL,
            &record.launch,
        )
    {
        let _ = crate::doctor::recipe_ready_local(ctx, &record.launch);
        if let Err(error) = crate::adapters::notify_auth(
            &ctx.root,
            project,
            crate::contracts::MACHINE_LOCAL,
            &record.launch,
        ) {
            first_error = first_error.or(Some(error));
        }
    }

    if let Err(error) = crate::threads::retry_pending_cleanup(ctx, project) {
        eprintln!("note: pending cleanup will retry: {error:#}");
    }
    crate::threads::resolve_report_only(ctx, project);

    if let Err(error) = steps::flush_coordinator_notices(ctx, project) {
        first_error = first_error.or(Some(error));
    }
    let local = open_threads(project, false);
    let pass = thread_pass(
        &LaunchPass {
            ctx,
            project,
            herdr: &herdr,
            threads: &local,
            agents: &agents,
            panes: &panes,
        },
        &prefix,
        None,
        refresh_tokens,
        None,
    )?;
    first_error = first_error.or(pass.error);
    if let Err(error) = crate::threads::tick(project, &herdr, &agents) {
        first_error = first_error.or(Some(error));
    }
    // The ops pass (A2) and the reviews pass (A3) run in the slow pass,
    // outside the project lock (SPEC-ADE item 57).
    // A reused pane id after a server restart is not the original process.
    let bound_pane =
        pane_alive && (record.server_socket_inode == 0 || record.server_socket_inode == inode);
    if let Err(error) =
        coordinator::recover(ctx, project, &herdr, &record, agent.as_ref(), bound_pane)
    {
        first_error = first_error.or(Some(error));
    }
    let coordinator_recorded = usize::from(!record.pane_id.is_empty());
    let coordinator_missing = usize::from(
        coordinator_recorded == 1
            && agent.is_none()
            && !panes.iter().any(|p| coordinator::pane_matches(&record, p)),
    );
    let recorded_panes = pass.recorded_panes + coordinator_recorded;
    let missing_panes = pass.missing_panes + coordinator_missing;

    // Announce unseen inbox items without typing into the coordinator pane.
    let mut state = steps::load_state(project);
    let before = state.clone();
    if let Err(error) = steps::announce_inbox(project, &mut state, &herdr) {
        first_error = first_error.or(Some(error.context("inbox announcement")));
    }
    if state != before {
        steps::save_state(project, &state)?;
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(Some(Seen {
            socket: record.socket,
            agents,
            panes,
            session_lost: recorded_panes >= 2 && missing_panes == recorded_panes,
        })),
    }
}

/// One remote machine: the courier's one helper call already read the box's
/// live `agent list`/`pane list`, so this pass uses that view instead of a
/// second `herdr --machine` bridge. It then runs the same thread pass and
/// launches as for local threads. The report bytes and sealed events arrived
/// through the courier; this pass never reads a report hash or copies a file.
fn remote_pass(
    pass: &LaunchPass<'_>,
    machine: &str,
    view: &steps::CourierOutcome,
    may_start: &mut bool,
    errors: &mut Vec<anyhow::Error>,
) -> Result<(), String> {
    let ctx = pass.ctx;
    let project = pass.project;
    let herdr = pass.herdr;
    let remote = herdr.on_machine(machine);
    let (agents, panes) = match (&view.agents, &view.panes) {
        (Some(agents), Some(panes)) => (agents.clone(), panes.clone()),
        // The box server did not answer: the sealed events were still imported,
        // but no lane state changes and no GONE is invented (SPEC-remote §4.3).
        _ => return Ok(()),
    };

    let threads = pass.threads;
    let prefix = coordinator::current_prefix(&ctx.root).map_err(|e| format!("{e:#}"))?;
    let state_input = LaunchPass {
        ctx,
        project,
        herdr: &remote,
        threads,
        agents: &agents,
        panes: &panes,
    };
    let state_pass = thread_pass_observed(
        &state_input,
        &prefix,
        None,
        true,
        Some(&view.progress),
        Some(ObservationView {
            machine_id: &view.machine_id,
            threads,
            agents: &agents,
            panes: &panes,
            boot_id: &view.boot_id,
            now: jiff::Timestamp::now(),
        }),
    )
    .map_err(|e| format!("{e:#}"))?;
    errors.extend(state_pass.error);
    let launched = launch_pass(
        &LaunchPass {
            ctx,
            project,
            herdr,
            threads,
            agents: &agents,
            panes: &panes,
        },
        may_start,
        false,
        errors,
    );
    if launched {
        errors.extend(request_remote_poll(&ctx.root, project, machine).err());
    }
    errors.extend(clean_managed_project_tabs(
        project, machine, &remote, &agents, &panes,
    ));
    Ok(())
}

/// Reconcile live courier views independently of coordinator availability.
fn tick_remote_with_steps(
    ctx: &Ctx,
    project: &Project,
    herdr: &Herdr<'_>,
    memory: &Memory,
    may_start: &mut bool,
    errors: &mut Vec<anyhow::Error>,
    step: &mut impl FnMut(&str) -> bool,
) -> bool {
    let remote_threads = open_threads(project, true);
    let machines: BTreeSet<_> = remote_threads
        .iter()
        .map(|t| t.machine_route().to_string())
        .collect();
    for machine in machines {
        if !step(&format!("remote state {machine}")) {
            return false;
        }
        let Some(view) = memory.machine_views.get(&machine) else {
            continue;
        };
        let view = match view {
            Ok(view) => view,
            Err(error) => {
                errors.push(anyhow::anyhow!("{machine}: unreachable this tick: {error}"));
                continue;
            }
        };
        let threads: Vec<_> = remote_threads
            .iter()
            .filter(|t| t.machine_route() == machine)
            .cloned()
            .collect();
        if let Err(error) = remote_pass(
            &LaunchPass {
                ctx,
                project,
                herdr,
                threads: &threads,
                agents: &[],
                panes: &[],
            },
            &machine,
            view,
            may_start,
            errors,
        ) {
            errors.push(anyhow::anyhow!("{machine}: {error}"));
        }
    }
    true
}

/// Closes only a tab the ADE can prove it created and no process still owns.
/// An unowned shell in a project-labelled workspace is advisory evidence, not
/// authority to destroy someone else's foreground work.
fn clean_managed_project_tabs(
    project: &Project,
    machine: &str,
    herdr: &Herdr<'_>,
    agents: &[Agent],
    panes: &[Pane],
) -> Vec<anyhow::Error> {
    thread::list(project)
        .into_iter()
        .filter(|record| {
            record.is_remote()
                && record.machine_route() == machine
                && record.kind != thread::Kind::Adopted
                && !matches!(
                    record.status,
                    thread::Status::Starting | thread::Status::Open
                )
                && !agents.iter().any(|agent| agent.tab_id == record.tab_id)
                && {
                    let tab_panes: Vec<_> = panes
                        .iter()
                        .filter(|pane| pane.tab_id == record.tab_id)
                        .collect();
                    tab_panes.len() == 1 && thread::pane_matches(record, tab_panes[0])
                }
        })
        .filter(|record| {
            herdr.pane_process_info(&record.pane_id).is_ok_and(|info| {
                info.pane_id == record.pane_id && info.foreground_processes.is_empty()
            })
        })
        .filter_map(|record| {
            herdr
                .pane_clear_tokens(&record.pane_id, &["parent"])
                .and_then(|()| herdr.tab_close(&record.tab_id))
                .err()
                .map(|error| {
                    anyhow::anyhow!("{machine}: close managed tab {}: {error}", record.tab_id)
                })
        })
        .collect()
}

/// Reconcile the outcome obligation independently of transport and running work.
fn goal_check_nudge(
    ctx: &Ctx,
    project: &Project,
    herdr: &Herdr<'_>,
    agents: &[Agent],
) -> Result<()> {
    let coordinator = project.coordinator();
    let agent = coordinator
        .as_ref()
        .and_then(|c| agents.iter().find(|a| coordinator::agent_matches(c, a)));
    steps::goal_check::reconcile(project, agent, jiff::Timestamp::now().as_second() as u64)?;
    let Some(coordinator) = coordinator else {
        return Ok(());
    };
    if !coordinator.closed_by_rolf_at.is_empty()
        || coordinator.prime_pending
        || coordinator::paused_by_provider(project, &coordinator)
        || !agent.is_some_and(Agent::ready)
    {
        return Ok(());
    }
    if let Some((token, line)) = steps::goal_check::notice(project) {
        if crate::adapters::dependency_waiting(
            &ctx.root,
            crate::contracts::MACHINE_LOCAL,
            &coordinator.launch,
        ) {
            let ready = crate::doctor::recipe_ready_local(ctx, &coordinator.launch);
            crate::adapters::notify_auth(
                &ctx.root,
                project,
                crate::contracts::MACHINE_LOCAL,
                &coordinator.launch,
            )?;
            if ready.is_err() {
                return Ok(());
            }
        }
        steps::deliver_goal_check(project, herdr, &coordinator.pane_id, &token, &line)?;
    }
    Ok(())
}

/// Copies and launches, remote machines, then inbox items,
/// housekeeping.
#[cfg(test)]
fn tick_slow(ctx: &Ctx, project: &Project, seen: &Seen, memory: &mut Memory) -> Vec<anyhow::Error> {
    tick_slow_with_steps(ctx, project, seen, memory, &mut |_| true).0
}

// Each boundary prevents a stop received during a slow operation from
// starting the next independent operation in the same project's pass.
fn tick_slow_with_steps(
    ctx: &Ctx,
    project: &Project,
    seen: &Seen,
    memory: &mut Memory,
    step: &mut impl FnMut(&str) -> bool,
) -> (Vec<anyhow::Error>, bool) {
    let mut errors = Vec::new();
    if !step("recovery") {
        return (errors, false);
    }
    errors.extend(crate::recovery::tick(ctx, project).err());
    let herdr = Herdr::new(ctx.env.herdr_bin(), &seen.socket, ctx.runner);
    let mut may_start = true;

    // Local threads: copy home when the report changed, then launches.
    let local = open_threads(project, false);
    for t in local.iter().filter(|t| t.status == thread::Status::Open) {
        if !step(&format!("local report {}", t.id)) {
            return (errors, false);
        }
        if let Some(hash) = thread::local_report_hash(t)
            && hash != t.report_hash
        {
            let copied = thread::copy_home_local(project, t, true, ctx.runner);
            match copied.outcome {
                thread::CopyOutcome::Failed(error) => {
                    errors.push(anyhow::anyhow!("{}: copy failed: {error}", t.id))
                }
                outcome => {
                    let notes = match outcome {
                        thread::CopyOutcome::Partial(notes) => notes,
                        _ => Vec::new(),
                    };
                    let updated = thread::update(project, &t.id, |t| {
                        t.copy_notes = notes;
                        t.report_hash = hash.clone();
                        t.last_report_change = project::now();
                    });
                    errors.extend(updated.err());
                }
            }
        }
    }
    if !step("local launches") {
        return (errors, false);
    }
    launch_pass(
        &LaunchPass {
            ctx,
            project,
            herdr: &herdr,
            threads: &local,
            agents: &seen.agents,
            panes: &seen.panes,
        },
        &mut may_start,
        true,
        &mut errors,
    );

    // Remote threads, one machine at a time, from this tick's courier views.
    // A machine with no view was not due this tick (SPEC-remote §4.3).
    let mut state = steps::load_state(project);
    let before = state.clone();
    // A stop after a stateful slow stage must still persist that stage's
    // completed work; otherwise the next ticker replays it.
    macro_rules! stop_after_state {
        ($name:expr) => {
            if !step($name) {
                if state != before {
                    errors.extend(steps::save_state(project, &state).err());
                }
                return (errors, false);
            }
        };
    }
    if !tick_remote_with_steps(
        ctx,
        project,
        &herdr,
        memory,
        &mut may_start,
        &mut errors,
        step,
    ) {
        return (errors, false);
    }

    stop_after_state!("session notice");
    errors.extend(steps::session_notice(project, &mut state, seen.session_lost).err());
    // D5 recovery and delivery (X1 to X5).
    // Reviews ran immediately after courier import. Each takes the project
    // lock only for its own file writes.
    stop_after_state!("ops");
    errors.extend(
        crate::ops::tick(ctx, project)
            .err()
            .map(|e| e.context("ops")),
    );
    errors.extend(crate::threads::park_completed(ctx, project).err());
    stop_after_state!("goal check");
    errors.extend(goal_check_nudge(ctx, project, &herdr, &seen.agents).err());
    inbox::prune_done(project, steps::DONE_RETENTION_DAYS);
    if state != before {
        errors.extend(steps::save_state(project, &state).err());
    }
    (errors, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_coordinators_have_one_status_without_recurring_errors() {
        for missing_binding in [false, true] {
            let world = crate::scenarios::World::new();
            let project = world.project("demo", "a.sock");
            let lane = thread::allocate(&project, |lane| {
                lane.status = thread::Status::Open;
                lane.repo = "/repo".into();
            })
            .unwrap();
            // This is the noisy case: automatic review is enabled and a
            // changed lane is ready, but Rolf has not opened its coordinator.
            project::write_atomic(&project.state_dir().join("reviews-enabled"), b"enabled\n")
                .unwrap();
            crate::events::seal_create_if_absent(
                &project,
                &crate::contracts::Event {
                    id: format!("{}-1-1", lane.id),
                    op: format!("{}-1-1", lane.id),
                    thread: lane.id.clone(),
                    attempt: 1,
                    recipient: Default::default(),
                    created: project::now(),
                    usage: None,
                    payload: crate::contracts::EventPayload {
                        done: Some(crate::contracts::DonePayload {
                            has_changes: Some(true),
                            sha: "sealed-sha".into(),
                            artifact: thread::store_artifact(&project, b"sealed report").unwrap(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                },
            )
            .unwrap();
            if missing_binding {
                std::fs::remove_file(project.state_dir().join("coordinator.json")).unwrap();
            } else {
                project
                    .update_coordinator(|record| record.socket.clear())
                    .unwrap();
            }
            let ctx = world.ctx();
            let mut memory = Memory::new(&ctx);
            let log = Log {
                path: log_path(&world.root),
            };
            for _ in 0..3 {
                assert!(!tick(&ctx, &log, &mut memory));
                let health: Health = read_evidence(&health_path(&world.root)).unwrap();
                assert!(health.failures.is_empty(), "{:?}", health.failures);
                assert_eq!(health.status.len(), 1);
                let (healthy, detail) = health_report(&world.root);
                assert_ne!(healthy, Some(false), "{detail}");
                assert_eq!(detail.matches("coordinator closed").count(), 1);
                assert!(detail.contains("reviews skipped"));
                assert_eq!(
                    thread::load(&project, &lane.id).unwrap().status,
                    thread::Status::Open
                );
            }
            let log = std::fs::read_to_string(log_path(&world.root)).unwrap();
            assert_eq!(
                log.matches("observation state changed:").count(),
                1,
                "{log}"
            );
            assert!(!log.contains("session unavailable"), "{log}");
            assert_eq!(world.runner.count("agent list"), 0);
            assert_eq!(world.runner.count("pane list"), 0);
            assert_eq!(world.runner.count("git"), 0);
            assert!(crate::review::list(&project).unwrap().is_empty());
            assert!(
                crate::events::latest_done_event(
                    &crate::events::for_thread(&project, &lane.id),
                    &lane.id,
                    1
                )
                .is_some()
            );

            // Closing the coordinator does not hide unreadable lane records.
            let path = thread::threads_dir(&project).join(format!("{}.toml", lane.id));
            project::write_atomic(&path, b"bad = [").unwrap();
            tick_for_test(&ctx, &mut memory);
            let health: Health = read_evidence(&health_path(&world.root)).unwrap();
            assert!(
                health
                    .failures
                    .iter()
                    .any(|failure| failure.contains(&path.display().to_string()))
            );
        }
    }

    #[test]
    fn closed_coordinators_still_reconcile_live_remote_lanes() {
        use crate::scenarios::{World, agent_json, pane_json};
        for missing_binding in [false, true] {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            let lane = world.thread(&project, &world.home.path().join("lane"), |record| {
                record.machine = "box".into();
                record.machine_id = "box".into();
                record.prompt_pending = false;
                record.bootstrap = "acknowledged".into();
                record.last_state = "idle".into();
            });
            if missing_binding {
                std::fs::remove_file(project.state_dir().join("coordinator.json")).unwrap();
            } else {
                project
                    .update_coordinator(|record| record.socket.clear())
                    .unwrap();
            }
            let agent = agent_json(
                &lane.workspace_id,
                &lane.tab_id,
                &lane.pane_id,
                &lane.cwd,
                &lane.agent_name,
                "working",
            );
            let pane = pane_json(&lane.workspace_id, &lane.tab_id, &lane.pane_id, &lane.cwd);
            let manifest = crate::box_helper::tests::ready(crate::steps::CourierManifest {
                boot_id: "boot-1".into(),
                agents: Some(serde_json::from_str(&format!("[{agent}]")).unwrap()),
                panes: Some(serde_json::from_str(&format!("[{pane}]")).unwrap()),
                ..Default::default()
            });
            world
                .runner
                .on_fn(|cmd| cmd.program == "ssh", move |_| Ok(ok(&manifest)));
            world.runner.on(
                "machine list --json",
                ok(r#"[{"id":"box","label":"box","target":"box","session":"default","enabled":true}]"#),
            );
            let ctx = world.ctx();
            let mut memory = Memory::new(&ctx);
            let log = Log {
                path: log_path(&world.root),
            };
            assert!(tick(&ctx, &log, &mut memory));
            let current = thread::load(&project, &lane.id).unwrap();
            assert_eq!(current.last_state, "working");
            assert_eq!(current.last_group, "working");
            assert_eq!(current.observation_source, "courier");
            assert!(!current.last_observed.is_empty());
            assert!(current.observation_error.is_empty());
            let health: Health = read_evidence(&health_path(&world.root)).unwrap();
            assert!(health.failures.is_empty(), "{:?}", health.failures);
            assert_eq!(health.status.len(), 1);
            assert!(coordinator_binding(&project).unwrap().is_none());
            assert!(crate::review::list(&project).unwrap().is_empty());
            assert_eq!(world.runner.count("agent start"), 0);
        }
    }

    #[test]
    fn observation_state_changes_log_once_including_recovery_and_restart() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let socket = project.coordinator().unwrap().socket;
        std::fs::remove_file(&socket).unwrap();
        project
            .update_coordinator(|record| record.socket.clear())
            .unwrap();
        let ctx = world.ctx();
        let mut memory = Memory::new(&ctx);
        let log = Log {
            path: log_path(&world.root),
        };
        for (state, expected) in [("closed", 1), ("missing", 2), ("closed", 3)] {
            project
                .update_coordinator(|record| {
                    record.socket = if state == "missing" {
                        socket.clone()
                    } else {
                        String::new()
                    };
                })
                .unwrap();
            for _ in 0..2 {
                tick(&ctx, &log, &mut memory);
                // Deduplication is durable, not dependent on in-memory cadence.
                memory = Memory::new(&ctx);
                let log = std::fs::read_to_string(log_path(&world.root)).unwrap();
                assert_eq!(
                    log.matches("observation state changed:").count(),
                    expected,
                    "{log}"
                );
                let health: Health = read_evidence(&health_path(&world.root)).unwrap();
                assert_eq!(health.failures.is_empty(), state == "closed");
                if state == "missing" {
                    assert_eq!(health_report(&world.root).0, Some(false));
                    assert!(
                        health
                            .failures
                            .iter()
                            .any(|failure| failure.contains(&socket))
                    );
                }
            }
        }
    }

    #[test]
    fn partial_observation_keeps_health_loud_and_logs_once_after_completion() {
        let root = tempfile::tempdir().unwrap();
        let log = Log {
            path: log_path(root.path()),
        };
        // Historical health has no status or log snapshot.
        project::write_atomic(
            &health_path(root.path()),
            br#"{"observed":"old","projects":1,"failures":[]}"#,
        )
        .unwrap();
        let mut health = Health::begin(root.path(), 1);
        health.failure(root.path(), &log, "demo: unreadable binding".into());
        assert_eq!(health_report(root.path()).0, Some(false));
        drop(health); // A stop/restart before pass completion must not lose the log transition.
        for _ in 0..2 {
            let mut health = Health::begin(root.path(), 1);
            health.failure(root.path(), &log, "demo: unreadable binding".into());
            health.publish(root.path(), &log);
        }
        let text = std::fs::read_to_string(log_path(root.path())).unwrap();
        assert_eq!(
            text.matches("observation state changed:").count(),
            1,
            "{text}"
        );
        Health::begin(root.path(), 1).publish(root.path(), &log);
        let text = std::fs::read_to_string(log_path(root.path())).unwrap();
        assert_eq!(
            text.matches("observation state changed:").count(),
            2,
            "{text}"
        );
        assert!(text.contains("cleared: demo: unreadable binding"));
    }

    #[test]
    fn unreadable_coordinator_bindings_are_not_closed() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let path = project.state_dir().join("coordinator.json");
        std::fs::write(&path, "{bad").unwrap();
        let ctx = world.ctx();
        let mut memory = Memory::new(&ctx);
        tick_for_test(&ctx, &mut memory);
        let health: Health = read_evidence(&health_path(&world.root)).unwrap();
        assert!(health.status.is_empty());
        assert_eq!(health.failures.len(), 1);
        assert!(health.failures[0].contains(&path.display().to_string()));
        assert_eq!(health_report(&world.root).0, Some(false));
    }

    #[test]
    fn missing_observations_name_the_path_while_another_project_advances() {
        use crate::scenarios::{World, agent_json};
        for fault in ["discovery", "load", "directory", "record"] {
            let world = World::new();
            let healthy = world.project("healthy", "a.sock");
            let broken = world.project("broken", "b.sock");
            *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&healthy));
            *world.agents.borrow_mut() = format!(
                "[{}]",
                agent_json(
                    "w1",
                    "w1:t1",
                    "w1:p1",
                    &healthy.canonical_dir().to_string_lossy(),
                    "hp-healthy-coordinator",
                    "idle"
                )
            );
            let path = match fault {
                "discovery" | "load" => broken.project_md(),
                "directory" => thread::threads_dir(&broken),
                _ => thread::threads_dir_for_write(&broken)
                    .unwrap()
                    .join("t-0001.toml"),
            };
            match fault {
                "discovery" => {
                    std::fs::remove_file(&path).unwrap();
                    // ELOOP is deterministic, including when tests run as root.
                    std::os::unix::fs::symlink("PROJECT.md", &path).unwrap();
                }
                "directory" => std::fs::write(&path, "not a directory").unwrap(),
                "record" => std::fs::write(&path, "not = [valid TOML").unwrap(),
                _ => {}
            }
            let ctx = world.ctx();
            let log = Log {
                path: log_path(&world.root),
            };
            let mut memory = Memory::new(&ctx);
            let mut injected = false;
            assert_eq!(
                tick_with_steps(&ctx, &log, &mut memory, &mut |step| {
                    if fault == "load" && step == "cheap project broken" && !injected {
                        std::fs::remove_file(&path).unwrap();
                        std::fs::create_dir(&path).unwrap();
                        injected = true;
                    }
                    true
                }),
                Some(true)
            );
            assert!(
                !healthy.coordinator().unwrap().last_agent_seen_at.is_empty(),
                "{fault}: healthy coordinator did not advance"
            );
            let health: Health = read_evidence(&health_path(&world.root)).unwrap();
            assert!(
                health
                    .failures
                    .iter()
                    .any(|failure| failure.contains(&path.display().to_string())),
                "{fault}: {:?}",
                health.failures
            );
            let (status, text) = health_report(&world.root);
            assert_eq!(status, Some(false), "{text}");
            assert!(text.contains(&path.display().to_string()), "{text}");
            assert!(
                std::fs::read_to_string(log_path(&world.root))
                    .unwrap()
                    .contains(&path.display().to_string())
            );
            // Reading root health does not require loading the broken project.
            if fault == "discovery" || fault == "load" {
                assert!(Project::load(&world.root, "broken").is_err());
            }
        }
    }

    #[test]
    fn repaired_lane_records_clear_the_failure_on_the_next_observation() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane =
            thread::allocate(&project, |lane| lane.status = thread::Status::Resolved).unwrap();
        let path = thread::threads_dir(&project).join(format!("{}.toml", lane.id));
        let original = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, "bad = [").unwrap();
        let ctx = world.ctx();
        let mut memory = Memory::new(&ctx);
        tick_for_test(&ctx, &mut memory);
        let health: Health = read_evidence(&health_path(&world.root)).unwrap();
        assert!(
            health
                .failures
                .iter()
                .any(|failure| failure.contains("does not parse"))
        );
        // In-place repair changes the file, not the directory stamp.
        std::fs::write(&path, original).unwrap();
        tick_for_test(&ctx, &mut memory);
        let health: Health = read_evidence(&health_path(&world.root)).unwrap();
        assert!(
            !health
                .failures
                .iter()
                .any(|failure| failure.contains("does not parse"))
        );
    }

    #[test]
    fn deletion_between_discovery_and_work_does_not_resurrect_records() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        thread::allocate(&project, |_| {}).unwrap();
        let ctx = world.ctx();
        let mut memory = Memory::new(&ctx);
        let log = Log {
            path: log_path(&world.root),
        };
        assert_eq!(
            tick_with_steps(&ctx, &log, &mut memory, &mut |step| {
                if step == "cheap project demo" {
                    std::fs::remove_dir_all(project.dir()).unwrap();
                }
                true
            }),
            Some(false)
        );
        assert!(!project.dir().exists());
        let health: Health = read_evidence(&health_path(&world.root)).unwrap();
        assert!(health.failures.is_empty(), "{:?}", health.failures);
        assert!(project::list_slugs_with_errors(&world.root).0.is_empty());
    }

    #[test]
    fn empty_roots_and_unreadable_roots_are_not_the_same_observation() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let (slugs, errors) = project::list_slugs_with_errors(&root);
        assert!(slugs.is_empty() && errors.is_empty());
        std::fs::create_dir(&root).unwrap();
        let (slugs, errors) = project::list_slugs_with_errors(&root);
        assert!(slugs.is_empty() && errors.is_empty());
        std::fs::remove_dir(&root).unwrap();
        std::fs::write(&root, "not a directory").unwrap();
        let (slugs, errors) = project::list_slugs_with_errors(&root);
        assert!(slugs.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(format!("{:#}", errors[0]).contains(&root.display().to_string()));
    }

    #[test]
    fn an_unavailable_session_is_visible_without_declaring_its_lane_gone() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = thread::allocate(&project, |lane| lane.status = thread::Status::Open).unwrap();
        std::fs::remove_file(project.coordinator().unwrap().socket).unwrap();
        let ctx = world.ctx();
        let mut memory = Memory::new(&ctx);
        assert!(!tick_for_test(&ctx, &mut memory));
        assert_eq!(
            thread::load(&project, &lane.id).unwrap().status,
            thread::Status::Open
        );
        let health: Health = read_evidence(&health_path(&world.root)).unwrap();
        assert!(
            health.failures.iter().any(
                |failure| failure.contains("session unavailable") && failure.contains("a.sock")
            )
        );
    }

    #[test]
    fn same_version_stall_recent_progress_and_released_lock_are_distinct() {
        let root = tempfile::tempdir().unwrap();
        let info = Info {
            version: crate::VERSION.into(),
            pid: 123,
            started: "this-run".into(),
            ..Info::default()
        };
        let mut holder = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path(root.path()))
            .unwrap();
        holder.lock().unwrap();
        holder
            .write_all(serde_json::to_string(&info).unwrap().as_bytes())
            .unwrap();
        let mut progress = Progress {
            pid: info.pid,
            started: info.started.clone(),
            sequence: 4,
            step: "cheap project demo".into(),
            updated: jiff::Timestamp::from_second(
                jiff::Timestamp::now().as_second() - INSTALL_REPLACE_WAIT.as_secs() as i64 - 1,
            )
            .unwrap()
            .to_string(),
        };
        project::write_json(&progress_path(root.path()), &progress).unwrap();
        project::write_json(
            &health_path(root.path()),
            &Health {
                observed: project::now(),
                ..Health::default()
            },
        )
        .unwrap();
        let (status, detail) = health_report(root.path());
        assert_eq!(status, Some(false));
        assert!(
            detail.contains("stalled") && detail.contains("sequence 4"),
            "{detail}"
        );
        let mut spawned = false;
        ensure_free(root.path(), false, |_| {
            spawned = true;
            Ok(())
        })
        .unwrap();
        assert!(health_report(root.path()).1.contains("stalled"));
        assert!(!spawned && !stop_path(root.path()).exists());
        progress.sequence += 1;
        progress.step = "pass complete".into();
        progress.updated = jiff::Timestamp::now().to_string();
        project::write_json(&progress_path(root.path()), &progress).unwrap();
        assert_eq!(health_report(root.path()).0, Some(true));
        ensure_free(root.path(), false, |_| {
            spawned = true;
            Ok(())
        })
        .unwrap();
        assert!(!spawned);
        // Historical progress remains readable, but cannot prove responsiveness.
        std::fs::write(
            progress_path(root.path()),
            r#"{"pid":123,"started":"this-run","sequence":5,"step":"pass complete"}"#,
        )
        .unwrap();
        assert!(
            health_report(root.path())
                .1
                .contains("responsiveness unknown")
        );
        holder.unlock().unwrap();
        assert!(
            health_report(root.path())
                .1
                .contains("not running (lock released)")
        );
        ensure_free(root.path(), true, |_| {
            spawned = true;
            Ok(())
        })
        .unwrap();
        assert!(spawned);
    }

    #[test]
    fn an_unreadable_lock_never_authorizes_a_competing_ticker() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(lock_path(root.path())).unwrap();
        assert!(matches!(lock_state(root.path()), LockState::Unknown(_)));
        let mut spawned = false;
        assert!(
            ensure_free(root.path(), false, |_| {
                spawned = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!spawned);
        assert!(health_report(root.path()).1.contains("ownership unknown"));
    }

    #[test]
    fn sleep_does_not_expire_provider_progress_or_outage_timers() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let now = jiff::Timestamp::now();
        let before = now.as_second() - 2400;
        crate::awake::set_sample(Some((before, 100)));
        drop(crate::awake::enter(&world.root, true).unwrap());
        crate::awake::set_sample(Some((now.as_second(), 140)));
        let (_clock, slept) = crate::awake::enter(&world.root, true).unwrap();
        assert!(slept);
        let waiting = thread::allocate(&project, |t| {
            t.status = thread::Status::Open;
            t.launch.recipe_id = "pi_example".into();
            // Thirty minutes of genuine provider wait before lid close.
            t.provider_wait_started = jiff::Timestamp::from_second(before - 1800)
                .unwrap()
                .to_string();
        })
        .unwrap();
        let mut cache = BTreeMap::from([(
            (crate::contracts::MACHINE_LOCAL.into(), ":".into()),
            Err("pi_not_ready: provider still warming up".into()),
        )]);
        assert!(resume_provider_starts(
            &world.ctx(),
            &project,
            &mut cache,
            |error| panic!("{error:#}")
        ));
        assert_eq!(
            thread::load(&project, &waiting.id).unwrap().status,
            thread::Status::Open
        );
        let timestamp = jiff::Timestamp::from_second(before).unwrap();
        let mut lane = thread::Thread {
            repo: "/repo".into(),
            progress_since: timestamp.to_string(),
            no_commit_since: timestamp.to_string(),
            progress_pane: "w:p".into(),
            progress_screen: "same".into(),
            progress_head: "same".into(),
            ..Default::default()
        };
        let notices = progress_notices(
            &mut lane,
            &steps::LaneProgress {
                pane: "w:p".into(),
                screen: "same".into(),
                head: "same".into(),
            },
            now,
            &ProgressThresholds {
                stall_minutes: 2,
                no_commit_minutes: 4,
            },
        );
        assert!(notices.stalled.is_none());
        assert!(notices.no_commit.is_none());
        let mut outage = steps::Outage::default();
        assert_eq!(
            outage.record(false, "unreachable: ssh timeout", timestamp, 600),
            None
        );
        assert_eq!(
            outage.record(false, "unreachable: ssh timeout", now, 600),
            None
        );
        crate::awake::set_sample(None);
    }

    use crate::paths::Env;
    use crate::runner::fake::{FakeRunner, fail, ok, timeout};

    #[test]
    fn provider_wait_expires_after_an_hour_without_spending_a_retry() {
        use crate::scenarios::World;
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let lane = thread::allocate(&project, |t| {
            t.role = "reviewer".into();
            t.launch.recipe_id = "pi_example".into();
            t.provider_wait_started = "2020-01-01T00:00:00Z".into();
            t.error = "waiting for provider: readiness probe timed out".into();
        })
        .unwrap();
        resume_provider_starts(&world.ctx(), &project, &mut BTreeMap::new(), |error| {
            panic!("{error:#}")
        });
        let failed = thread::load(&project, &lane.id).unwrap();
        assert_eq!(failed.status, thread::Status::Failed);
        assert_eq!(failed.launch_attempts, 0);
        assert_eq!(failed.attempt, 0);
        assert_eq!(
            failed.failure_class,
            crate::contracts::FailureClass::Unknown
        );
        assert!(
            failed.error.contains("provider_wait_expired"),
            "{}",
            failed.error
        );
    }

    #[test]
    fn provider_outage_shares_one_probe_without_a_restart_fleet_or_new_work_budget() {
        let world = crate::scenarios::World::new();
        let first = world.project("first", "a.sock");
        let second = world.project("second", "b.sock");
        let mut lanes = Vec::new();
        for (project, recipe) in [(&first, "one"), (&second, "two")] {
            let lane = thread::allocate(project, |t| {
                t.status = thread::Status::Open;
                t.attempt = 4;
                t.launch.kind = "claude".into();
                t.launch.recipe_id = recipe.into();
                t.launch.work_retries = 2;
                t.launch.same_recipe_retries = 3;
                t.provider_wait_started = project::now();
            })
            .unwrap();
            lanes.push((project, lane));
        }
        let runner = FakeRunner::new();
        let ready = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = ready.clone();
        runner.on_fn(
            |cmd| cmd.program == "claude",
            move |_| {
                Ok(if flag.get() {
                    ok("OK")
                } else {
                    fail(1, "Usage limit reached")
                })
            },
        );
        let ctx = Ctx {
            runner: &runner,
            ..world.ctx()
        };
        let probes = || {
            runner
                .calls
                .borrow()
                .iter()
                .filter(|cmd| cmd.program == "claude")
                .count()
        };
        for _ in 0..2 {
            for project in [&first, &second] {
                resume_provider_starts(&ctx, project, &mut BTreeMap::new(), |error| {
                    panic!("{error:#}")
                });
            }
        }
        assert_eq!(probes(), 1);
        assert!(lanes.iter().all(|(project, lane)| {
            !thread::load(project, &lane.id)
                .unwrap()
                .provider_wait_started
                .is_empty()
        }));
        ready.set(true);
        crate::adapters::expire_dependency_probe(
            &world.root,
            crate::contracts::MACHINE_LOCAL,
            &lanes[0].1.launch,
        );
        for project in [&first, &second] {
            resume_provider_starts(&ctx, project, &mut BTreeMap::new(), |error| {
                panic!("{error:#}")
            });
        }
        assert_eq!(probes(), 2);
        for (project, before) in lanes {
            let after = thread::load(project, &before.id).unwrap();
            assert!(after.provider_wait_started.is_empty());
            assert_eq!(after.attempt, before.attempt);
            assert_eq!(after.launch, before.launch);
        }
        assert_eq!(runner.count("agent start"), 0);
        assert_eq!(runner.count("agent prompt"), 0);
    }

    #[test]
    fn a_recorded_future_reset_outlives_the_lane_wait_timeout() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = thread::allocate(&project, |t| {
            t.role = "reviewer".into();
            t.launch.kind = "claude".into();
            t.provider_wait_started = "2020-01-01T00:00:00Z".into();
        })
        .unwrap();
        let evidence = crate::adapters::DependencyEvidence {
            kind: "quota".into(),
            reset_at: Some(
                "2099-01-01T00:00:00Z"
                    .parse::<jiff::Timestamp>()
                    .unwrap()
                    .as_second(),
            ),
        };
        crate::adapters::observe_dependency(
            &world.root,
            crate::contracts::MACHINE_LOCAL,
            &lane.launch,
            "Usage limit reached",
            &evidence,
        )
        .unwrap();
        resume_provider_starts(&world.ctx(), &project, &mut BTreeMap::new(), |error| {
            panic!("{error:#}")
        });
        let held = thread::load(&project, &lane.id).unwrap();
        assert_eq!(held.status, lane.status);
        assert_eq!(held.provider_wait_started, lane.provider_wait_started);
        assert_eq!(held.launch, lane.launch);
        assert_eq!(world.runner.count("agent start"), 0);
    }

    #[test]
    fn uncertain_session_resume_is_not_replayed_and_live_activity_reconciles_it() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |t| {
            t.bootstrap = "acknowledged".into();
            t.launch.kind = "claude".into();
        });
        world.runner.on("pane read", ok("WebSocket closed"));
        world.runner.on(
            "agent prompt",
            fail(1, "transport disconnected after submission"),
        );
        let ctx = world.ctx();
        let herdr = Herdr::new("herdr", "test.sock", &world.runner);
        let panes = [Pane {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: lane.cwd.clone(),
        }];
        let poll = |state: &str| {
            let current = thread::load(&project, &lane.id).unwrap();
            thread_pass(
                &LaunchPass {
                    ctx: &ctx,
                    project: &project,
                    herdr: &herdr,
                    threads: &[current],
                    agents: &[Agent {
                        pane_id: lane.pane_id.clone(),
                        tab_id: lane.tab_id.clone(),
                        workspace_id: lane.workspace_id.clone(),
                        cwd: lane.cwd.clone(),
                        name: lane.agent_name.clone(),
                        agent_status: state.into(),
                        ..Default::default()
                    }],
                    panes: &panes,
                },
                "ha",
                None,
                false,
                None,
            )
        };
        poll("idle").unwrap();
        crate::adapters::expire_dependency_probe(
            &world.root,
            crate::contracts::MACHINE_LOCAL,
            &lane.launch,
        );
        assert!(poll("idle").is_err());
        poll("idle").unwrap();
        assert_eq!(world.runner.count("agent prompt"), 1);
        assert!(thread::load(&project, &lane.id).unwrap().connection_waiting);
        poll("working").unwrap();
        let current = thread::load(&project, &lane.id).unwrap();
        assert!(!current.connection_waiting);
        assert_eq!(current.connection_resumes.len(), 1);
        assert_eq!(current.attempt, lane.attempt);
    }

    #[test]
    fn stale_session_observations_cannot_duplicate_a_resume_or_touch_a_new_attempt() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |_| {});
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let herdr = Herdr::new("herdr", "test.sock", &world.runner);
        let agents = [Agent {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: lane.cwd.clone(),
            name: lane.agent_name.clone(),
            agent_status: "idle".into(),
            ..Agent::default()
        }];
        for _ in 0..2 {
            resume_session(
                &world.ctx(),
                &project,
                &herdr,
                &agents,
                &lane,
                vec![],
                "Continue",
            )
            .unwrap();
        }
        assert_eq!(world.runner.count("agent prompt"), 1);
        let before = thread::load(&project, &lane.id).unwrap();
        assert_eq!(before.connection_resumes.len(), 1);
        assert_eq!(before.follow_ups.len(), 1);
        assert_eq!(before.follow_ups[0].state, thread::FollowUpState::Delivered);
        let replacement = thread::update(&project, &lane.id, |record| {
            record.attempt += 1;
            record.connection_resumes.clear();
        })
        .unwrap();
        resume_session(
            &world.ctx(),
            &project,
            &herdr,
            &agents,
            &before,
            vec![],
            "Continue",
        )
        .unwrap();
        assert_eq!(world.runner.count("agent prompt"), 1);
        assert_eq!(thread::load(&project, &lane.id).unwrap(), replacement);
    }

    #[test]
    fn deferred_launch_waits_for_disk_without_expiring_then_submits() {
        use crate::scenarios::{World, pane_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("lane");
        let lane = world.thread(&project, &cwd, |record| {
            record.prompt_pending = true;
            record.launch.kind = "claude".into();
        });
        let runner = FakeRunner::new();
        let lane = thread::update(&project, &lane.id, |t| {
            t.startup_wait_started = "2020-01-01T00:00:00Z".into();
            t.launch.ready_timeout_ms = 60_000;
        })
        .unwrap();
        let free = std::rc::Rc::new(std::cell::Cell::new(5_u64));
        let current = free.clone();
        runner.on_fn(|cmd| cmd.display().contains("df -Pk"), move |_| {
            Ok(ok(&format!("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk 50000000 0 {} 1% /\n", current.get() * 1_000_000)))
        });
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2"}}}"#),
        );
        let ctx = Ctx {
            runner: &runner,
            ..world.ctx()
        };
        let herdr = Herdr::new(
            ctx.env.herdr_bin(),
            &project.coordinator().unwrap().socket,
            &runner,
        );
        let panes = serde_json::from_str::<Vec<Pane>>(&format!(
            "[{}]",
            pane_json("w2", "w2:t1", "w2:p1", &cwd.to_string_lossy())
        ))
        .unwrap();
        let records = vec![lane.clone()];
        let pass = LaunchPass {
            ctx: &ctx,
            project: &project,
            herdr: &herdr,
            threads: &records,
            agents: &[],
            panes: &panes,
        };
        let mut errors = Vec::new();
        launch_pass(&pass, &mut true, false, &mut errors);
        assert_eq!(runner.count("agent start"), 0);
        assert_eq!(thread::load(&project, &lane.id).unwrap().launch_attempts, 0);
        assert!(
            thread::load(&project, &lane.id)
                .unwrap()
                .error
                .starts_with("disk_low:")
        );
        let waiting = thread::load(&project, &lane.id).unwrap();
        assert!(waiting.startup_wait_started.is_empty());
        let checked = thread_pass(
            &LaunchPass {
                threads: std::slice::from_ref(&waiting),
                ..pass
            },
            "ha",
            None,
            false,
            None,
        )
        .unwrap();
        assert!(checked.error.is_none(), "{:?}", checked.error);
        assert_eq!(
            thread::load(&project, &lane.id).unwrap().status,
            thread::Status::Open
        );
        free.set(20);
        launch_pass(&pass, &mut true, false, &mut errors);
        assert_eq!(runner.count("agent start"), 1);
        let launched = thread::load(&project, &lane.id).unwrap();
        assert_eq!(launched.launch_attempts, 1);
        assert!(thread::in_start_window(&launched, jiff::Timestamp::now()));
    }

    #[test]
    fn explicit_launch_delivers_brief_when_agent_registers_three_seconds_later() {
        use crate::scenarios::{World, pane_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("lane");
        let lane = world.thread(&project, &cwd, |record| {
            record.prompt_pending = true;
            record.launch.kind = "claude".into();
        });
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            pane_json("w2", "w2:t1", "w2:p1", &cwd.to_string_lossy())
        );
        // The start command returns before herdr's agent list knows the name.
        // Simulate the registration delay in the list rather than in start.
        let runner = FakeRunner::new();
        let launched = std::rc::Rc::new(std::cell::Cell::new(None::<Instant>));
        let started = launched.clone();
        runner.on_fn(
            |cmd| cmd.display().contains("agent start hp-demo-t-0001"),
            move |_| {
                started.set(Some(Instant::now()));
                Ok(ok(r#"{"result":{"agent":{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","name":"hp-demo-t-0001"}}}"#))
            },
        );
        let observed = launched.clone();
        let cwd_for_list = cwd.to_string_lossy().to_string();
        runner.on_fn(
            |cmd| cmd.display().contains("agent list"),
            move |_| {
                let agents = if observed.get().is_some_and(|at| at.elapsed() >= Duration::from_secs(3)) {
                    format!(r#"[{{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","cwd":"{cwd_for_list}","name":"hp-demo-t-0001","agent_status":"idle"}}]"#)
                } else {
                    "[]".to_string()
                };
                Ok(ok(&format!(r#"{{"result":{{"agents":{agents}}}}}"#)))
            },
        );
        let panes = world.panes.borrow().clone();
        runner.on(
            "pane list",
            ok(&format!(r#"{{"result":{{"panes":{panes}}}}}"#)),
        );
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let ctx = Ctx {
            runner: &runner,
            ..world.ctx()
        };
        launch_thread_now(&ctx, &project, &lane.id).unwrap();
        let saved = thread::load(&project, &lane.id).unwrap();
        assert_eq!(saved.launch_attempts, 1);
        assert!(!saved.prompt_pending);
        assert_eq!(runner.count("agent start hp-demo-t-0001"), 1);
        assert_eq!(runner.count("agent prompt"), 1);
    }

    #[test]
    fn late_registration_keeps_brief_pending_until_ticker_sees_named_agent() {
        use crate::scenarios::{World, pane_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("lane");
        let lane = world.thread(&project, &cwd, |record| {
            record.prompt_pending = true;
            record.launch.kind = "claude".into();
        });
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            pane_json("w2", "w2:t1", "w2:p1", &cwd.to_string_lossy())
        );
        world.runner.on("agent start hp-demo-t-0001", ok(r#"{"result":{"agent":{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","name":"hp-demo-t-0001"}}}"#));
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
        launch_thread_with_wait(&world.ctx(), &project, &lane.id, Duration::from_millis(120))
            .unwrap();
        let pending = thread::load(&project, &lane.id).unwrap();
        assert_eq!(pending.status, thread::Status::Open);
        assert!(pending.prompt_pending);
        assert_eq!(world.runner.count("agent prompt"), 0);

        *world.agents.borrow_mut() = format!(
            r#"[{{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","cwd":"{}","name":"hp-demo-t-0001","agent_status":"idle"}}]"#,
            cwd.display()
        );
        let socket = project.coordinator().unwrap().socket;
        let herdr = Herdr::new(world.env.herdr_bin(), &socket, &world.runner);
        let agents = herdr.agent_list().unwrap();
        let panes = herdr.pane_list().unwrap();
        let pass = thread_pass(
            &LaunchPass {
                ctx: &world.ctx(),
                project: &project,
                herdr: &herdr,
                threads: &[pending],
                agents: &agents,
                panes: &panes,
            },
            "ha",
            None,
            false,
            None,
        )
        .unwrap();
        assert!(pass.error.is_none());
        assert!(!thread::load(&project, &lane.id).unwrap().prompt_pending);
        assert_eq!(world.runner.count("agent prompt"), 1);
    }

    #[test]
    fn capped_brief_wait_stays_pending_without_resubmission() {
        use crate::scenarios::World;
        for (code, activity) in [
            ("timeout", "working"),
            ("agent_prompt_stalled", "blocked"),
            ("timeout", "idle"),
        ] {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            let lane = world.thread(&project, &world.home.path().join("lane"), |record| {
                record.prompt_pending = true;
                record.launch_attempts = 1;
                record.launch.ready_timeout_ms = 300_000;
            });
            world.runner.on(
                "agent prompt",
                ok(&format!(
                    r#"{{"error":{{"code":"{code}","message":"wait expired"}}}}"#
                )),
            );
            let socket = project.coordinator().unwrap().socket;
            let herdr = Herdr::new(world.env.herdr_bin(), &socket, &world.runner);
            let mut agent = Agent {
                pane_id: lane.pane_id.clone(),
                tab_id: lane.tab_id.clone(),
                workspace_id: lane.workspace_id.clone(),
                cwd: lane.cwd.clone(),
                name: lane.agent_name.clone(),
                agent_status: "idle".into(),
                ..Agent::default()
            };
            let pane = Pane {
                pane_id: lane.pane_id.clone(),
                tab_id: lane.tab_id.clone(),
                workspace_id: lane.workspace_id.clone(),
                cwd: lane.cwd.clone(),
            };
            let run_pass = |snapshot: &thread::Thread, agent: &Agent| {
                thread_pass(
                    &LaunchPass {
                        ctx: &world.ctx(),
                        project: &project,
                        herdr: &herdr,
                        threads: std::slice::from_ref(snapshot),
                        agents: std::slice::from_ref(agent),
                        panes: std::slice::from_ref(&pane),
                    },
                    "ha",
                    None,
                    false,
                    None,
                )
                .unwrap()
            };
            assert!(run_pass(&lane, &agent).error.is_none());
            let pending = thread::load(&project, &lane.id).unwrap();
            assert_eq!(pending.status, thread::Status::Open);
            assert!(pending.prompt_pending);
            assert!(pending.brief_submitted);
            let calls = world.runner.calls.borrow();
            let prompt = calls
                .iter()
                .find(|cmd| cmd.display().contains("agent prompt"))
                .unwrap();
            let cap = crate::herdr::AGENT_START_TIMEOUT.as_millis() as u64;
            assert!(
                prompt
                    .args
                    .windows(2)
                    .any(|pair| pair == ["--timeout", &cap.to_string()])
            );
            assert_eq!(
                prompt.timeout,
                crate::herdr::AGENT_START_TIMEOUT + Duration::from_secs(5)
            );
            drop(calls);

            // Even the original idle snapshot is rechecked under the brief lock.
            assert!(run_pass(&lane, &agent).error.is_none());
            assert!(thread::load(&project, &lane.id).unwrap().prompt_pending);
            assert_eq!(world.runner.count("agent prompt"), 1);

            agent.agent_status = activity.into();
            if activity == "idle" {
                thread::update(&project, &lane.id, |record| {
                    record.bootstrap = "acknowledged".into()
                })
                .unwrap();
            }
            assert!(run_pass(&pending, &agent).error.is_none());
            assert!(!thread::load(&project, &lane.id).unwrap().prompt_pending);
            assert!(run_pass(&lane, &agent).error.is_none());
            assert_eq!(world.runner.count("agent prompt"), 1);

            // A replacement attempt or pane earns its own submission.
            thread::update(&project, &lane.id, |record| record.attempt += 1).unwrap();
            assert!(!thread::load(&project, &lane.id).unwrap().brief_submitted);
            assert!(
                thread::load(&project, &lane.id)
                    .unwrap()
                    .brief_submitted_at
                    .is_empty()
            );
            thread::update(&project, &lane.id, |record| {
                record.brief_submitted = true;
                record.brief_submitted_at = project::now();
            })
            .unwrap();
            thread::update(&project, &lane.id, |record| record.pane_id = "w3:p1".into()).unwrap();
            assert!(!thread::load(&project, &lane.id).unwrap().brief_submitted);
            assert!(
                thread::load(&project, &lane.id)
                    .unwrap()
                    .brief_submitted_at
                    .is_empty()
            );
        }
    }

    #[test]
    fn undelivered_brief_fails_after_its_window_without_replaying_or_killing() {
        use crate::scenarios::World;
        for remote in [false, true] {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            let lane = world.thread(&project, &world.home.path().join("lane"), |record| {
                record.prompt_pending = true;
                record.launch_attempts = 1;
                record.launch.ready_timeout_ms = 300_000;
                if remote {
                    record.machine = "box".into();
                }
            });
            world.runner.on("machine list --json", ok("[]"));
            // The command timed out without a server submission, as t-0607's
            // box log shows. Staging must not become an infinite delivery latch.
            world.runner.on(
                "agent prompt",
                crate::runner::Output {
                    timed_out: true,
                    ..Default::default()
                },
            );
            world.runner.on("pane read", ok("pi\nempty prompt\n"));
            let socket = project.coordinator().unwrap().socket;
            let local = Herdr::new(world.env.herdr_bin(), &socket, &world.runner);
            let herdr = local.on_machine(lane.machine_route());
            let mut agent = Agent {
                pane_id: lane.pane_id.clone(),
                tab_id: lane.tab_id.clone(),
                workspace_id: lane.workspace_id.clone(),
                cwd: lane.cwd.clone(),
                name: lane.agent_name.clone(),
                agent_status: "idle".into(),
                ..Agent::default()
            };
            let pane = Pane {
                pane_id: lane.pane_id.clone(),
                tab_id: lane.tab_id.clone(),
                workspace_id: lane.workspace_id.clone(),
                cwd: lane.cwd.clone(),
            };
            let run_pass = |agent: &Agent| {
                let current = thread::load(&project, &lane.id).unwrap();
                thread_pass(
                    &LaunchPass {
                        ctx: &world.ctx(),
                        project: &project,
                        herdr: &herdr,
                        threads: &[current],
                        agents: std::slice::from_ref(agent),
                        panes: std::slice::from_ref(&pane),
                    },
                    "ha",
                    None,
                    false,
                    None,
                )
                .unwrap()
            };
            assert!(run_pass(&agent).error.is_none());
            let pending = thread::load(&project, &lane.id).unwrap();
            assert!(pending.error.contains("timed out"));
            assert!(!pending.brief_submitted_at.is_empty());
            // An old persisted stage (including an interrupted send) gets a
            // full window when no timestamp was recorded, without a new paste.
            thread::update(&project, &lane.id, |record| {
                record.brief_submitted_at.clear()
            })
            .unwrap();
            assert!(run_pass(&agent).error.is_none());
            let historical = thread::load(&project, &lane.id).unwrap();
            assert_eq!(historical.status, thread::Status::Open);
            assert!(!historical.brief_submitted_at.is_empty());
            // A lane can take about a minute to start. Neither its first poll
            // nor a minute-old ambiguous submission is a delivery failure.
            thread::update(&project, &lane.id, |record| {
                record.brief_submitted_at =
                    (jiff::Timestamp::now() - jiff::SignedDuration::from_secs(60)).to_string();
            })
            .unwrap();
            assert!(run_pass(&agent).error.is_none());
            assert_eq!(
                thread::load(&project, &lane.id).unwrap().status,
                thread::Status::Open
            );
            thread::update(&project, &lane.id, |record| {
                record.brief_submitted_at =
                    (jiff::Timestamp::now() - jiff::SignedDuration::from_secs(301)).to_string();
            })
            .unwrap();
            assert!(run_pass(&agent).error.is_none());
            let failed = thread::load(&project, &lane.id).unwrap();
            assert_eq!(failed.status, thread::Status::Failed);
            assert!(failed.prompt_pending);
            assert_eq!(failed.pane_id, lane.pane_id);
            assert!(failed.error.starts_with("brief_delivery_failed:"));
            assert!(failed.error.contains("timed out"));
            assert!(failed.error.contains("empty prompt"));
            assert_eq!(failed.last_group, "waiting-on-you");
            assert!(run_pass(&agent).error.is_none());
            assert_eq!(world.runner.count("agent prompt"), 1);
            assert_eq!(world.runner.count("tab close"), 0);
            assert_eq!(world.runner.count("workspace close"), 0);
            assert_eq!(crate::inbox::unhandled(&project).len(), 1);
            // A late start settles the submission instead of creating another
            // attempt. The retry path separately checks fresh working state.
            if remote {
                thread::update(&project, &lane.id, |record| {
                    record.bootstrap = "acknowledged".into()
                })
                .unwrap();
            } else {
                agent.agent_status = "working".into();
            }
            assert!(run_pass(&agent).error.is_none());
            let settled = thread::load(&project, &lane.id).unwrap();
            assert_eq!(settled.status, thread::Status::Open);
            assert!(!settled.prompt_pending);
            assert!(settled.error.is_empty());
            assert_eq!(world.runner.count("agent prompt"), 1);
        }
    }

    #[test]
    fn remote_delivery_failure_keeps_polling_until_late_evidence() {
        use crate::scenarios::{World, agent_json, pane_json};
        for evidence in ["none", "working", "bootstrap"] {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            let lane = world.thread(&project, &world.home.path().join("lane"), |record| {
                record.machine = "box".into();
                record.machine_id = "box".into();
                record.status = thread::Status::Failed;
                record.prompt_pending = true;
                record.brief_submitted = true;
                record.launch_attempts = 1;
                record.attempt = 1;
                record.launch.brief_hash = "frozen".into();
                record.error = "brief_delivery_failed: no activity".into();
            });
            let agent = agent_json(
                &lane.workspace_id,
                &lane.tab_id,
                &lane.pane_id,
                &lane.cwd,
                &lane.agent_name,
                if evidence == "working" {
                    "working"
                } else {
                    "idle"
                },
            );
            let pane = pane_json(&lane.workspace_id, &lane.tab_id, &lane.pane_id, &lane.cwd);
            let manifest = crate::box_helper::tests::ready(crate::steps::CourierManifest {
                boot_id: "boot-1".into(),
                agents: Some(serde_json::from_str(&format!("[{agent}]")).unwrap()),
                panes: Some(serde_json::from_str(&format!("[{pane}]")).unwrap()),
                bootstraps: if evidence == "bootstrap" {
                    vec![crate::steps::BootstrapReceipt {
                        slug: "demo".into(),
                        thread: lane.id.clone(),
                        brief_hash: "frozen".into(),
                        pane: lane.pane_id.clone(),
                    }]
                } else {
                    Vec::new()
                },
                ..Default::default()
            });
            world
                .runner
                .on_fn(|cmd| cmd.program == "ssh", move |_| Ok(ok(&manifest)));
            world.runner.on(
                "machine list --json",
                ok(r#"[{"id":"box","label":"box","target":"box","session":"default","enabled":true}]"#),
            );
            let ctx = world.ctx();
            let mut memory = Memory::new(&ctx);
            memory.tick = 1;
            let log = Log {
                path: world.home.path().join("ticker.log"),
            };
            assert!(machine_passes(&ctx, &[&project], &mut memory, &log).is_empty());
            let view = memory
                .machine_views
                .get("box")
                .expect("failed delivery must still be polled")
                .as_ref()
                .unwrap();
            let threads = open_threads(&project, true);
            assert_eq!(
                threads.len(),
                1,
                "failed delivery must reach the remote state pass"
            );
            let herdr = Herdr::new(
                ctx.env.herdr_bin(),
                &project.coordinator().unwrap().socket,
                &world.runner,
            );
            let mut errors = Vec::new();
            remote_pass(
                &LaunchPass {
                    ctx: &ctx,
                    project: &project,
                    herdr: &herdr,
                    threads: &threads,
                    agents: &[],
                    panes: &[],
                },
                "box",
                view,
                &mut true,
                &mut errors,
            )
            .unwrap();
            assert!(errors.is_empty(), "{errors:#?}");
            let current = thread::load(&project, &lane.id).unwrap();
            assert_eq!(current.attempt, lane.attempt);
            assert_eq!(current.pane_id, lane.pane_id);
            if evidence == "none" {
                assert_eq!(current.status, thread::Status::Failed);
                assert!(current.prompt_pending);
            } else {
                assert_eq!(current.status, thread::Status::Open);
                assert!(!current.prompt_pending);
                assert!(current.error.is_empty());
            }
            assert_eq!(world.runner.count("agent prompt"), 0);
            assert_eq!(world.runner.count("agent start"), 0);
            assert_eq!(world.runner.count("tab close"), 0);
            assert_eq!(world.runner.count("workspace close"), 0);
        }
    }

    #[test]
    fn paused_project_imports_a_box_seal_and_parks_without_starting_work() {
        use crate::contracts::{DonePayload, Event, EventPayload, Recipient};
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = thread::allocate(&project, |t| {
            t.status = thread::Status::Open;
            t.machine = "box".into();
            t.machine_id = "box".into();
            t.pane_id = "w2:p1".into();
            t.tab_id = "w2:t1".into();
            t.workspace_id = "w2".into();
        })
        .unwrap();
        project.set_status(Status::Paused).unwrap();
        let report = b"finished on the box\n";
        let artifact = thread::sha256_hex(report);
        let event = Event {
            id: format!("{}-1-1", lane.id),
            op: format!("{}-1-1", lane.id),
            thread: lane.id.clone(),
            attempt: 1,
            created: project::now(),
            usage: None,
            recipient: Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 1,
            },
            payload: EventPayload {
                done: Some(DonePayload {
                    sha: "abc".into(),
                    report_path: ".reports/lane.md".into(),
                    artifact: artifact.clone(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        };
        let bytes = toml::to_string(&event).unwrap().into_bytes();
        let hash = thread::sha256_hex(&bytes);
        let manifest = crate::box_helper::tests::ready(crate::steps::CourierManifest {
            boot_id: "boot-1".into(),
            agents: Some(Vec::new()),
            panes: Some(Vec::new()),
            envelopes: vec![crate::steps::BoxEnvelope {
                slug: "demo".into(),
                event: event.id.clone(),
                event_path: format!("/box/demo/.state/events/{}.toml", event.id),
                event_hash: hash.clone(),
                artifact_path: format!("/box/demo/.state/artifacts/{artifact}"),
                artifact_hash: artifact.clone(),
            }],
            receipts: vec![crate::steps::CompletionReceipt {
                slug: "demo".into(),
                event: event.id.clone(),
                event_hash: hash,
                artifact_hash: artifact.clone(),
            }],
            ..Default::default()
        });
        world.runner.on(
            "machine list --json",
            ok(r#"[{"id":"box","label":"box","target":"box","session":"default","enabled":true}]"#),
        );
        world
            .runner
            .on_fn(|cmd| cmd.program == "ssh", move |_| Ok(ok(&manifest)));
        let id = event.id.clone();
        world.runner.on_fn(
            |cmd| cmd.program == "scp",
            move |cmd| {
                let dir = PathBuf::from(cmd.args.last().unwrap().trim_end_matches('/'));
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join(format!("{id}.toml")), &bytes)?;
                std::fs::write(dir.join(&artifact), report)?;
                Ok(ok(""))
            },
        );
        let ctx = world.ctx();
        let mut memory = Memory::new(&ctx);
        tick_for_test(&ctx, &mut memory);
        assert_eq!(crate::events::list(&project).len(), 1);
        assert!(thread::load(&project, &lane.id).unwrap().parked);
        assert_eq!(project.status(), Status::Paused);
        assert_eq!(world.runner.count("agent start"), 0);
        assert_eq!(world.runner.count("agent prompt"), 0);
    }

    #[test]
    fn courier_install_skew_is_unavailable_not_lane_death_or_a_link_outage() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, &world.home.path().join("lane"), |t| {
            t.machine = "box".into();
            t.machine_id = "box".into();
            t.status = thread::Status::Open;
            t.launch_attempts = 1;
        });
        let mut state = crate::events::remote_state(&project, "box");
        state.missing.insert(lane.id.clone(), 2);
        crate::events::save_remote_state(&project, "box", &state).unwrap();
        world.runner.on(
            "machine list --json",
            ok(r#"[{"id":"box","label":"box","target":"box","session":"default","enabled":true}]"#),
        );
        world
            .runner
            .on("ssh", ok(r#"{"status":"Skew","build":"0.1.0+old.1"}"#));
        let ctx = world.ctx();
        let mut memory = Memory::new(&ctx);
        let log = Log {
            path: world.home.path().join("ticker.log"),
        };
        for tick in 1..=12 {
            memory.tick = tick;
            machine_passes(&ctx, &[&project], &mut memory, &log);
            assert!(
                memory.machine_views["box"]
                    .as_ref()
                    .unwrap_err()
                    .contains("version_skew:")
            );
            let held = thread::load(&project, &lane.id).unwrap();
            assert_eq!(held.status, thread::Status::Open);
            assert_eq!(held.attempt, lane.attempt);
            assert!(
                crate::events::remote_state(&project, "box")
                    .missing
                    .is_empty()
            );
            assert!(crate::events::list(&project).is_empty());
            assert!(crate::inbox::unhandled(&project).is_empty());
        }
    }

    #[test]
    fn every_project_on_a_box_gets_one_outage_and_recovery_notice() {
        use crate::scenarios::World;
        for detail in [
            "ssh: connect to host box: Operation timed out",
            "helper failed",
        ] {
            let world = World::new();
            let first = world.project("first", "first.sock");
            let second = world.project("second", "second.sock");
            for project in [&first, &second] {
                for _ in 0..2 {
                    thread::allocate(project, |lane| {
                        lane.status = thread::Status::Open;
                        lane.machine = "box".into();
                        lane.machine_id = "box".into();
                    })
                    .unwrap();
                }
            }
            let down = std::rc::Rc::new(std::cell::Cell::new(true));
            let flag = down.clone();
            world.runner.on_fn(
                |cmd| cmd.program == "ssh",
                move |_| {
                    Ok(if flag.get() {
                        fail(255, detail)
                    } else {
                        ok(&crate::box_helper::tests::ready(
                            crate::steps::CourierManifest {
                                boot_id: "boot-1".into(),
                                agents: Some(Vec::new()),
                                panes: Some(Vec::new()),
                                ..Default::default()
                            },
                        ))
                    })
                },
            );
            world.runner.on(
                "machine list --json",
                ok(r#"[{"id":"box","label":"box","target":"box","session":"default","enabled":true}]"#),
            );
            let ctx = world.ctx();
            let mut memory = Memory::new(&ctx);
            memory.outage_secs = 0;
            let log = Log {
                path: world.home.path().join("ticker.log"),
            };
            let projects = [&first, &second, &first];
            let notices = |project: &Project| {
                inbox::unhandled(project)
                    .into_iter()
                    .filter(|item| item.kind == "outage")
                    .collect::<Vec<_>>()
            };
            for tick in [1, 10] {
                memory.tick = tick;
                assert!(machine_passes(&ctx, &projects, &mut memory, &log).is_empty());
                for project in [&first, &second] {
                    let items = notices(project);
                    assert_eq!(items.len(), 1);
                }
            }
            down.set(false);
            for tick in [19, 20] {
                memory.tick = tick;
                assert!(machine_passes(&ctx, &projects, &mut memory, &log).is_empty());
                for project in [&first, &second] {
                    let items = notices(project);
                    assert_eq!(items.len(), 2);
                }
            }
            assert_eq!(
                world
                    .runner
                    .calls
                    .borrow()
                    .iter()
                    .filter(|cmd| cmd.program == "ssh")
                    .count(),
                4
            );
        }
    }

    #[test]
    fn agent_not_ready_during_brief_prompt_is_pending_not_failed() {
        use crate::scenarios::{World, pane_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("lane");
        let lane = world.thread(&project, &cwd, |t| {
            t.prompt_pending = true;
            t.launch_attempts = 1;
        });
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            pane_json("w2", "w2:t1", "w2:p1", &cwd.to_string_lossy())
        );
        *world.agents.borrow_mut() = format!(
            r#"[{{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","cwd":"{}","name":"hp-demo-t-0001","agent_status":"idle"}}]"#,
            cwd.display()
        );
        world.runner.on(
            "agent prompt",
            ok(r#"{"error":{"code":"agent_not_ready","message":"registration pending"}}"#),
        );
        launch_thread_with_wait(&world.ctx(), &project, &lane.id, Duration::from_millis(120))
            .unwrap();
        let saved = thread::load(&project, &lane.id).unwrap();
        assert_eq!(saved.status, thread::Status::Open);
        assert!(saved.prompt_pending);
        assert!(!saved.brief_submitted);
    }

    #[test]
    fn one_remote_pass_submits_every_independent_lane_start() {
        let fixture = fixture(false);
        std::fs::create_dir_all(fixture.root.join("cfg")).unwrap();
        std::fs::write(
            fixture.root.join("cfg/config.toml"),
            crate::remote::TEST_MACHINE,
        )
        .unwrap();
        let runner = FakeRunner::new();
        runner.on("machine list --json", ok("[]"));
        runner.on("pane process-info", ok(r#"{"result":{"process_info":{"foreground_processes":[{"pid":42,"name":"claude","argv0":"claude"}]}}}"#));
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"workspace_id":"w2","tab_id":"w2:t1","pane_id":"w2:p1"}}}"#),
        );
        let mut records = Vec::new();
        let mut panes = Vec::new();
        for number in 1..=3 {
            let id = format!("t-{number:04}");
            let tab_id = format!("w2:t{number}");
            let pane_id = format!("w2:p{number}");
            let cwd = format!("/box/lane-{number}");
            let record = thread::allocate(&fixture.project, |record| {
                record.id = id.clone();
                record.status = thread::Status::Open;
                record.prompt_pending = true;
                record.machine = "buildbox".into();
                record.machine_id = "buildbox".into();
                record.workspace_id = "w2".into();
                record.tab_id = tab_id.clone();
                record.pane_id = pane_id.clone();
                record.cwd = cwd.clone();
                record.agent = "claude".into();
                record.agent_name = format!("hp-demo-{id}");
                record.launch.kind = "claude".into();
                record.attempt = 2;
            })
            .unwrap();
            panes.push(Pane {
                pane_id,
                tab_id,
                workspace_id: "w2".into(),
                cwd,
            });
            records.push(record);
        }
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "test.sock", &runner);
        let mut may_start = true;
        let mut errors = Vec::new();
        launch_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: &records,
                agents: &[],
                panes: &panes,
            },
            &mut may_start,
            false,
            &mut errors,
        );

        assert!(errors.is_empty(), "{errors:#?}");
        assert_eq!(runner.count("agent start"), 3);
        for record in thread::list(&fixture.project) {
            assert_eq!(record.launch_attempts, 1, "{} was not submitted", record.id);
        }
        let rebound = thread::load(&fixture.project, "t-0001").unwrap();
        assert_eq!(
            rebound.identity.socket,
            fixture.project.coordinator().unwrap().socket
        );
        assert_eq!(rebound.identity.pane_id, rebound.pane_id);
        assert_eq!(rebound.identity.cwd, rebound.cwd);
        assert!(!rebound.identity.pane_id.is_empty());
    }

    #[test]
    fn startup_block_keeps_the_screen_and_failure_class_on_the_thread() {
        let fixture = fixture(false);
        let runner = FakeRunner::new();
        runner.on(
            "agent start",
            ok(r#"{"error":{"code":"agent_not_ready","message":"blocked during startup"}}"#),
        );
        runner.on_fn(
            |cmd| cmd.display().contains("pane read w1:p1"),
            |_| Ok(ok("❯ \n")),
        );
        runner.on("pane read", ok("Trust this folder?\n  1. Yes\n  2. No\n"));
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let record = thread::allocate(&fixture.project, |t| {
            t.status = thread::Status::Open;
            t.prompt_pending = true;
            t.pane_id = "w2:p1".into();
            t.tab_id = "w2:t1".into();
            t.workspace_id = "w2".into();
            t.cwd = "/repo".into();
            t.agent_name = "hp-demo-t-0001".into();
            t.launch.kind = "claude".into();
        })
        .unwrap();
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "test.sock", &runner);
        let panes = [Pane {
            pane_id: record.pane_id.clone(),
            tab_id: record.tab_id.clone(),
            workspace_id: record.workspace_id.clone(),
            cwd: record.cwd.clone(),
        }];
        let records = [record.clone()];
        let coordinator = Agent {
            pane_id: fixture.project.coordinator().unwrap().pane_id,
            agent_status: "idle".into(),
            ..Agent::default()
        };
        let mut start = true;
        let mut errors = Vec::new();
        launch_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: &records,
                agents: std::slice::from_ref(&coordinator),
                panes: &panes,
            },
            &mut start,
            true,
            &mut errors,
        );
        let saved = thread::load(&fixture.project, &record.id).unwrap();
        assert_eq!(saved.status, thread::Status::Starting);
        assert!(saved.prompt_pending);
        assert!(!saved.startup_wait_started.is_empty());
        assert!(errors.is_empty());
        for _ in 0..2 {
            let blocked = Agent {
                pane_id: record.pane_id.clone(),
                tab_id: record.tab_id.clone(),
                workspace_id: record.workspace_id.clone(),
                cwd: record.cwd.clone(),
                name: record.agent_name.clone(),
                agent_status: "blocked".into(),
                ..Agent::default()
            };
            let current = thread::load(&fixture.project, &record.id).unwrap();
            thread_pass(
                &LaunchPass {
                    ctx: &ctx,
                    project: &fixture.project,
                    herdr: &herdr,
                    threads: &[current],
                    agents: &[blocked],
                    panes: &panes,
                },
                "ha",
                None,
                true,
                None,
            )
            .unwrap();
            assert_eq!(
                thread::load(&fixture.project, &record.id).unwrap().status,
                thread::Status::Starting
            );
        }
        thread::update(&fixture.project, &record.id, |t| {
            t.startup_wait_started = "2020-01-01T00:00:00Z".into()
        })
        .unwrap();
        let blocked = Agent {
            pane_id: record.pane_id.clone(),
            tab_id: record.tab_id.clone(),
            workspace_id: record.workspace_id.clone(),
            cwd: record.cwd.clone(),
            name: record.agent_name.clone(),
            agent_status: "blocked".into(),
            ..Agent::default()
        };
        let current = thread::load(&fixture.project, &record.id).unwrap();
        thread_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: &[current],
                agents: &[blocked, coordinator],
                panes: &panes,
            },
            "ha",
            None,
            true,
            None,
        )
        .unwrap();
        let saved = thread::load(&fixture.project, &record.id).unwrap();
        assert_eq!(saved.status, thread::Status::Failed);
        assert_eq!(saved.launch_attempts, 1);
        assert!(
            saved.error.contains("Trust this folder? | 1. Yes | 2. No"),
            "{}",
            saved.error
        );
        assert_eq!(saved.failure_class, crate::contracts::FailureClass::Unknown);
        assert!(errors.is_empty());
        assert_eq!(runner.count("agent start"), 1);
        assert_eq!(runner.count("agent prompt"), 0);
        assert_eq!(saved.start_notices.len(), 1);
        assert!(saved.start_notices[0].line.contains("Trust this folder?"));
        assert!(!saved.start_notices[0].submitted);
    }

    #[test]
    fn idle_connection_error_resumes_in_place_and_stops_after_three_in_an_hour() {
        let fixture = fixture(false);
        let runner = FakeRunner::new();
        runner.on("pane read", ok("WebSocket closed\n"));
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        runner.on_fn(|cmd| cmd.program == "claude", |_| Ok(ok("OK")));
        let record = thread::allocate(&fixture.project, |t| {
            t.status = thread::Status::Open;
            t.bootstrap = "acknowledged".into();
            t.launch.kind = "claude".into();
            t.launch_attempts = 1;
            t.pane_id = "w2:p1".into();
            t.tab_id = "w2:t1".into();
            t.workspace_id = "w2".into();
            t.cwd = "/repo".into();
            t.agent_name = "hp-demo-t-0001".into();
        })
        .unwrap();
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "test.sock", &runner);
        let panes = [Pane {
            pane_id: record.pane_id.clone(),
            tab_id: record.tab_id.clone(),
            workspace_id: record.workspace_id.clone(),
            cwd: record.cwd.clone(),
        }];
        let agents = [Agent {
            pane_id: record.pane_id.clone(),
            tab_id: record.tab_id.clone(),
            workspace_id: record.workspace_id.clone(),
            cwd: record.cwd.clone(),
            name: record.agent_name.clone(),
            agent_status: "idle".into(),
            ..Agent::default()
        }];
        let poll = || {
            let current = thread::load(&fixture.project, &record.id).unwrap();
            thread_pass(
                &LaunchPass {
                    ctx: &ctx,
                    project: &fixture.project,
                    herdr: &herdr,
                    threads: &[current],
                    agents: &agents,
                    panes: &panes,
                },
                "ha",
                None,
                true,
                None,
            )
            .unwrap();
        };
        poll();
        assert_eq!(runner.count("agent prompt"), 0);
        crate::adapters::expire_dependency_probe(
            &fixture.root,
            crate::contracts::MACHINE_LOCAL,
            &record.launch,
        );
        poll();
        assert_eq!(runner.count("agent prompt"), 1);
        let saved = thread::load(&fixture.project, &record.id).unwrap();
        assert_eq!(saved.launch_attempts, 1);
        assert_eq!(saved.connection_resumes.len(), 1);
        // Model three real, spaced-out recoveries without waiting an hour.
        thread::update(&fixture.project, &record.id, |t| {
            t.connection_resumes = vec![
                "2020-01-01T00:00:00Z".into(),
                "2020-01-01T00:01:00Z".into(),
                "2020-01-01T00:02:00Z".into(),
            ];
        })
        .unwrap();
        // The bound uses a rolling hour, not a lifetime count.
        poll();
        assert_eq!(runner.count("agent prompt"), 2);
        thread::update(&fixture.project, &record.id, |t| {
            t.connection_resumes = vec![project::now(); 3];
        })
        .unwrap();
        poll();
        let saved = thread::load(&fixture.project, &record.id).unwrap();
        assert!(saved.connection_waiting);
        assert_eq!(saved.launch_attempts, 1);
        assert_eq!(runner.count("agent prompt"), 2);
    }

    #[test]
    fn lane_presence_keeps_missing_registration_distinct_from_confirmed_death() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let mut lane = world.thread(&project, world.home.path(), |_| {});
        let pane = Pane {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: lane.cwd.clone(),
        };
        let herdr = Herdr::new("herdr", "test.sock", &world.runner);
        let view = ObservationView {
            machine_id: "",
            threads: &[],
            agents: &[],
            panes: std::slice::from_ref(&pane),
            boot_id: "boot-1",
            now: jiff::Timestamp::now(),
        };
        let observed = LaneObservation::read(&lane, view, &herdr, true);
        assert!(observed.presence == Presence::Unavailable);
        assert!(observed.rebooted);
        assert_eq!(
            observed.identity,
            format!(
                "{}:{}:{}:{}",
                lane.attempt, lane.workspace_id, lane.tab_id, lane.pane_id
            )
        );
        assert!(
            LaneObservation::read(&lane, ObservationView { panes: &[], ..view }, &herdr, false)
                .presence
                == Presence::Absent
        );
        let agent = Agent {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: lane.cwd.clone(),
            name: lane.agent_name.clone(),
            agent_status: "working".into(),
            ..Default::default()
        };
        assert!(
            LaneObservation::read(
                &lane,
                ObservationView {
                    agents: std::slice::from_ref(&agent),
                    ..view
                },
                &herdr,
                false
            )
            .presence
                == Presence::Present
        );
        lane.identity.workspace_id = lane.workspace_id.clone();
        lane.identity.tab_id = lane.tab_id.clone();
        lane.identity.pane_id = lane.pane_id.clone();
        lane.identity.process = Some(crate::contracts::ProcessIdentity {
            pid: 42,
            argv0: "pi".into(),
        });
        world
            .runner
            .on("pane process-info", fail(1, "process evidence unavailable"));
        assert!(
            LaneObservation::read(&lane, view, &herdr, false).presence == Presence::Unavailable
        );
        let shell = FakeRunner::new();
        shell.on("pane process-info", ok(r#"{"result":{"process_info":{"pane_id":"w2:p1","foreground_processes":[{"pid":5,"name":"bash"}]}}}"#));
        let herdr = Herdr::new("herdr", "test.sock", &shell);
        assert!(LaneObservation::read(&lane, view, &herdr, false).presence == Presence::Absent);
    }

    #[test]
    fn box_blocked_has_one_durable_producer_before_coordinator_transport() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |t| {
            t.machine = "box".into();
            t.machine_id = "box".into();
            t.repo.clear();
            t.bootstrap = "acknowledged".into();
            t.launch_attempts = 1;
        });
        let agent = Agent {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: lane.cwd.clone(),
            name: lane.agent_name.clone(),
            agent_status: "blocked".into(),
            ..Default::default()
        };
        let ctx = world.ctx();
        let herdr = Herdr::new("herdr", "test.sock", &world.runner);
        let view = steps::CourierOutcome {
            machine_id: "box".into(),
            boot_id: "boot-1".into(),
            agents: Some(vec![agent]),
            panes: Some(vec![]),
            progress: Default::default(),
        };
        let starting = thread::update(&project, &lane.id, |t| {
            t.startup_wait_started = project::now()
        })
        .unwrap();
        assert!(
            observation_pass(
                &ctx,
                &project,
                ObservationView {
                    machine_id: "box",
                    threads: std::slice::from_ref(&starting),
                    agents: view.agents.as_ref().unwrap(),
                    panes: &[],
                    boot_id: "boot-1",
                    now: jiff::Timestamp::now(),
                }
            )
            .is_empty()
        );
        assert!(
            thread::load(&project, &lane.id)
                .unwrap()
                .start_notices
                .is_empty()
        );
        thread::update(&project, &lane.id, |t| t.startup_wait_started.clear()).unwrap();
        for _ in 0..3 {
            let current = thread::load(&project, &lane.id).unwrap();
            let input = LaunchPass {
                ctx: &ctx,
                project: &project,
                herdr: &herdr,
                threads: std::slice::from_ref(&current),
                agents: &[],
                panes: &[],
            };
            let mut errors = Vec::new();
            remote_pass(&input, "box", &view, &mut false, &mut errors).unwrap();
            assert!(errors.is_empty(), "{errors:?}");
        }
        let saved = thread::load(&project, &lane.id).unwrap();
        assert_eq!(saved.last_group, "waiting-on-you");
        assert_eq!(saved.start_notices.len(), 1);
        assert!(saved.start_notices[0].line.starts_with("BLOCKED"));
        assert_eq!(world.runner.count("agent prompt"), 0);
        let c = project.coordinator().unwrap();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::agent_json(
                &c.workspace_id,
                &c.tab_id,
                &c.pane_id,
                &c.cwd,
                &c.agent_name,
                "idle"
            )
        );
        world.runner.on("pane read", ok("❯ \n"));
        let stored_project = project.clone();
        let id = lane.id.clone();
        world.runner.on_fn(
            |cmd| cmd.display().contains("agent prompt"),
            move |_| {
                let saved = thread::load(&stored_project, &id).unwrap();
                assert_eq!(saved.last_group, "waiting-on-you");
                assert_eq!(saved.start_notices.len(), 1);
                Ok(ok(r#"{"result":{}}"#))
            },
        );
        steps::deliver_transition_notices(&ctx, &project).unwrap();
        steps::flush_notices_for_test(&project, &herdr);
        steps::deliver_transition_notices(&ctx, &project).unwrap();
        assert_eq!(world.runner.count("agent prompt"), 1);
        assert_eq!(world.runner.count("pane submit-text"), 0);
        assert_eq!(world.runner.count("--clear-token parent"), 0);
    }

    #[test]
    fn box_blocked_after_an_absent_snapshot_still_queues_one_notice() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |t| {
            t.machine = "box".into();
            t.machine_id = "box".into();
            t.last_state = "working".into();
            t.last_group = "working".into();
        });
        let ctx = world.ctx();
        let observe = |agents: &[Agent]| {
            let current = thread::load(&project, &lane.id).unwrap();
            let errors = observation_pass(
                &ctx,
                &project,
                ObservationView {
                    machine_id: "box",
                    threads: std::slice::from_ref(&current),
                    agents,
                    panes: &[],
                    boot_id: "boot-1",
                    now: jiff::Timestamp::now(),
                },
            );
            assert!(errors.is_empty(), "{errors:?}");
        };
        observe(&[]);
        let missing = thread::load(&project, &lane.id).unwrap();
        assert_eq!(missing.status, thread::Status::Open);
        assert_eq!(missing.last_group, "waiting-on-you");
        assert_eq!(
            crate::events::remote_state(&project, "box")
                .missing
                .get(&lane.id),
            Some(&1)
        );
        let agent = Agent {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: lane.cwd.clone(),
            name: lane.agent_name.clone(),
            agent_status: "blocked".into(),
            ..Default::default()
        };
        for _ in 0..3 {
            observe(std::slice::from_ref(&agent));
        }
        let saved = thread::load(&project, &lane.id).unwrap();
        assert_eq!(saved.status, thread::Status::Open);
        assert_eq!(saved.start_notices.len(), 1);
        assert!(saved.start_notices[0].line.starts_with("BLOCKED"));
        assert_eq!(world.runner.count("agent prompt"), 0);
    }

    #[test]
    fn post_start_blocked_lane_alerts_once_and_working_rearms_it_without_keys() {
        let f = fixture(false);
        let runner = FakeRunner::new();
        runner.on("pane read", ok("❯ \n"));
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let lane = thread::allocate(&f.project, |t| {
            t.status = thread::Status::Open;
            t.pane_id = "w1:p2".into();
            t.tab_id = "w1:t2".into();
            t.workspace_id = "w1".into();
            t.cwd = "/lane".into();
            t.agent_name = "hp-demo-t-0001".into();
            t.last_state = "blocked".into();
            t.last_state_change = "2020-01-01T00:00:00Z".into();
            t.last_group = "working".into();
        })
        .unwrap();
        let coordinator = f.project.coordinator().unwrap();
        runner.on("agent list", ok(&with_cwd(AGENT_READY, &f)));
        let coordinator_agent = Agent {
            pane_id: coordinator.pane_id,
            tab_id: coordinator.tab_id,
            workspace_id: coordinator.workspace_id,
            cwd: coordinator.cwd,
            name: coordinator.agent_name,
            agent_status: "idle".into(),
            ..Agent::default()
        };
        let mut agent = Agent {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: lane.cwd.clone(),
            name: lane.agent_name.clone(),
            agent_status: "blocked".into(),
            ..Agent::default()
        };
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "test.sock", &runner);
        let poll = |agent: &Agent| {
            let current = thread::load(&f.project, &lane.id).unwrap();
            thread_pass(
                &LaunchPass {
                    ctx: &ctx,
                    project: &f.project,
                    herdr: &herdr,
                    threads: &[current],
                    agents: &[agent.clone(), coordinator_agent.clone()],
                    panes: &[],
                },
                "ha",
                None,
                false,
                None,
            )
            .unwrap();
        };
        poll(&agent);
        poll(&agent);
        assert_eq!(runner.count("agent prompt"), 0);
        assert_eq!(
            thread::load(&f.project, &lane.id)
                .unwrap()
                .start_notices
                .len(),
            1
        );
        steps::deliver_transition_notices(&ctx, &f.project).unwrap();
        steps::flush_notices_for_test(&f.project, &herdr);
        assert_eq!(runner.count("agent prompt"), 1);
        assert!(
            runner
                .calls
                .borrow()
                .iter()
                .any(|call| call.display().contains(&format!(
                    "BLOCKED {} needs input in w1:p2; inspect the current question; no keys were sent",
                    lane.id
                )))
        );
        assert_eq!(runner.count("pane submit-text"), 0);
        agent.agent_status = "working".into();
        poll(&agent);
        assert_eq!(
            thread::load(&f.project, &lane.id).unwrap().last_group,
            "working"
        );
        agent.agent_status = "blocked".into();
        poll(&agent);
        // The new spell starts a fresh duration, not an instant alert.
        assert_eq!(runner.count("agent prompt"), 1);
    }

    #[test]
    fn a_soon_ready_agent_gets_its_brief_after_repeated_startup_blocks() {
        let fixture = fixture(false);
        let runner = FakeRunner::new();
        runner.on("agent rename", ok(r#"{"result":{}}"#));
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let record = thread::allocate(&fixture.project, |t| {
            t.status = thread::Status::Starting;
            t.prompt_pending = true;
            t.startup_wait_started = project::now();
            t.launch_attempts = 1;
            t.launch.kind = "claude".into();
            t.launch.ready_timeout_ms = 300_000;
            t.pane_id = "w2:p1".into();
            t.tab_id = "w2:t1".into();
            t.workspace_id = "w2".into();
            t.cwd = "/repo".into();
            t.agent_name = "hp-demo-t-0001".into();
        })
        .unwrap();
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "test.sock", &runner);
        let panes = [Pane {
            pane_id: record.pane_id.clone(),
            tab_id: record.tab_id.clone(),
            workspace_id: record.workspace_id.clone(),
            cwd: record.cwd.clone(),
        }];
        for state in ["blocked", "blocked", "idle"] {
            let agent = Agent {
                pane_id: record.pane_id.clone(),
                tab_id: record.tab_id.clone(),
                workspace_id: record.workspace_id.clone(),
                cwd: record.cwd.clone(),
                // Herdr can drop the name on a timed-out start. The pane is
                // still ours and must be renamed before its brief is sent.
                name: if state == "idle" {
                    String::new()
                } else {
                    record.agent_name.clone()
                },
                agent_status: state.into(),
                ..Agent::default()
            };
            let current = thread::load(&fixture.project, &record.id).unwrap();
            thread_pass(
                &LaunchPass {
                    ctx: &ctx,
                    project: &fixture.project,
                    herdr: &herdr,
                    threads: &[current],
                    agents: &[agent],
                    panes: &panes,
                },
                "ha",
                None,
                true,
                None,
            )
            .unwrap();
            let saved = thread::load(&fixture.project, &record.id).unwrap();
            assert_eq!(
                saved.status,
                if state == "idle" {
                    thread::Status::Open
                } else {
                    thread::Status::Starting
                }
            );
            assert_eq!(saved.last_group, "working");
        }
        let saved = thread::load(&fixture.project, &record.id).unwrap();
        assert!(!saved.prompt_pending);
        assert!(saved.startup_wait_started.is_empty());
        assert_eq!(runner.count("agent prompt"), 1);
        assert_eq!(runner.count("agent rename"), 1);
        assert_eq!(saved.identity.agent_name.as_deref(), Some("hp-demo-t-0001"));
    }

    #[test]
    fn a_listed_pane_without_agent_state_stays_open_and_unknown() {
        let fixture = fixture(false);
        let runner = FakeRunner::new();
        let record = thread::allocate(&fixture.project, |record| {
            record.status = thread::Status::Open;
            record.workspace_id = "w1".into();
            record.tab_id = "w1:t2".into();
            record.pane_id = "w1:p2".into();
            record.cwd = "/work/lane".into();
            record.agent = "agy".into();
            record.agent_name = "hp-demo-t-0001".into();
            record.last_state = "idle".into();
            record.last_group = "working".into();
        })
        .unwrap();
        let pane = Pane {
            workspace_id: record.workspace_id.clone(),
            tab_id: record.tab_id.clone(),
            pane_id: record.pane_id.clone(),
            cwd: record.cwd.clone(),
        };
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "test.sock", &runner);
        let pass = thread_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: std::slice::from_ref(&record),
                agents: &[],
                panes: &[pane],
            },
            "ha",
            Some(&std::collections::BTreeMap::new()),
            true,
            None,
        )
        .unwrap();

        assert!(pass.error.is_none());
        let saved = thread::load(&fixture.project, &record.id).unwrap();
        assert_eq!(saved.status, thread::Status::Open);
        assert_eq!(saved.failure_class, crate::contracts::FailureClass::Unknown);
        assert_eq!(saved.last_group, thread::Group::Unknown.token());
        assert_eq!(runner.count("tab close"), 0);
    }

    #[test]
    fn an_identified_agent_dying_before_seal_sends_gone_even_with_a_draft_report() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let c = project.coordinator().unwrap();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::agent_json(
                &c.workspace_id,
                &c.tab_id,
                &c.pane_id,
                &c.cwd,
                &c.agent_name,
                "working",
            )
        );
        let lane = thread::allocate(&project, |t| {
            t.status = thread::Status::Open;
            t.pane_id = "w2:p1".into();
            t.tab_id = "w2:t1".into();
            t.workspace_id = "w2".into();
            t.cwd = "/lane".into();
            t.agent_name = "hp-demo-t-0001".into();
            t.report_hash = "unsealed-draft".into();
            t.identity.workspace_id = t.workspace_id.clone();
            t.identity.tab_id = t.tab_id.clone();
            t.identity.pane_id = t.pane_id.clone();
            t.identity.process = Some(crate::contracts::ProcessIdentity {
                pid: 42,
                argv0: "pi".into(),
            });
        })
        .unwrap();
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            crate::scenarios::pane_json("w2", "w2:t1", "w2:p1", "/lane")
        );
        world.runner.on("pane process-info", ok(r#"{"result":{"process_info":{"pane_id":"w2:p1","foreground_processes":[{"pid":5,"name":"bash"}]}}}"#));
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let ctx = world.ctx();
        let herdr = Herdr::new("herdr", &c.socket, &world.runner);
        let pass = thread_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &project,
                herdr: &herdr,
                threads: std::slice::from_ref(&lane),
                agents: &herdr.agent_list().unwrap(),
                panes: &herdr.pane_list().unwrap(),
            },
            "ha",
            None,
            false,
            None,
        )
        .unwrap();
        assert!(pass.error.is_none(), "{:?}", pass.error);
        let failed = thread::load(&project, &lane.id).unwrap();
        assert_eq!(failed.status, thread::Status::Failed);
        assert_eq!(
            failed.failure_class,
            crate::contracts::FailureClass::ProcessGone
        );
        steps::deliver_transition_notices(&ctx, &project).unwrap();
        steps::deliver_transition_notices(&ctx, &project).unwrap();
        assert_eq!(
            world
                .runner
                .calls
                .borrow()
                .iter()
                .filter(
                    |cmd| cmd.display().contains("agent prompt") && cmd.display().contains("GONE")
                )
                .count(),
            1
        );
    }

    #[test]
    fn recovered_local_attempt_ignores_old_identity_until_its_start_timeout() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let c = project.coordinator().unwrap();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::agent_json(
                &c.workspace_id,
                &c.tab_id,
                &c.pane_id,
                &c.cwd,
                &c.agent_name,
                "working",
            )
        );
        let lane = thread::allocate(&project, |t| {
            t.status = thread::Status::Starting;
            t.attempt = 2;
            t.workspace_id = "w2".into();
            t.tab_id = "w2:t2".into();
            t.pane_id = "w2:p2".into();
            t.cwd = "/lane".into();
            t.startup_wait_started = project::now();
            t.launch.ready_timeout_ms = 60_000;
            t.identity.workspace_id = "w2".into();
            t.identity.tab_id = "w2:t1".into();
            t.identity.pane_id = "w2:p1".into();
            t.identity.process = Some(crate::contracts::ProcessIdentity {
                pid: 42,
                argv0: "pi".into(),
            });
            t.identity.agent_session = Some("saved-session".into());
        })
        .unwrap();
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            crate::scenarios::pane_json("w2", "w2:t2", "w2:p2", "/lane")
        );
        world.runner.on("pane process-info", ok(r#"{"result":{"process_info":{"pane_id":"w2:p2","foreground_processes":[{"pid":5,"name":"bash"}]}}}"#));
        let ctx = world.ctx();
        let herdr = Herdr::new("herdr", &c.socket, &world.runner);
        let check = || {
            let pass = thread_pass(
                &LaunchPass {
                    ctx: &ctx,
                    project: &project,
                    herdr: &herdr,
                    threads: &[thread::load(&project, &lane.id).unwrap()],
                    agents: &herdr.agent_list().unwrap(),
                    panes: &herdr.pane_list().unwrap(),
                },
                "ha",
                None,
                false,
                None,
            )
            .unwrap();
            assert!(pass.error.is_none(), "{:?}", pass.error);
        };
        check();
        let saved = thread::load(&project, &lane.id).unwrap();
        assert_eq!(saved.status, thread::Status::Starting);
        assert_eq!(saved.attempt, 2);
        assert_eq!(
            saved.identity.agent_session.as_deref(),
            Some("saved-session")
        );
        assert_eq!(world.runner.count("tab close"), 0);
        assert_eq!(world.runner.count("workspace close"), 0);
        // Readiness can disappear after placement but before submission. A
        // provider pause must not use up this attempt's startup window.
        thread::update(&project, &lane.id, |t| {
            t.status = thread::Status::Open;
            t.provider_wait_started = project::now();
            t.startup_wait_started = "2020-01-01T00:00:00Z".into();
        })
        .unwrap();
        let mut ready = BTreeMap::from([(
            crate::adapters::dependency_key(crate::contracts::MACHINE_LOCAL, &lane.launch),
            Ok(()),
        )]);
        resume_provider_starts(&ctx, &project, &mut ready, |error| panic!("{error:#}"));
        let resumed = thread::load(&project, &lane.id).unwrap();
        assert!(resumed.provider_wait_started.is_empty());
        assert!(thread::in_start_window(&resumed, jiff::Timestamp::now()));
        check();
        let resumed = thread::load(&project, &lane.id).unwrap();
        assert_eq!(resumed.status, thread::Status::Open);
        assert_eq!(resumed.attempt, 2);
        assert_eq!(world.runner.count("tab close"), 0);
        thread::update(&project, &lane.id, |t| {
            t.startup_wait_started = "2020-01-01T00:00:00Z".into();
        })
        .unwrap();
        check();
        assert_eq!(
            thread::load(&project, &lane.id).unwrap().status,
            thread::Status::Open
        );
        assert_eq!(
            world.runner.count("tab close") + world.runner.count("workspace close"),
            0
        );
        thread::update(&project, &lane.id, |t| t.launch_attempts = 1).unwrap();
        check();
        let failed = thread::load(&project, &lane.id).unwrap();
        assert_eq!(failed.status, thread::Status::Failed);
        assert_eq!(
            failed.failure_class,
            crate::contracts::FailureClass::ProcessGone
        );
        assert!(world.runner.count("tab close") + world.runner.count("workspace close") > 0);
    }

    #[test]
    fn a_lane_placed_after_the_pane_snapshot_is_not_failed_or_closed() {
        let fixture = fixture(false);
        let record = thread::allocate(&fixture.project, |t| {
            t.status = thread::Status::Open;
            t.workspace_id = "w1".into();
            t.tab_id = "w1:t2".into();
            t.pane_id = "w1:p2".into();
            t.cwd = "/work/lane".into();
        })
        .unwrap();
        let coordinator_pane = Pane {
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            pane_id: "w1:p1".into(),
            cwd: fixture.project.dir().to_string_lossy().into_owned(),
        };
        let live_json = r#"{"result":{"panes":[{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1","cwd":"/work"},{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","cwd":"/elsewhere"}]}}"#;
        let runner = FakeRunner::new();
        runner.on("pane list", ok(live_json));
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "test.sock", &runner);
        let pass = thread_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: std::slice::from_ref(&record),
                agents: &[],
                panes: std::slice::from_ref(&coordinator_pane), // snapshot before placement
            },
            "ha",
            None,
            false,
            None,
        )
        .unwrap();
        assert!(pass.error.is_none());
        assert_eq!(runner.count("pane list"), 1);
        assert_eq!(runner.count("tab close"), 0);
        assert_eq!(
            thread::load(&fixture.project, &record.id).unwrap().status,
            thread::Status::Open
        );

        // A later pass whose fresh view really lacks the pane can still fail it.
        let gone = FakeRunner::new();
        gone.on("pane list", ok(&with_cwd(PANE, &fixture)));
        gone.on("agent list", ok(NO_AGENTS));
        let ctx = Ctx {
            runner: &gone,
            ..ctx
        };
        let herdr = Herdr::new(
            "herdr",
            &fixture.project.coordinator().unwrap().socket,
            &gone,
        );
        let current = thread::load(&fixture.project, &record.id).unwrap();
        let pass = thread_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: &[current],
                agents: &[],
                panes: &[coordinator_pane],
            },
            "ha",
            None,
            false,
            None,
        )
        .unwrap();
        assert!(pass.error.is_none(), "{:?}", pass.error);
        let failed = thread::load(&fixture.project, &record.id).unwrap();
        assert_eq!(failed.status, thread::Status::Failed);
        assert_eq!(
            failed.failure_class,
            crate::contracts::FailureClass::ProcessGone
        );
    }

    #[test]
    fn a_replacement_placed_during_the_recheck_is_not_failed() {
        let fixture = fixture(false);
        let record = thread::allocate(&fixture.project, |t| {
            t.status = thread::Status::Open;
            t.workspace_id = "w1".into();
            t.tab_id = "w1:t2".into();
            t.pane_id = "w1:p2".into();
            t.cwd = "/work/lane".into();
        })
        .unwrap();
        let project = fixture.project.clone();
        let id = record.id.clone();
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.display().contains("pane list"),
            move |_| {
                thread::update(&project, &id, |t| {
                    t.attempt += 1;
                    t.tab_id = "w1:t3".into();
                    t.pane_id = "w1:p3".into();
                })?;
                Ok(ok(r#"{"result":{"panes":[{"workspace_id":"w1","tab_id":"w1:t3","pane_id":"w1:p3","cwd":"/work/lane"}]}}"#))
            },
        );
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "test.sock", &runner);
        let coordinator_pane = Pane {
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            pane_id: "w1:p1".into(),
            cwd: fixture.project.dir().to_string_lossy().into_owned(),
        };
        let pass = thread_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: std::slice::from_ref(&record),
                agents: &[],
                panes: &[coordinator_pane],
            },
            "ha",
            None,
            false,
            None,
        )
        .unwrap();
        assert!(pass.error.is_none());
        let saved = thread::load(&fixture.project, &record.id).unwrap();
        assert_eq!(saved.status, thread::Status::Open);
        assert_eq!(saved.attempt, record.attempt + 1);
        assert_eq!(saved.pane_id, "w1:p3");
        assert_eq!(runner.count("pane list"), 1);
        assert_eq!(runner.count("tab close"), 0);
    }

    #[test]
    fn a_new_box_lane_wakes_the_ticker_and_marks_its_machine_due() {
        let fixture = fixture(false);

        request_remote_poll(&fixture.root, &fixture.project, "machine-1").unwrap();

        assert!(wake_path(&fixture.root).exists());
        assert!(poll_requests(&fixture.project).contains("machine-1"));
    }

    #[test]
    fn a_box_poll_closes_only_a_managed_tab_with_verified_empty_process_state() {
        let fixture = fixture(false);
        thread::allocate(&fixture.project, |record| {
            record.status = thread::Status::Resolved;
            record.machine = "buildbox".into();
            record.machine_id = "machine-1".into();
            record.workspace_id = "w2".into();
            record.tab_id = "w2:t1".into();
            record.pane_id = "w2:p1".into();
            record.cwd = "/deleted-worktree".into();
        })
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "pane process-info --pane w2:p1",
            ok(r#"{"result":{"process_info":{"pane_id":"w2:p1","foreground_processes":[]}}}"#),
        );
        runner.on("tab close w2:t1", ok(r#"{"result":{}}"#));
        runner.on("pane report-metadata", ok(r#"{"result":{}}"#));
        let herdr = Herdr::new("herdr", "", &runner).on_machine("machine-1");
        let panes = [Pane {
            pane_id: "w2:p1".into(),
            tab_id: "w2:t1".into(),
            workspace_id: "w2".into(),
            cwd: "/deleted-worktree".into(),
        }];

        let errors = clean_managed_project_tabs(&fixture.project, "machine-1", &herdr, &[], &panes);

        assert!(errors.is_empty(), "{errors:#?}");
        assert_eq!(runner.count("tab close w2:t1"), 1);
        assert_eq!(runner.count("tab close w2:t2"), 0);

        let mut shared_tab = panes.to_vec();
        shared_tab.push(Pane {
            pane_id: "w2:p2".into(),
            tab_id: "w2:t1".into(),
            workspace_id: "w2".into(),
            cwd: "/someone-else".into(),
        });
        let errors =
            clean_managed_project_tabs(&fixture.project, "machine-1", &herdr, &[], &shared_tab);
        assert!(errors.is_empty(), "{errors:#?}");
        assert_eq!(runner.count("tab close w2:t1"), 1);
    }

    #[test]
    fn successful_child_exit_without_a_ticker_is_a_startup_failure() {
        let root = tempfile::tempdir().unwrap();
        let error = spawn_and_confirm(
            spawn_command(Path::new("/bin/true"), root.path()),
            root.path(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("without a running ticker"),
            "{error}"
        );
        assert_eq!(lock_state(root.path()), LockState::Free);
    }

    #[test]
    fn a_detached_ticker_uses_the_projects_root_not_the_callers_folder() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("projects-root");
        std::fs::create_dir(&root).unwrap();
        let command = spawn_command(Path::new("/bin/true"), &root);
        assert_eq!(command.get_current_dir(), Some(root.as_path()));
    }

    /// `ensure` never writes the stop file, so `review` cannot deadlock
    /// waiting for a running ticker whose own pass waits on its lock.
    #[test]
    fn ensure_leaves_a_running_ticker_alone() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let lane_path = thread::threads_dir_for_write(&project)
            .unwrap()
            .join("t-0001.toml");
        std::fs::write(&lane_path, "bad = [").unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &env,
            root: root.clone(),
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: true,
        };
        // A running ticker of another version: `start` would stop it, `ensure`
        // must not.
        let mut file = File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(lock_path(&root))
            .unwrap();
        file.lock().unwrap();
        file.write_all(br#"{"version":"old","pid":1}"#).unwrap();
        let mut memory = Memory::new(&ctx);
        assert!(!tick_for_test(&ctx, &mut memory));
        // An ensure-only discovery cannot clear an unobserved lane failure.
        let broken = root.join("broken");
        std::os::unix::fs::symlink("broken", &broken).unwrap();
        ensure(&ctx).unwrap();
        let (_, detail) = health_report(&root);
        assert!(detail.contains("responsiveness unknown"), "{detail}");
        assert!(
            detail.contains(&lane_path.display().to_string()),
            "{detail}"
        );
        assert!(
            detail.contains(&broken.join("PROJECT.md").display().to_string()),
            "{detail}"
        );
        // A partial pass also keeps the prior failure until completion.
        std::fs::remove_file(&lane_path).unwrap();
        let log = Log {
            path: log_path(&root),
        };
        assert!(
            tick_with_steps(&ctx, &log, &mut memory, &mut |step| step != "machine phase").is_none()
        );
        assert!(
            health_report(&root)
                .1
                .contains(&lane_path.display().to_string())
        );
        assert!(!tick_for_test(&ctx, &mut memory));
        assert!(
            !health_report(&root)
                .1
                .contains(&lane_path.display().to_string())
        );
        assert!(!stop_path(&root).exists(), "ensure writes no stop file");
        drop(file);
    }

    #[test]
    fn ensure_spawns_once_when_free_and_never_contends_with_a_lock_holder() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let mut starts = 0;
        ensure_free(path, false, |_| {
            starts += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(starts, 1);
        let file = File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(lock_path(path))
            .unwrap();
        file.lock().unwrap();
        ensure_free(path, false, |_| {
            starts += 1;
            Ok(())
        })
        .unwrap();
        assert!(health_report(path).1.contains("responsiveness unknown"));
        assert_eq!(starts, 1);
    }

    #[test]
    fn supervisor_reports_only_an_unexpected_exit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        project::write_atomic(&clean_exit_path(path), b"").unwrap();
        ensure_free(path, true, |_| Ok(())).unwrap();
        assert!(!clean_exit_path(path).exists());
        assert!(!recovery_path(path).exists());
        ensure_free(path, true, |_| Ok(())).unwrap();
        assert!(recovery_path(path).exists());
    }

    #[test]
    fn lock_probe_sees_a_holder_and_its_version() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(lock_state(root.path()), LockState::Free);
        let mut file = File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(lock_path(root.path()))
            .unwrap();
        file.lock().unwrap();
        file.write_all(br#"{"version":"v9","pid":1}"#).unwrap();
        match lock_state(root.path()) {
            LockState::Held(info) => assert_eq!(info.version, "v9"),
            state => panic!("lock should be held: {state:?}"),
        }
        drop(file);
        // Another test may fork a child at this instant; until that child execs,
        // it shares the locked descriptor. Real callers poll too (`ticker stop`).
        let deadline = Instant::now() + Duration::from_secs(2);
        while lock_state(root.path()) != LockState::Free && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(lock_state(root.path()), LockState::Free);
    }

    #[test]
    fn a_contender_exits_immediately_without_waiting_for_the_lock() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
        let mut old = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path(&root))
            .unwrap();
        old.lock().unwrap();
        old.write_all(br#"{"version":"old","pid":1}"#).unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &env,
            root: root.clone(),
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: true,
        };
        let start = Instant::now();
        run(&ctx).unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!stop_path(&root).exists());
        assert!(matches!(lock_state(&root), LockState::Held(info) if info.pid == 1));
    }

    #[test]
    fn unpublished_lock_holder_is_not_stopped_as_stale() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
        let holder = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path(&root))
            .unwrap();
        holder.lock().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &env,
            root: root.clone(),
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: true,
        };
        let error = start_for_install(&ctx).unwrap_err().to_string();
        assert!(error.contains("has not published"), "{error}");
        assert!(!stop_path(&root).exists());
    }

    #[test]
    fn a_timed_out_install_leaves_a_stop_request_for_the_next_start() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
        let mut holder = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path(&root))
            .unwrap();
        holder.lock().unwrap();
        holder
            .write_all(br#"{"version":"old","pid":1,"started":"pass-1"}"#)
            .unwrap();
        project::write_json(
            &progress_path(&root),
            &Progress {
                pid: 1,
                started: "pass-1".into(),
                sequence: 4,
                step: "machine oci".into(),
                ..Progress::default()
            },
        )
        .unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &env,
            root: root.clone(),
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: true,
        };
        let error = start_for_install_with_wait(&ctx, Duration::from_millis(75))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("timed out") && error.contains("old") && error.contains("machine oci"),
            "{error}"
        );
        assert!(stop_path(&root).exists());
        assert!(matches!(lock_state(&root), LockState::Held(info) if info.pid == 1));
        // The old pass finishes and observes the stop request. The next
        // command can start the installed binary without a manual stop.
        drop(holder);
        // Concurrent tests may fork while this descriptor is locked. A child
        // briefly inherits it until exec, so wait for the observed release.
        let deadline = Instant::now() + Duration::from_secs(2);
        while lock_state(&root) != LockState::Free && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(lock_state(&root), LockState::Free);
        ensure_free(&root, false, |_| Ok(())).unwrap();
        assert!(!stop_path(&root).exists());
    }

    #[test]
    fn install_wait_resets_only_when_the_holder_reaches_another_step() {
        let root = tempfile::tempdir().unwrap();
        let info = Info {
            version: "old".into(),
            pid: 123,
            started: "this-run".into(),
            ..Info::default()
        };
        let progress = Progress {
            pid: info.pid,
            started: info.started.clone(),
            sequence: 1,
            step: "cheap project demo".into(),
            ..Progress::default()
        };
        project::write_json(&progress_path(root.path()), &progress).unwrap();
        let path = progress_path(root.path());
        let clock = std::cell::Cell::new(Duration::ZERO);
        let mut updated = false;
        assert_eq!(
            request_stop_with_progress_on(
                root.path(),
                Duration::from_millis(100),
                true,
                || clock.get(),
                |interval| {
                    clock.set(clock.get() + interval);
                    if clock.get() >= Duration::from_millis(75) && !updated {
                        project::write_json(
                            &path,
                            &Progress {
                                sequence: 2,
                                step: "machine oci".into(),
                                ..progress.clone()
                            },
                        )
                        .unwrap();
                        updated = true;
                    }
                },
                |_| {
                    if clock.get() >= Duration::from_millis(125) {
                        LockState::Free
                    } else {
                        LockState::Held(info.clone())
                    }
                }
            )
            .unwrap(),
            StopOutcome::Stopped
        );
        assert!(clock.get() >= Duration::from_millis(125));
    }

    #[test]
    fn stop_between_projects_skips_the_second_even_if_it_would_block() {
        use std::cell::Cell;
        struct BlockingSecond<'a> {
            fake: &'a FakeRunner,
            root: &'a Path,
            agent_lists: Cell<usize>,
        }
        impl crate::runner::Runner for BlockingSecond<'_> {
            fn run(&self, cmd: &crate::runner::Cmd) -> Result<crate::runner::Output> {
                if cmd.display().contains("agent list") {
                    let next = self.agent_lists.get() + 1;
                    self.agent_lists.set(next);
                    if next == 1 {
                        // Arrives during the first project's cheap step.
                        std::fs::write(stop_path(self.root), b"")?;
                    } else {
                        // A real second project could wait here on SSH.
                        std::thread::sleep(Duration::from_millis(300));
                    }
                }
                self.fake.run(cmd)
            }
        }
        let fixture = fixture(false);
        let blocked = project::create(&fixture.root, "zzz-blocked", "", vec![]).unwrap();
        blocked
            .update_coordinator(|record| {
                *record = fixture.project.coordinator().unwrap();
            })
            .unwrap();
        let fake = FakeRunner::new();
        fake.on("agent list", ok(NO_AGENTS));
        fake.on("pane list", ok(&with_cwd(PANE, &fixture)));
        let runner = BlockingSecond {
            fake: &fake,
            root: &fixture.root,
            agent_lists: Cell::new(0),
        };
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let log = Log {
            path: fixture.root.join("test.log"),
        };
        let mut memory = Memory::new(&ctx);
        let result = tick_with_steps(&ctx, &log, &mut memory, &mut |_| {
            !stop_path(&fixture.root).exists()
        });
        assert_eq!(result, None);
        assert_eq!(runner.agent_lists.get(), 1);
    }

    #[test]
    fn stop_during_a_slow_project_skips_its_remaining_work() {
        let fixture = fixture(false);
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let seen = Seen {
            socket: String::new(),
            agents: vec![],
            panes: vec![],
            session_lost: false,
        };
        let mut memory = Memory::new(&ctx);
        let mut steps = Vec::new();
        let (errors, completed) =
            tick_slow_with_steps(&ctx, &fixture.project, &seen, &mut memory, &mut |name| {
                steps.push(name.to_string());
                if name == "recovery" {
                    std::fs::write(stop_path(&fixture.root), b"").unwrap();
                }
                !stop_path(&fixture.root).exists() || name == "recovery"
            });
        assert!(!completed);
        assert!(errors.is_empty(), "{errors:#?}");
        assert_eq!(steps, ["recovery", "local launches"]);
        assert_eq!(runner.count("agent start"), 0);
    }

    #[test]
    fn explicit_start_does_not_claim_success_with_a_stale_holder() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
        let mut holder = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path(&root))
            .unwrap();
        holder.lock().unwrap();
        holder
            .write_all(br#"{"version":"old","pid":42928}"#)
            .unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &env,
            root: root.clone(),
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: true,
        };
        let error = start(&ctx).unwrap_err().to_string();
        assert!(error.contains("42928") && error.contains("old"), "{error}");
        assert!(!stop_path(&root).exists());
    }

    #[test]
    fn install_waits_for_a_pass_longer_than_five_seconds() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
        let mut holder = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path(&root))
            .unwrap();
        holder.lock().unwrap();
        holder.write_all(br#"{"version":"old","pid":1}"#).unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &env,
            root: root.clone(),
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: true,
        };
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5500));
            drop(holder);
        });
        let start = Instant::now();
        // This unit-test executable cannot run `ticker run`; its failed exit
        // must now be reported instead of mistaken for a successful spawn.
        let error = start_for_install(&ctx).unwrap_err().to_string();
        assert!(error.contains("ticker startup failed"), "{error}");
        assert!(start.elapsed() >= Duration::from_secs(5));
        release.join().unwrap();
        assert!(!stop_path(&root).exists());
    }

    #[test]
    fn explicit_start_reports_install_deferral_when_no_ticker_runs() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
        let config = home.path().join("cfg");
        let _install = crate::harness::lock(&config).unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &env,
            root: root.clone(),
            config_dir: config,
            runner: &runner,
            detached_ticker: true,
        };
        assert!(
            start(&ctx)
                .unwrap_err()
                .to_string()
                .contains("installation in progress")
        );
        ensure(&ctx).unwrap();
        assert!(!lock_path(&root).exists());
        let holder = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path(&root))
            .unwrap();
        holder.lock().unwrap();
        assert!(
            start(&ctx)
                .unwrap_err()
                .to_string()
                .contains("no running ticker is confirmed")
        );
        assert!(!stop_path(&root).exists());
    }

    #[test]
    fn install_does_not_start_another_ticker_or_wait_behind_a_new_holder() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
        let config = home.path().join("cfg");
        let _install = crate::harness::lock(&config).unwrap();
        let mut holder = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path(&root))
            .unwrap();
        holder.lock().unwrap();
        holder
            .write_all(format!(r#"{{"version":"{}","pid":4321}}"#, crate::VERSION).as_bytes())
            .unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &env,
            root: root.clone(),
            config_dir: config,
            runner: &runner,
            detached_ticker: true,
        };
        start(&ctx).unwrap();
        ensure(&ctx).unwrap();
        assert!(!stop_path(&root).exists());
        std::fs::write(stop_path(&root), b"").unwrap();
        start_for_install(&ctx).unwrap();
        assert!(!stop_path(&root).exists());
        assert!(matches!(lock_state(&root), LockState::Held(info) if info.pid == 4321));
    }

    const AGENT_READY: &str = r#"{"result":{"agents":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","name":"hp-demo-coordinator","agent":"claude","agent_status":"idle","cwd":"CWD"}]}}"#;
    const AGENT_BLOCKED: &str = r#"{"result":{"agents":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","name":"hp-demo-coordinator","agent":"claude","agent_status":"blocked","cwd":"CWD"}]}}"#;
    const NO_AGENTS: &str = r#"{"result":{"agents":[]}}"#;
    const PANE: &str = r#"{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"CWD"}]}}"#;

    struct Fixture {
        _home: tempfile::TempDir,
        env: Env,
        root: PathBuf,
        project: Project,
    }

    fn fixture(pending: bool) -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        std::fs::create_dir(project.state_dir().join("tasks")).unwrap();
        let socket = home.path().join("herdr.sock");
        std::fs::write(&socket, b"").unwrap();
        let cwd = project.dir().to_string_lossy().into_owned();
        project
            .update_coordinator(|c| {
                c.socket = socket.to_string_lossy().into_owned();
                c.workspace_id = "w1".into();
                c.tab_id = "w1:t1".into();
                c.pane_id = "w1:p1".into();
                c.agent_name = "hp-demo-coordinator".into();
                c.cwd = cwd;
                c.prime_pending = pending;
                c.server_socket_inode = socket_inode(&socket);
            })
            .unwrap();
        let env = Env::for_test(home.path(), &[]);
        Fixture {
            _home: home,
            env,
            root,
            project,
        }
    }

    fn with_cwd(json: &str, fixture: &Fixture) -> String {
        json.replace("CWD", &fixture.project.dir().to_string_lossy())
    }

    #[test]
    fn failed_local_lane_reclaims_unnamed_agent_only_in_its_worktree() {
        let f = fixture(false);
        let lane = thread::allocate(&f.project, |t| {
            t.status = thread::Status::Failed;
            t.attempt = 3;
            t.launch_attempts = 1;
            t.error = "agent_not_ready: still blocked at the end of its ready window".into();
            t.launch.kind = "claude".into();
            t.agent_name = "hp-demo-t-0001".into();
            t.worktree_path = "/work/lane".into();
            t.cwd = t.worktree_path.clone();
            t.pane_id = "w1:p2".into();
            t.tab_id = "w1:t2".into();
            t.workspace_id = "w1".into();
        })
        .unwrap();
        assert_eq!(open_threads(&f.project, false).len(), 1);
        assert!(open_threads(&f.project, true).is_empty());
        let pane = Pane {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: lane.cwd.clone(),
        };
        let mut agent = Agent {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: "/elsewhere".into(),
            agent: "claude".into(),
            agent_status: "working".into(),
            ..Default::default()
        };
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "test.sock", &runner);
        let inspect = |agent: &Agent| {
            let current = thread::load(&f.project, &lane.id).unwrap();
            thread_pass(
                &LaunchPass {
                    ctx: &ctx,
                    project: &f.project,
                    herdr: &herdr,
                    threads: &[current],
                    agents: std::slice::from_ref(agent),
                    panes: std::slice::from_ref(&pane),
                },
                "ha",
                None,
                false,
                None,
            )
            .unwrap();
            thread::load(&f.project, &lane.id).unwrap()
        };
        assert_eq!(inspect(&agent).status, thread::Status::Failed);
        agent.cwd = lane.worktree_path.clone();
        let revived = inspect(&agent);
        assert_eq!(revived.status, thread::Status::Open);
        assert_eq!(revived.agent_name, "");
        assert!(revived.error.is_empty());
        assert!(
            revived.prompt_pending,
            "the undelivered brief must be retried"
        );
        assert_eq!(revived.attempt, 3);
        assert_eq!(revived.launch_attempts, 1);
        assert_eq!(revived.last_group, "working");
    }

    fn nudge_context<'a>(f: &'a Fixture, runner: &'a FakeRunner) -> Ctx<'a> {
        Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner,
            detached_ticker: false,
        }
    }

    fn nudge_pass(f: &Fixture, runner: &FakeRunner, agents: &[Agent]) {
        let herdr = Herdr::new(
            f.env.herdr_bin(),
            &f.project.coordinator().unwrap().socket,
            runner,
        );
        goal_check_nudge(&nudge_context(f, runner), &f.project, &herdr, agents).unwrap();
        steps::flush_notices_for_test(&f.project, &herdr);
    }

    fn nudge_setup() -> (Fixture, FakeRunner, Agent) {
        let f = fixture(false);
        let runner = FakeRunner::new();
        runner.on("agent list", ok(&with_cwd(AGENT_READY, &f)));
        runner.on("pane read", ok("❯ \n"));
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let c = f.project.coordinator().unwrap();
        let agent = Agent {
            pane_id: c.pane_id,
            tab_id: c.tab_id,
            workspace_id: c.workspace_id,
            cwd: c.cwd,
            name: c.agent_name,
            agent_status: "idle".into(),
            ..Default::default()
        };
        (f, runner, agent)
    }

    #[test]
    fn goal_check_and_notice_delivery_hold_only_the_coordinators_failed_dependency() {
        let (f, runner, agent) = nudge_setup();
        runner.on_fn(|cmd| cmd.program == "claude", |_| Ok(ok("OK")));
        f.project
            .update_coordinator(|c| c.launch.kind = "claude".into())
            .unwrap();
        let launch = f.project.coordinator().unwrap().launch;
        let mut evidence = crate::adapters::DependencyEvidence {
            kind: "quota".into(),
            reset_at: Some(
                "2099-01-01T00:00:00Z"
                    .parse::<jiff::Timestamp>()
                    .unwrap()
                    .as_second(),
            ),
        };
        crate::adapters::observe_dependency(
            &f.root,
            crate::contracts::MACHINE_LOCAL,
            &launch,
            "Usage limit reached",
            &evidence,
        )
        .unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        assert_eq!(runner.count("agent prompt"), 0);
        assert!(
            steps::goal_check::notice(&f.project).is_some(),
            "diagnosis remains owed, not consumed by the outage"
        );
        evidence.reset_at = Some(0);
        crate::adapters::observe_dependency(
            &f.root,
            crate::contracts::MACHINE_LOCAL,
            &launch,
            "Usage limit reached",
            &evidence,
        )
        .unwrap();
        crate::adapters::expire_dependency_probe(&f.root, crate::contracts::MACHINE_LOCAL, &launch);
        for _ in 0..2 {
            nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        }
        assert_eq!(runner.count("agent prompt"), 1);
        assert_eq!(
            runner
                .calls
                .borrow()
                .iter()
                .filter(|cmd| cmd.program == "claude")
                .count(),
            1
        );
        assert_eq!(runner.count("agent start"), 0);
    }

    #[test]
    fn shared_dependency_recovery_releases_an_already_queued_coordinator_goal() {
        use crate::scenarios::{World, agent_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        project
            .update_coordinator(|c| c.launch.kind = "claude".into())
            .unwrap();
        let c = project.coordinator().unwrap();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json(
                &c.workspace_id,
                &c.tab_id,
                &c.pane_id,
                &c.cwd,
                &c.agent_name,
                "idle"
            )
        );
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        world.runner.on("pane read", ok("❯ \n"));
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let ctx = world.ctx();
        let herdr = Herdr::new("herdr", &c.socket, &world.runner);
        goal_check_nudge(&ctx, &project, &herdr, &herdr.agent_list().unwrap()).unwrap();
        assert!(
            steps::goal_check::notice(&project).is_none(),
            "goal is already queued"
        );
        assert_eq!(world.runner.count("agent prompt"), 0);
        crate::adapters::observe_dependency(
            &world.root,
            crate::contracts::MACHINE_LOCAL,
            &c.launch,
            "Usage limit reached",
            &crate::adapters::DependencyEvidence {
                kind: "quota".into(),
                reset_at: None,
            },
        )
        .unwrap();
        tick_cheap(&ctx, &project, false).unwrap();
        steps::flush_notices_for_test(&project, &herdr);
        assert_eq!(world.runner.count("agent prompt"), 0);
        crate::adapters::expire_dependency_probe(
            &world.root,
            crate::contracts::MACHINE_LOCAL,
            &c.launch,
        );
        tick_cheap(&ctx, &project, false).unwrap();
        for _ in 0..2 {
            steps::flush_notices_for_test(&project, &herdr);
        }
        assert_eq!(world.runner.count("agent prompt"), 1);
        assert_eq!(
            world
                .runner
                .calls
                .borrow()
                .iter()
                .filter(|cmd| cmd.program == "claude")
                .count(),
            1
        );
        assert_eq!(world.runner.count("agent start"), 0);
    }

    #[test]
    fn goal_check_allows_independent_work_during_review_but_not_closed_unbound_or_busy() {
        let (f, runner, mut agent) = nudge_setup();
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        crate::plan::step_add(&ctx, "demo", "Outcome", vec![], vec![], None).unwrap();
        agent.agent_status = "busy".into();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        agent.agent_status = "idle".into();
        let lane = thread::allocate(&f.project, |t| {
            t.role = "worker".into();
            t.status = thread::Status::Open;
        })
        .unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        thread::update(&f.project, &lane.id, |t| {
            t.status = thread::Status::Resolved
        })
        .unwrap();
        assert_eq!(runner.count("agent prompt"), 1);
        let review = crate::review::Review {
            id: "review-1".into(),
            repo: String::new(),
            integration: String::new(),
            base: String::new(),
            candidate_branch: String::new(),
            members: vec![],
            gates: vec![],
            gates_note: String::new(),
            selected_gates: vec![],
            reviewer: None,
            phase: crate::review::Phase::Reviewing,
            verdict: None,
            verdict_event: String::new(),
            reviewer_after: String::new(),
            checked_event: String::new(),
            retry_attempt: None,
            retry_generation: 0,
            moved: 0,
            refresh_tip: None,
            push_remote: None,
            install_required: false,
            fast_forward: false,
            merged_at: String::new(),
            installed_at: String::new(),
            push: false,
            install: false,
            install_result: String::new(),
            close: false,
            prune: false,
            attention: String::new(),
            no_verdict_since: String::new(),
            notices: Vec::new(),
        };
        std::fs::create_dir_all(crate::review::dir(&f.project)).unwrap();
        std::fs::write(
            crate::review::path(&f.project, "review-1"),
            toml::to_string(&review).unwrap(),
        )
        .unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        assert_eq!(
            runner.count("agent prompt"),
            1,
            "review start owes no check"
        );
        crate::prompt::record_test_request(
            &f.project,
            "q-independent",
            "Do the independent work too",
        )
        .unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        assert_eq!(
            runner.count("agent prompt"),
            2,
            "new human work still wakes during review"
        );
        std::fs::remove_file(crate::review::path(&f.project, "review-1")).unwrap();
        // An unbound project does not start a coordinator or get a prompt.
        let unbound = project::create(&f.root, "unbound", "", vec![]).unwrap();
        crate::plan::step_add(&ctx, "unbound", "Outcome", vec![], vec![], None).unwrap();
        let herdr = Herdr::new(
            f.env.herdr_bin(),
            &f.project.coordinator().unwrap().socket,
            &runner,
        );
        goal_check_nudge(&ctx, &unbound, &herdr, &[agent.clone()]).unwrap();
        f.project
            .update_coordinator(|c| c.closed_by_rolf_at = project::now())
            .unwrap();
        nudge_pass(&f, &runner, &[agent]);
        assert_eq!(runner.count("agent prompt"), 2);
        assert_eq!(runner.count("agent start"), 0);
    }

    #[test]
    fn goal_check_outbox_preserves_drafts_deduplicates_restarts_and_drops_consumed_wakes() {
        let (f, _, agent) = nudge_setup();
        let runner = FakeRunner::new();
        runner.on("agent list", ok(&with_cwd(AGENT_READY, &f)));
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let screen = std::rc::Rc::new(std::cell::RefCell::new(
            "❯ Rolf's unfinished draft\n".to_string(),
        ));
        let visible = screen.clone();
        runner.on_fn(
            |cmd| cmd.display().contains("pane read"),
            move |_| Ok(ok(&visible.borrow())),
        );
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        assert_eq!(runner.count("agent prompt"), 0);
        assert!(steps::goal_check::load(&f.project).disposition.is_none());
        *screen.borrow_mut() = "❯ \n".into();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        for _ in 0..4 {
            nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        }
        assert_eq!(runner.count("agent prompt"), 1);
        assert!(
            steps::goal_check::load(&f.project).disposition.is_none(),
            "submission is not progress"
        );
        crate::prompt::record_test_request(&f.project, "q-new", "Reconsider the outcome").unwrap();
        let c = f.project.coordinator().unwrap();
        let herdr = Herdr::new(f.env.herdr_bin(), &c.socket, &runner);
        goal_check_nudge(
            &nudge_context(&f, &runner),
            &f.project,
            &herdr,
            std::slice::from_ref(&agent),
        )
        .unwrap();
        f.project
            .update_coordinator(|c| c.closed_by_rolf_at = project::now())
            .unwrap();
        steps::flush_notices_for_test(&f.project, &herdr);
        assert_eq!(
            runner.count("agent prompt"),
            1,
            "never wake a deliberately closed coordinator"
        );
        f.project
            .update_coordinator(|c| c.closed_by_rolf_at.clear())
            .unwrap();
        steps::goal_check::record(
            &f.project,
            steps::goal_check::Disposition::Wait {
                tasks: vec![],
                party: "Rolf".into(),
                condition: "reply to the consequential choice".into(),
            },
            "The choice blocks authorized work",
        )
        .unwrap();
        steps::flush_notices_for_test(&f.project, &herdr);
        assert_eq!(
            runner.count("agent prompt"),
            1,
            "consumed checks do not leak through the batch"
        );
        let batch: serde_json::Value =
            project::read_json(&f.project.state_dir().join("notice-batch.json")).unwrap();
        assert_eq!(batch["entries"], serde_json::json!([]));
    }

    #[test]
    fn replacement_binding_reparents_live_local_lane_on_next_pass() {
        let f = fixture(false);
        let lane = thread::allocate(&f.project, |t| {
            t.status = thread::Status::Open;
            t.pane_id = "w1:p2".into();
            t.tab_id = "w1:t2".into();
            t.workspace_id = "w1".into();
            t.agent = "claude".into();
            t.cwd = f.project.dir().to_string_lossy().into_owned();
            t.identity.process = Some(crate::contracts::ProcessIdentity {
                pid: 42,
                argv0: "claude".into(),
            });
        })
        .unwrap();
        thread::update(&f.project, &lane.id, |t| {
            let agent = crate::herdr::Agent {
                pane_id: t.pane_id.clone(),
                tab_id: t.tab_id.clone(),
                workspace_id: t.workspace_id.clone(),
                ..Default::default()
            };
            thread::bind_identity(
                t,
                &f.project.coordinator().unwrap().socket,
                &agent,
                Some(crate::contracts::ProcessIdentity {
                    pid: 42,
                    argv0: "claude".into(),
                }),
            );
        })
        .unwrap();
        let mut state = steps::load_state(&f.project);
        state.lanes_parented_to = "w1:p1".into();
        steps::save_state(&f.project, &state).unwrap();
        f.project
            .update_coordinator(|c| {
                c.pane_id = "w1:p9".into();
                c.tab_id = "w1:t9".into();
            })
            .unwrap();
        let runner = FakeRunner::new();
        runner.on("agent list", ok(&with_cwd(r#"{"result":{"agents":[{"pane_id":"w1:p9","tab_id":"w1:t9","workspace_id":"w1","name":"hp-demo-coordinator","agent":"claude","agent_status":"idle"},{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","name":"","cwd":"CWD","agent":"claude","agent_status":"idle","tokens":{"parent":"w1:p1"}}]}}"#, &f)));
        runner.on("pane list", ok(&with_cwd(r#"{"result":{"panes":[{"pane_id":"w1:p9","tab_id":"w1:t9","workspace_id":"w1","cwd":"CWD"},{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","cwd":"CWD"}]}}"#, &f)));
        runner.on("pane process-info", ok(r#"{"result":{"process_info":{"foreground_processes":[{"pid":42,"name":"claude","argv0":"claude"}]}}}"#));
        runner.on("pane report-metadata", ok(r#"{"result":{}}"#));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let _ = tick_cheap(&ctx, &f.project, false);
        assert_eq!(
            runner.count("pane report-metadata w1:p2 --source herdr-ade --token parent=w1:p9"),
            1,
            "{}",
            lane.id
        );
        assert_eq!(steps::load_state(&f.project).lanes_parented_to, "w1:p9");
    }

    #[test]
    fn changing_global_config_does_not_broadcast_project_inbox_items() {
        let f = fixture(false);
        let runner = FakeRunner::new();
        runner.on("agent list", ok(NO_AGENTS));
        runner.on("pane list", ok(&with_cwd(PANE, &f)));
        runner.on("workspace report-metadata", ok(r#"{"result":{}}"#));
        let config_dir = f.root.join("cfg");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), "").unwrap();
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: config_dir.clone(),
            runner: &runner,
            detached_ticker: false,
        };
        let mut memory = Memory::new(&ctx);
        assert!(tick_for_test(&ctx, &mut memory));
        std::fs::write(
            config_dir.join("config.toml"),
            "[doctor]\nmin_free_disk_gb = 13\n",
        )
        .unwrap();
        assert!(tick_for_test(&ctx, &mut memory));
        assert!(
            !crate::inbox::unhandled(&f.project)
                .iter()
                .any(|item| item.kind == "config-changed")
        );
    }

    #[test]
    fn first_pass_sweeps_lanes_sealed_before_install_without_relaunching() {
        use crate::scenarios::{World, pane_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("lane");
        std::fs::create_dir_all(&cwd).unwrap();
        let lane = world.thread(&project, &cwd, |t| {
            t.attempt = 1;
            t.launch_attempts = 1;
            t.bootstrap = "acknowledged".into();
        });
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            pane_json("w2", "w2:t1", "w2:p1", &lane.cwd)
        );
        let event = crate::contracts::Event {
            id: format!("{}-1-1", lane.id),
            usage: None,
            op: "done".into(),
            thread: lane.id.clone(),
            attempt: 1,
            recipient: crate::contracts::Recipient::default(),
            created: project::now(),
            payload: crate::contracts::EventPayload {
                done: Some(crate::contracts::DonePayload {
                    has_changes: None,
                    sha: "abc".into(),
                    artifact: "report".into(),
                    report_path: lane.report_path(),
                    attestation: None,
                    published_ref: None,
                }),
                ..Default::default()
            },
        };
        crate::events::seal_create_if_absent(&project, &event).unwrap();
        crate::events::append_delivery(
            &project,
            &event.id,
            crate::contracts::DeliveryState::Submitted,
        )
        .unwrap();
        // A second project with an already-sealed lane shares this ticker pass.
        let second = world.project("other", "b.sock");
        let other = world.thread(&second, &cwd, |t| {
            t.attempt = 1;
            t.launch_attempts = 1;
            t.bootstrap = "acknowledged".into();
        });
        let mut second_event = event.clone();
        second_event.id = format!("{}-1-1", other.id);
        second_event.thread = other.id.clone();
        crate::events::seal_create_if_absent(&second, &second_event).unwrap();
        crate::events::append_delivery(
            &second,
            &second_event.id,
            crate::contracts::DeliveryState::Submitted,
        )
        .unwrap();
        // This pane was already closed by Rolf after sealing. It must not
        // become a failed start just because no cached report hash exists.
        let closed = world.project("closed", "c.sock");
        let missing = world.thread(&closed, &cwd, |t| {
            t.attempt = 1;
            t.launch_attempts = 1;
            t.pane_id = "w3:p1".into();
            t.tab_id = "w3:t1".into();
            t.workspace_id = "w3".into();
        });
        crate::events::seal_create_if_absent(&closed, &event).unwrap();
        crate::events::append_delivery(
            &closed,
            &event.id,
            crate::contracts::DeliveryState::Submitted,
        )
        .unwrap();
        world
            .panes
            .borrow_mut()
            .insert_str(1, &format!("{},", world.coordinator_pane(&closed)));
        let ctx = world.ctx();
        assert_eq!(
            crate::threads::rows(&ctx, &closed)[0].group,
            thread::Group::Parked
        );
        let mut memory = Memory::new(&ctx);
        // All records predate this binary; no new seal or manual command is needed.
        assert!(tick_for_test(&ctx, &mut memory));
        assert!(thread::load(&project, &lane.id).unwrap().parked);
        assert!(thread::load(&second, &other.id).unwrap().parked);
        let closed_lane = thread::load(&closed, &missing.id).unwrap();
        assert!(closed_lane.parked);
        assert_ne!(
            closed_lane.failure_class,
            crate::contracts::FailureClass::ProcessGone
        );
        assert_eq!(world.runner.count("workspace close w2"), 2);
        assert_eq!(world.runner.count("workspace close w3"), 0);
        assert!(tick_for_test(&ctx, &mut memory));
        assert_eq!(world.runner.count("workspace close w2"), 2);
        assert_eq!(world.runner.count("agent start"), 0);
    }

    #[test]
    fn thousands_of_settled_lanes_do_not_get_opened_each_pass() {
        use crate::contracts::{DonePayload, Event, EventPayload, Recipient, WaitingPayload};
        let f = fixture(false);
        let thread_dir = thread::threads_dir_for_write(&f.project).unwrap();
        let event_dir = f.project.record_dir_for_write("events").unwrap();
        for number in 1..=3004 {
            let id = format!("t-{number:04}");
            let lane = thread::Thread {
                id: id.clone(),
                status: if number <= 3000 {
                    thread::Status::Resolved
                } else {
                    thread::Status::Starting
                },
                created: project::now(),
                ..Default::default()
            };
            std::fs::write(
                thread_dir.join(format!("{id}.toml")),
                toml::to_string(&lane).unwrap(),
            )
            .unwrap();
            if number <= 3000 {
                let event_id = format!("{id}-1-1");
                let event = Event {
                    id: event_id.clone(),
                    op: event_id.clone(),
                    thread: id,
                    attempt: 1,
                    recipient: Recipient {
                        pane: "w1:p1".into(),
                        coordinator_attempt: 1,
                    },
                    created: project::now(),
                    usage: None,
                    payload: if number % 2 == 0 {
                        EventPayload {
                            done: Some(DonePayload::default()),
                            ..Default::default()
                        }
                    } else {
                        EventPayload {
                            failed: Some(WaitingPayload::default()),
                            ..Default::default()
                        }
                    },
                };
                std::fs::write(
                    event_dir.join(format!("{event_id}.toml")),
                    crate::events::bytes(&event).unwrap(),
                )
                .unwrap();
            }
        }
        // Measure the old invalidated-thread-snapshot and per-lane full-log
        // lookup pattern independently of other ticker work.
        let before_threads = thread::count_thread_reads(|| {
            assert_eq!(thread::list_with_errors(&f.project).0.len(), 3004);
        });
        let before_events = crate::events::count_event_reads(|| {
            for _ in 0..4 {
                assert_eq!(crate::events::list(&f.project).len(), 3000);
            }
        });
        let runner = FakeRunner::new();
        runner.on("agent list", ok(NO_AGENTS));
        runner.on("pane list", ok(&with_cwd(PANE, &f)));
        runner.on("workspace report-metadata", ok(r#"{"result":{}}"#));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let mut memory = Memory::new(&ctx);
        let _cache = crate::record_cache::Cache::new();
        assert!(tick_for_test(&ctx, &mut memory));
        let measure = |memory: &mut Memory| {
            let mut event_reads = 0;
            let thread_reads = thread::count_thread_reads(|| {
                event_reads = crate::events::count_event_reads(|| {
                    assert!(tick_for_test(&ctx, memory));
                    assert_eq!(thread::list_live(&f.project).len(), 4);
                    for number in 3001..=3004 {
                        let lane = thread::Thread {
                            id: format!("t-{number:04}"),
                            ..Default::default()
                        };
                        assert!(crate::events::for_thread(&f.project, &lane.id).is_empty());
                        restore_unchanged_seal(&ctx, &f.project, &lane).unwrap();
                        assert!(
                            crate::threads::report_artifact_stored(&f.project, &lane)
                                .is_ok_and(|stored| !stored)
                        );
                    }
                });
            });
            (thread_reads, event_reads)
        };
        let idle = measure(&mut memory);
        assert_eq!(idle, (0, 0));
        for number in 3001..=3004 {
            thread::update(&f.project, &format!("t-{number:04}"), |t| {
                t.title = "changed".into()
            })
            .unwrap();
        }
        let changed = measure(&mut memory);
        // Four changed live records are read by the work snapshot and by the
        // stamp-only health validator; settled history is not opened again.
        assert_eq!(changed, (8, 0));
        println!(
            "3000 settled + 4 live: old scan pattern {before_threads} thread / {before_events} event opens; warm full pass {idle:?}; four changed live records {changed:?}"
        );
    }

    #[test]
    fn idle_pass_only_polls_live_herdr_state() {
        let f = fixture(false);
        let runner = FakeRunner::new();
        runner.on("agent list", ok(NO_AGENTS));
        runner.on("pane list", ok(&with_cwd(PANE, &f)));
        runner.on("workspace report-metadata", ok(r#"{"result":{}}"#));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let mut memory = Memory::new(&ctx);
        assert!(tick_for_test(&ctx, &mut memory));
        let (agents, panes, metadata) = (
            runner.count("agent list"),
            runner.count("pane list"),
            runner.count("report-metadata"),
        );
        assert!(tick_for_test(&ctx, &mut memory));
        assert_eq!(runner.count("agent list") - agents, 1);
        assert_eq!(runner.count("pane list") - panes, 1);
        assert_eq!(runner.count("report-metadata"), metadata);
        assert_eq!(runner.count("git"), 0);
    }

    #[test]
    fn parked_local_pi_reopens_with_brief_then_ordered_follow_ups() {
        parked_pi_reopens_with_brief_then_ordered_follow_ups(false, false, false, None);
    }

    #[test]
    fn parked_box_pi_reopens_with_brief_then_ordered_follow_ups() {
        parked_pi_reopens_with_brief_then_ordered_follow_ups(true, false, false, None);
    }

    #[test]
    fn resumed_local_and_box_pi_deliver_the_reopening_follow_up_and_record_later_delivery() {
        for (remote, late) in [(false, false), (true, false), (false, true)] {
            parked_pi_reopens_with_brief_then_ordered_follow_ups(remote, true, late, None);
        }
        for remote in [false, true] {
            for timing in ["delayed", "delayed-unobserved"] {
                parked_pi_reopens_with_brief_then_ordered_follow_ups(
                    remote,
                    true,
                    false,
                    Some(timing),
                );
            }
        }
        parked_pi_reopens_with_brief_then_ordered_follow_ups(
            true,
            true,
            false,
            Some("session-unobserved"),
        );
    }

    #[test]
    fn parked_pi_missing_session_restarts_locally_and_on_box() {
        for remote in [false, true] {
            for timing in ["missing", "disappeared"] {
                parked_pi_reopens_with_brief_then_ordered_follow_ups(
                    remote,
                    timing == "disappeared",
                    false,
                    Some(timing),
                );
            }
        }
    }

    #[test]
    fn parked_pi_follow_up_survives_gone_and_manual_retry_in_first_prompt() {
        for (remote, recovery) in [
            (true, "gone"),
            (true, "uncertain"),
            (true, "gone-timeout"),
            (true, "manual"),
            (false, "manual"),
        ] {
            parked_pi_reopens_with_brief_then_ordered_follow_ups(
                remote,
                true,
                false,
                Some(recovery),
            );
        }
    }

    fn parked_pi_reopens_with_brief_then_ordered_follow_ups(
        remote: bool,
        mut resuming: bool,
        late: bool,
        recovery: Option<&'static str>,
    ) {
        use crate::scenarios::World;
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let folder = world.home.path().join("lane");
        std::fs::create_dir_all(&folder).unwrap();
        let session = folder.join("saved-session.jsonl");
        // A Mac-side file must not authorize a resume on a box that lacks it.
        if remote || recovery != Some("missing") {
            std::fs::write(
                &session,
                b"{\"type\":\"session\",\"id\":\"saved-session\"}\n",
            )
            .unwrap();
        }
        let session_reply =
            std::rc::Rc::new(std::cell::Cell::new(if recovery == Some("missing") {
                "missing"
            } else {
                "present"
            }));
        if remote {
            let config = world.home.path().join("cfg/config.toml");
            let existing = std::fs::read_to_string(&config).unwrap();
            std::fs::write(
                config,
                format!(
                    "{existing}{}\n[[machines.buildbox.repos]]\npath = \"/repo\"\nbox_path = \"/box/repo\"\npublish_url = \"https://example.com/repo.git\"\n",
                    crate::remote::TEST_MACHINE
                ),
            )
            .unwrap();
            world.runner.on("machine list --json", ok("[]"));
            let reply = session_reply.clone();
            world.runner.on_fn(
                |cmd| cmd.program == "ssh",
                move |cmd| {
                    Ok(if crate::box_helper::tests::is_doctor(cmd) {
                        crate::testkit::diagnostic_output(cmd, 99_999_999, None)
                    } else if cmd.display().contains("if test -f") {
                        if reply.get() == "unreachable" {
                            fail(255, "ssh: Connection timed out")
                        } else {
                            ok(reply.get())
                        }
                    } else {
                        ok("")
                    })
                },
            );
        }
        let brief_hash = thread::store_artifact(&project, b"frozen brief").unwrap();
        let lane = world.thread(&project, &folder, |t| {
            t.attempt = 2;
            t.parked = true;
            t.prompt_pending = false;
            t.bootstrap = "acknowledged".into();
            t.launch.kind = "pi".into();
            t.launch.recipe_id = "pi_codex_astra_high".into();
            t.launch.brief_hash = brief_hash.clone();
            t.identity.workspace_id = t.workspace_id.clone();
            t.identity.tab_id = t.tab_id.clone();
            t.identity.pane_id = t.pane_id.clone();
            t.identity.process = Some(crate::contracts::ProcessIdentity {
                pid: 42,
                argv0: "pi".into(),
            });
            t.agent = "pi".into();
            if resuming || recovery == Some("missing") {
                let reported: Agent = serde_json::from_value(serde_json::json!({
                    "pane_id": t.pane_id, "tab_id": t.tab_id, "workspace_id": t.workspace_id,
                    "agent_session": {"agent":"pi", "kind":"path", "value":session, "source":"herdr:pi"}
                })).unwrap();
                t.identity.agent_session = Some(reported.agent_session.unwrap().id);
            }
            if remote {
                t.machine = "buildbox".into();
                t.machine_id = "buildbox".into();
            }
        });
        std::fs::write(thread::task_path(&project, &lane.id), "frozen brief").unwrap();
        let seal = format!("{}-{}-1", lane.id, lane.attempt);
        crate::events::seal_create_if_absent(
            &project,
            &crate::contracts::Event {
                id: seal.clone(),
                op: seal,
                thread: lane.id.clone(),
                attempt: lane.attempt,
                recipient: Default::default(),
                created: project::now(),
                usage: None,
                payload: crate::contracts::EventPayload {
                    done: Some(crate::contracts::DonePayload {
                        report_path: lane.report_path(),
                        artifact: thread::store_artifact(&project, b"sealed report").unwrap(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            },
        )
        .unwrap();
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        world
            .runner
            .on("workspace list", ok(r#"{"result":{"workspaces":[]}}"#));
        let created = ok(
            r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}}"#,
        );
        world.runner.on("tab create", created.clone());
        world.runner.on("workspace create", created);
        world.runner.on("tab rename", ok(r#"{"result":{}}"#));
        let agent = Agent {
            pane_id: "w1:p2".into(),
            tab_id: "w1:t2".into(),
            workspace_id: "w1".into(),
            name: lane.agent_name.clone(),
            agent: "pi".into(),
            agent_status: "idle".into(),
            cwd: lane.cwd.clone(),
            ..Agent::default()
        };
        let pane = Pane {
            pane_id: agent.pane_id.clone(),
            tab_id: agent.tab_id.clone(),
            workspace_id: agent.workspace_id.clone(),
            cwd: agent.cwd.clone(),
        };
        world.runner.on(
            "agent start",
            ok(&format!(
                r#"{{"result":{{"agent":{}}}}}"#,
                serde_json::json!({
                    "pane_id": agent.pane_id,
                    "tab_id": agent.tab_id,
                    "workspace_id": agent.workspace_id,
                    "name": agent.name,
                    "agent": "pi",
                    "agent_status": "idle",
                    "cwd": agent.cwd,
                })
            )),
        );
        world.runner.on(
            "pane cwd",
            ok(&format!(r#"{{"result":{{"cwd":"{}"}}}}"#, lane.cwd)),
        );
        world.runner.on("pane parent", ok(r#"{"result":{}}"#));
        world.runner.on(
            "pane process-info",
            if recovery == Some("delayed-unobserved") {
                fail(1, r#"{"error":{"code":"timeout","message":"process observation unavailable"}}"#)
            } else {
                ok(r#"{"result":{"process_info":{"pane_id":"w1:p2","foreground_processes":[{"pid":99,"name":"bash"}]}}}"#)
            },
        );
        let delivery_project = project.clone();
        let delivery_id = lane.id.clone();
        let refuse_first = std::cell::Cell::new(true);
        world.runner.on_fn(
            |cmd| cmd.display().contains("agent prompt"),
            move |cmd| {
                // A PTY submission alone is not proof that a new process has
                // consumed its first instruction. Require the activity gate.
                assert!(cmd.args.iter().any(|arg| arg == "--wait"));
                let saved = thread::load(&delivery_project, &delivery_id).unwrap();
                if recovery == Some("gone-timeout") && cmd.display().contains("skill lane") {
                    assert!(saved.follow_ups.iter().all(|f| f.state == thread::FollowUpState::Uncertain));
                    return Ok(fail(1, r#"{"error":{"code":"timeout","message":"activity unknown"}}"#));
                }
                if !cmd.display().contains("skill lane") {
                    let pending = saved
                        .follow_ups
                        .iter()
                        .find(|f| f.state == thread::FollowUpState::Uncertain)
                        .unwrap();
                    assert!(pending.delivered_at.is_empty());
                    assert!(cmd.args.iter().any(|arg| arg == &pending.text));
                    if refuse_first.replace(false) {
                        return Ok(fail(1, r#"{"error":{"code":"agent_not_ready","message":"new process is not ready yet"}}"#));
                    }
                }
                Ok(ok(r#"{"result":{}}"#))
            },
        );
        let ctx = world.ctx();
        for text in ["first correction", "second correction"] {
            assert!(matches!(
                crate::threads::prompt(&ctx, "demo", &lane.id, text).unwrap(),
                crate::threads::PromptOutcome::Queued { attempt: 2 }
            ));
        }
        let reopened = thread::load(&project, &lane.id).unwrap();
        assert!(!reopened.parked);
        assert!(reopened.recovery_pending);
        assert!(!reopened.prompt_pending);
        assert_eq!(reopened.bootstrap == "resuming", resuming);
        assert_eq!(world.runner.count("agent start"), 0);
        if remote {
            for _ in 0..2 {
                assert!(
                    observation_pass(
                        &ctx,
                        &project,
                        ObservationView {
                            machine_id: reopened.machine_route(),
                            threads: std::slice::from_ref(&reopened),
                            agents: &[],
                            panes: &[],
                            boot_id: "boot-1",
                            now: jiff::Timestamp::now(),
                        }
                    )
                    .is_empty()
                );
            }
            let pending = thread::load(&project, &lane.id).unwrap();
            assert_eq!(pending.attempt, 2);
            assert_eq!(
                pending.launch.same_recipe_retries,
                lane.launch.same_recipe_retries
            );
            assert!(
                pending
                    .start_notices
                    .iter()
                    .all(|n| !n.line.contains("GONE"))
            );
        }
        threads::place_recovery(&ctx, &project, &reopened).unwrap();
        let mut reopened = thread::update(&project, &lane.id, |t| {
            t.error = "provider ready".into();
            if matches!(
                recovery,
                Some("delayed" | "delayed-unobserved" | "session-unobserved")
            ) {
                t.startup_wait_started =
                    (jiff::Timestamp::now() - jiff::SignedDuration::from_secs(600)).to_string();
            }
        })
        .unwrap();
        assert!(!reopened.startup_wait_started.is_empty());
        assert!(
            reopened.identity.process.is_none(),
            "the parked process cannot prove death in a new pane"
        );
        if remote {
            for _ in 0..2 {
                assert!(
                    observation_pass(
                        &ctx,
                        &project,
                        ObservationView {
                            machine_id: reopened.machine_route(),
                            threads: std::slice::from_ref(&reopened),
                            agents: &[],
                            panes: std::slice::from_ref(&pane),
                            boot_id: "boot-1",
                            now: jiff::Timestamp::now(),
                        }
                    )
                    .is_empty()
                );
            }
            let placed = thread::load(&project, &lane.id).unwrap();
            assert_eq!(placed.attempt, 2);
            assert_eq!(
                placed.launch.same_recipe_retries,
                lane.launch.same_recipe_retries
            );
            assert!(
                placed
                    .start_notices
                    .iter()
                    .all(|n| !n.line.contains("GONE"))
            );
        }
        let retried = matches!(
            recovery,
            Some("gone" | "uncertain" | "gone-timeout" | "manual")
        );
        if retried {
            // The first reopen really loses its pane before submission. This
            // is an external death, not the missing-session fallback above.
            if recovery != Some("manual") {
                if recovery == Some("uncertain") {
                    reopened = thread::update(&project, &lane.id, |t| {
                        t.follow_ups[0].state = thread::FollowUpState::Uncertain
                    })
                    .unwrap();
                }
                for _ in 0..2 {
                    assert!(
                        observation_pass(
                            &ctx,
                            &project,
                            ObservationView {
                                machine_id: reopened.machine_route(),
                                threads: std::slice::from_ref(&reopened),
                                agents: &[],
                                panes: &[],
                                boot_id: "boot-1",
                                now: jiff::Timestamp::now() + jiff::SignedDuration::from_secs(600),
                            }
                        )
                        .is_empty()
                    );
                }
                assert!(
                    thread::load(&project, &lane.id)
                        .unwrap()
                        .start_notices
                        .iter()
                        .any(|n| n.line.contains("GONE"))
                );
            } else {
                thread::update(&project, &lane.id, |t| {
                    t.status = thread::Status::Failed;
                    t.startup_wait_started.clear();
                })
                .unwrap();
                threads::retry(&ctx, "demo", &lane.id, "replace the lost reopen").unwrap();
            }
            let pending = thread::load(&project, &lane.id).unwrap();
            assert_eq!(pending.attempt, lane.attempt + 1);
            assert!(
                pending
                    .follow_ups
                    .iter()
                    .all(|f| f.state == thread::FollowUpState::Queued
                        && f.attempt == pending.attempt
                        && f.carried_from_attempt == lane.attempt)
            );
            threads::place_recovery(&ctx, &project, &pending).unwrap();
            reopened =
                thread::update(&project, &lane.id, |t| t.error = "provider ready".into()).unwrap();
            resuming = false;
        }
        if recovery == Some("disappeared") {
            session_reply.set("missing");
            if !remote {
                std::fs::remove_file(&session).unwrap();
            }
            resuming = false;
        }
        let herdr =
            Herdr::new("herdr", "test.sock", &world.runner).on_machine(reopened.machine_route());
        let mut errors = Vec::new();
        let input = LaunchPass {
            ctx: &ctx,
            project: &project,
            herdr: &herdr,
            threads: std::slice::from_ref(&reopened),
            agents: &[],
            panes: std::slice::from_ref(&pane),
        };
        if remote {
            let view = steps::CourierOutcome {
                machine_id: reopened.machine_route().into(),
                boot_id: "boot-1".into(),
                agents: Some(vec![]),
                panes: Some(vec![pane.clone()]),
                progress: Default::default(),
            };
            if recovery == Some("session-unobserved") {
                session_reply.set("unreachable");
                remote_pass(
                    &input,
                    reopened.machine_route(),
                    &view,
                    &mut true,
                    &mut errors,
                )
                .unwrap();
                assert!(
                    errors.iter().any(|error| {
                        let message = format!("{error:#}");
                        message.contains("resume session")
                            && crate::remote::is_unreachable(&message)
                    }),
                    "{errors:?}"
                );
                let waiting = thread::load(&project, &lane.id).unwrap();
                assert_eq!(waiting.status, thread::Status::Open);
                assert_eq!(waiting.attempt, lane.attempt);
                assert_eq!(waiting.launch_attempts, 0);
                assert_eq!(
                    waiting.launch.same_recipe_retries,
                    lane.launch.same_recipe_retries
                );
                assert_eq!(waiting.bootstrap, "resuming");
                assert!(waiting.start_notices.is_empty());
                assert_eq!(world.runner.count("agent start"), 0);
                session_reply.set("present");
                errors.clear();
            }
            remote_pass(
                &input,
                reopened.machine_route(),
                &view,
                &mut true,
                &mut errors,
            )
            .unwrap();
        } else {
            errors.extend(thread_pass(&input, "ha", None, false, None).unwrap().error);
            launch_pass(&input, &mut true, true, &mut errors);
        }
        assert!(errors.is_empty(), "{errors:?}");
        let submitted = thread::load(&project, &lane.id).unwrap();
        assert_eq!(submitted.status, thread::Status::Open, "{submitted:?}");
        assert!(
            thread::in_start_window(&submitted, jiff::Timestamp::now()),
            "submission, not the old placement, starts the ready window"
        );
        assert_eq!(reopened.attempt, lane.attempt + u32::from(retried));
        assert_eq!(reopened.worktree_path, lane.worktree_path);
        let calls = world.runner.calls.borrow();
        let launch = calls
            .iter()
            .find(|c| c.display().contains("agent start") && !c.display().contains("--help"))
            .unwrap();
        let resume_flag = launch.args.iter().position(|arg| arg == "--session");
        assert_eq!(resume_flag.is_some(), resuming);
        if let Some(index) = resume_flag {
            assert_eq!(launch.args[index + 1], session.to_string_lossy());
            assert!(launch.args[..index].iter().any(|arg| arg == "--"));
        }
        drop(calls);
        assert_eq!(
            world.runner.count("agent start") - world.runner.count("agent start --help"),
            1
        );
        assert_eq!(world.runner.count("agent prompt"), 0);
        if remote {
            let calls = world.runner.calls.borrow();
            let card_call = calls
                .iter()
                .rfind(|call| {
                    call.program == "ssh"
                        && call
                            .stdin
                            .as_ref()
                            .is_some_and(|text| text.contains("start_line"))
                })
                .unwrap();
            let card: crate::contracts::LaneCard =
                toml::from_str(card_call.stdin.as_ref().unwrap()).unwrap();
            assert_eq!(card.pane_id, reopened.pane_id);
            assert_eq!(card.attempt, reopened.attempt);
            assert_eq!(card.brief_hash, brief_hash);
        }
        let herdr =
            Herdr::new("herdr", "test.sock", &world.runner).on_machine(reopened.machine_route());
        let run_pass = || {
            let pass = thread_pass(
                &LaunchPass {
                    ctx: &ctx,
                    project: &project,
                    herdr: &herdr,
                    threads: &[thread::load(&project, &lane.id).unwrap()],
                    agents: std::slice::from_ref(&agent),
                    panes: std::slice::from_ref(&pane),
                },
                "ha",
                None,
                false,
                None,
            )
            .unwrap();
            pass.error
        };
        if retried {
            assert!(run_pass().is_none());
            if recovery == Some("gone-timeout") {
                let staged = thread::load(&project, &lane.id).unwrap();
                assert!(
                    staged
                        .follow_ups
                        .iter()
                        .all(|f| f.state == thread::FollowUpState::Uncertain
                            && f.delivered_at.is_empty())
                );
                assert!(run_pass().is_none());
                assert_eq!(
                    world.runner.count("agent prompt"),
                    1,
                    "ambiguous initial prompt is not replayed"
                );
                thread::update(&project, &lane.id, |t| t.bootstrap = "acknowledged".into())
                    .unwrap();
                assert!(run_pass().is_none());
            }
            let saved = thread::load(&project, &lane.id).unwrap();
            assert!(
                saved
                    .follow_ups
                    .iter()
                    .all(|f| f.state == thread::FollowUpState::Delivered
                        && !f.delivered_at.is_empty())
            );
            let calls = world.runner.calls.borrow();
            let prompt = calls
                .iter()
                .find(|c| c.display().contains("agent prompt"))
                .unwrap()
                .display();
            assert!(prompt.contains("first correction"), "{prompt}");
            assert!(prompt.contains("second correction"), "{prompt}");
            assert!(prompt.contains("sealed report"), "{prompt}");
            assert_eq!(world.runner.count("agent prompt"), 1);
            return;
        }
        if late {
            startup_failure(
                &LaunchPass {
                    ctx: &ctx,
                    project: &project,
                    herdr: &herdr,
                    threads: &[],
                    agents: &[],
                    panes: std::slice::from_ref(&pane),
                },
                &thread::load(&project, &lane.id).unwrap(),
                "agent state unknown at the end of its ready window",
            )
            .unwrap();
            assert_eq!(
                thread::load(&project, &lane.id).unwrap().status,
                thread::Status::Failed
            );
            assert!(run_pass().is_none());
            let recovered = thread::load(&project, &lane.id).unwrap();
            assert_eq!(recovered.status, thread::Status::Open);
            assert!(recovered.startup_wait_started.is_empty());
            assert_eq!(recovered.bootstrap, "resuming");
            assert_eq!(world.runner.count("agent prompt"), 0);
        }
        if resuming {
            assert!(run_pass().is_none());
            assert_eq!(
                thread::load(&project, &lane.id).unwrap().bootstrap,
                "acknowledged"
            );
            assert_eq!(world.runner.count("agent prompt"), 0);
        } else {
            assert!(run_pass().is_none());
            assert_eq!(world.runner.count("agent prompt"), 1);
            // A transported brief still needs this process's skill receipt.
            assert!(run_pass().is_none());
            assert_eq!(world.runner.count("agent prompt"), 1);
            thread::update(&project, &lane.id, |t| t.bootstrap = "acknowledged".into()).unwrap();
        }
        // A ready snapshot can go stale just after a reopen. Preserve the
        // first instruction and do not let the later one overtake it.
        assert!(run_pass().is_some());
        let pending = thread::load(&project, &lane.id).unwrap();
        assert!(
            pending
                .follow_ups
                .iter()
                .all(|f| f.state == thread::FollowUpState::Queued)
        );
        assert!(run_pass().is_none());
        let expected_prompts = if resuming { 3 } else { 4 };
        let calls = world.runner.calls.borrow();
        let prompts: Vec<_> = calls
            .iter()
            .filter(|call| call.display().contains("agent prompt"))
            .map(|call| call.display())
            .collect();
        assert_eq!(prompts.len(), expected_prompts);
        if !resuming {
            assert!(prompts[0].contains("skill lane"));
            if remote {
                assert!(prompts[0].contains("/home/agent/.local/bin/herdr-ade"));
                assert!(prompts[0].contains("/home/agent/.herdr-ade"));
            } else {
                assert!(prompts[0].contains("`ha skill lane`"));
                assert!(!prompts[0].contains("/home/agent/.local/bin/herdr-ade"));
            }
            assert!(prompts[0].contains("brief.md"));
            assert!(prompts[expected_prompts - 2].contains(&lane.report_path()));
            assert!(
                prompts[expected_prompts - 2].contains(&format!("{}/brief.md", lane.thread_dir))
            );
        }
        assert!(prompts[expected_prompts - 2].contains("first correction"));
        assert!(prompts[expected_prompts - 1].contains("second correction"));
        drop(calls);
        let saved = thread::load(&project, &lane.id).unwrap();
        assert!(
            saved
                .follow_ups
                .iter()
                .all(|f| f.state == thread::FollowUpState::Delivered && !f.delivered_at.is_empty())
        );
        assert_eq!(saved.attempt, lane.attempt);
        assert_eq!(
            saved.launch.same_recipe_retries,
            lane.launch.same_recipe_retries
        );
        assert!(saved.start_notices.iter().all(|n| !n.line.contains("GONE")));
        if remote {
            assert!(
                observation_pass(
                    &ctx,
                    &project,
                    ObservationView {
                        machine_id: saved.machine_route(),
                        threads: std::slice::from_ref(&saved),
                        agents: std::slice::from_ref(&agent),
                        panes: std::slice::from_ref(&pane),
                        boot_id: "boot-1",
                        now: jiff::Timestamp::now() + jiff::SignedDuration::from_secs(600),
                    }
                )
                .is_empty()
            );
            let observed = thread::load(&project, &lane.id).unwrap();
            assert_eq!(observed.attempt, lane.attempt);
            assert!(
                observed
                    .start_notices
                    .iter()
                    .all(|n| !n.line.contains("GONE"))
            );
        }
        // A later message takes the immediate CLI path on this same reopened
        // pane. It must use the same durable delivery lifecycle as the queue.
        *world.agents.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::agent_json(
                "w1",
                "w1:t2",
                "w1:p2",
                &lane.cwd,
                &lane.agent_name,
                "idle"
            )
        );
        assert!(matches!(
            crate::threads::prompt(&ctx, "demo", &lane.id, "later correction").unwrap(),
            crate::threads::PromptOutcome::Sent { attempt: 2, .. }
        ));
        let saved = thread::load(&project, &lane.id).unwrap();
        assert_eq!(saved.follow_ups.len(), 3);
        assert!(
            saved
                .follow_ups
                .iter()
                .all(|f| f.state == thread::FollowUpState::Delivered && !f.delivered_at.is_empty())
        );
        assert!(run_pass().is_none());
        assert_eq!(world.runner.count("agent prompt"), expected_prompts + 1);
    }

    #[test]
    fn follow_ups_queued_during_start_arrive_after_the_brief_in_order() {
        queued_follow_ups_drain(false);
    }

    #[test]
    fn adopted_start_drains_follow_ups_without_a_bootstrap_receipt() {
        queued_follow_ups_drain(true);
    }

    fn queued_follow_ups_drain(adopted: bool) {
        let f = fixture(false);
        let runner = FakeRunner::new();
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        runner.on("report-metadata", ok(r#"{"result":{}}"#));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let lane = thread::allocate(&f.project, |lane| {
            lane.status = thread::Status::Open;
            lane.prompt_pending = true;
            if adopted {
                lane.kind = thread::Kind::Adopted;
            }
            lane.launch.ready_timeout_ms = 300_000;
            lane.workspace_id = "w1".into();
            lane.tab_id = "w1:t2".into();
            lane.pane_id = "w1:p2".into();
            lane.cwd = "/wt".into();
            lane.agent = "claude".into();
            lane.agent_name = "hp-demo-t-0001".into();
        })
        .unwrap();
        let seal_waiting = |sequence, text: &str| {
            use crate::contracts::{Event, EventPayload, Recipient, WaitingPayload};
            let event = Event {
                id: format!("{}-1-{sequence}", lane.id),
                usage: None,
                op: format!("test-{sequence}"),
                thread: lane.id.clone(),
                attempt: 1,
                recipient: Recipient::default(),
                created: format!("2026-09-21T00:00:0{sequence}Z"),
                payload: EventPayload {
                    waiting: Some(WaitingPayload {
                        text: text.into(),
                        ..WaitingPayload::default()
                    }),
                    ..EventPayload::default()
                },
            };
            let dir = crate::events::dir(&f.project);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(format!("{}.toml", event.id)),
                toml::to_string(&event).unwrap(),
            )
            .unwrap();
            event.id
        };
        let first_wait = seal_waiting(1, "First choice?");

        assert!(matches!(
            crate::threads::prompt(&ctx, "demo", &lane.id, "check the first gate").unwrap(),
            crate::threads::PromptOutcome::Queued { attempt: 1 }
        ));
        assert!(matches!(
            crate::threads::prompt(&ctx, "demo", &lane.id, "then check the second gate").unwrap(),
            crate::threads::PromptOutcome::Queued { attempt: 1 }
        ));
        assert_eq!(runner.count("agent prompt"), 0);
        assert_eq!(
            thread::load(&f.project, &lane.id).unwrap().follow_ups[0].waiting_event,
            first_wait
        );
        // This later wait was not present when either queued response was accepted.
        let second_wait = seal_waiting(2, "Second choice?");

        let agent = Agent {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            name: lane.agent_name.clone(),
            agent: lane.agent.clone(),
            agent_status: "idle".into(),
            cwd: lane.cwd.clone(),
            ..Agent::default()
        };
        let pane = Pane {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: lane.cwd.clone(),
        };
        let herdr = Herdr::new("herdr", "test.sock", &runner);
        let run_pass = || {
            thread_pass(
                &LaunchPass {
                    ctx: &ctx,
                    project: &f.project,
                    herdr: &herdr,
                    threads: &[thread::load(&f.project, &lane.id).unwrap()],
                    agents: std::slice::from_ref(&agent),
                    panes: std::slice::from_ref(&pane),
                },
                "ha",
                None,
                true,
                None,
            )
            .unwrap()
        };
        let pass = run_pass();
        assert!(pass.error.is_none());
        assert_eq!(runner.count("agent prompt"), 1);
        assert_eq!(
            thread::load(&f.project, &lane.id).unwrap().follow_ups.len(),
            2
        );

        // Transporting the brief is not enough. The matching skill receipt is
        // the ordering gate for follow-ups.
        if !adopted {
            thread::update(&f.project, &lane.id, |lane| {
                lane.bootstrap = "acknowledged".into()
            })
            .unwrap();
        }
        let pass = run_pass();
        assert!(pass.error.is_none());

        let calls = runner.calls.borrow();
        let prompts: Vec<_> = calls
            .iter()
            .filter(|call| call.display().contains("agent prompt"))
            .map(|call| call.display())
            .collect();
        assert_eq!(prompts.len(), 3);
        assert!(prompts.iter().all(|prompt| prompt.contains(&format!(
            "--timeout {}",
            crate::herdr::AGENT_START_TIMEOUT.as_millis()
        ))));
        assert_eq!(
            prompts[0].contains("skill lane"),
            !adopted,
            "{}",
            prompts[0]
        );
        assert!(
            prompts[1].contains("check the first gate"),
            "{}",
            prompts[1]
        );
        assert!(
            prompts[2].contains("then check the second gate"),
            "{}",
            prompts[2]
        );
        let saved = thread::load(&f.project, &lane.id).unwrap();
        assert!(!saved.prompt_pending);
        assert_eq!(saved.follow_ups.len(), 2);
        assert!(saved.follow_ups.iter().all(|follow_up| follow_up.state
            == thread::FollowUpState::Delivered
            && !follow_up.delivered_at.is_empty()));
        assert_eq!(saved.answered_waiting_event, first_wait);
        assert_ne!(saved.answered_waiting_event, second_wait);
        drop(calls);

        // Ambiguous transport is durable before the call and is never blindly
        // retried on a later pass.
        thread::update(&f.project, &lane.id, |lane| {
            lane.follow_ups.push(thread::FollowUp {
                attempt: 1,
                text: "an uncertain clarification".into(),
                state: thread::FollowUpState::Queued,
                ..thread::FollowUp::default()
            });
        })
        .unwrap();
        for response in [
            timeout(),
            fail(
                1,
                r#"{"error":{"code":"agent_prompt_failed","message":"PTY closed after input"}}"#,
            ),
            fail(
                1,
                r#"{"error":{"code":"agent_prompt_stalled","message":"input submitted but no activity observed"}}"#,
            ),
        ] {
            // Each independent scripted transport starts with a queued entry.
            thread::update(&f.project, &lane.id, |lane| {
                lane.follow_ups[2].state = thread::FollowUpState::Queued
            })
            .unwrap();
            let uncertain_runner = FakeRunner::new();
            uncertain_runner.on("agent prompt", response);
            uncertain_runner.on("report-metadata", ok(r#"{"result":{}}"#));
            let uncertain_ctx = Ctx {
                env: &f.env,
                root: f.root.clone(),
                config_dir: f.root.join("cfg"),
                runner: &uncertain_runner,
                detached_ticker: false,
            };
            let uncertain_herdr = Herdr::new("herdr", "test.sock", &uncertain_runner);
            let uncertain_pass = || {
                thread_pass(
                    &LaunchPass {
                        ctx: &uncertain_ctx,
                        project: &f.project,
                        herdr: &uncertain_herdr,
                        threads: &[thread::load(&f.project, &lane.id).unwrap()],
                        agents: std::slice::from_ref(&agent),
                        panes: std::slice::from_ref(&pane),
                    },
                    "ha",
                    None,
                    true,
                    None,
                )
                .unwrap()
            };
            assert!(uncertain_pass().error.is_some());
            assert!(uncertain_pass().error.is_none());
            assert_eq!(uncertain_runner.count("agent prompt"), 1);
            assert_eq!(
                thread::load(&f.project, &lane.id).unwrap().follow_ups[2].state,
                thread::FollowUpState::Uncertain
            );
            assert_eq!(crate::inbox::unhandled(&f.project).len(), 1);
        }
    }

    #[test]
    fn pending_prime_is_delivered_only_to_a_ready_agent() {
        let f = fixture(true);
        let runner = FakeRunner::new();
        runner.on("agent list", ok(&with_cwd(AGENT_BLOCKED, &f)));
        runner.on("pane list", ok(&with_cwd(PANE, &f)));
        runner.on("report-metadata", ok("{}"));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        assert!(tick_project(&ctx, &f.project).unwrap());
        assert_eq!(runner.count("agent prompt"), 0);
        assert!(f.project.coordinator().unwrap().prime_pending);

        let runner = FakeRunner::new();
        runner.on("agent list", ok(&with_cwd(AGENT_READY, &f)));
        runner.on("pane list", ok(&with_cwd(PANE, &f)));
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        runner.on("report-metadata", ok("{}"));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        assert!(tick_project(&ctx, &f.project).unwrap());
        assert_eq!(runner.count("agent prompt"), 1);
        // Transport is not the receipt: only `ha context` clears it (D14).
        let record = f.project.coordinator().unwrap();
        assert!(record.prime_sent && record.prime_pending);
        // The prompt went to the recorded socket.
        let calls = runner.calls.borrow();
        let prompt = calls
            .iter()
            .find(|c| c.display().contains("agent prompt"))
            .unwrap();
        assert!(prompt.env.iter().any(
            |(k, v)| k == "HERDR_SOCKET_PATH" && v == &f.project.coordinator().unwrap().socket
        ));
    }

    #[test]
    fn rejected_prime_stays_pending() {
        let f = fixture(true);
        let runner = FakeRunner::new();
        runner.on("agent list", ok(&with_cwd(AGENT_READY, &f)));
        runner.on("pane list", ok(&with_cwd(PANE, &f)));
        runner.on(
            "agent prompt",
            fail(
                1,
                r#"{"error":{"code":"agent_blocked","message":"blocked"}}"#,
            ),
        );
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        assert!(tick_project(&ctx, &f.project).is_err());
        assert!(f.project.coordinator().unwrap().prime_pending);
    }

    #[test]
    fn a_pane_with_other_identity_is_left_alone() {
        let f = fixture(true);
        let runner = FakeRunner::new();
        runner.on("workspace report-metadata", ok(r#"{"result":{}}"#));
        // Same ids, different working directory: not our pane.
        runner.on(
            "agent list",
            ok(&AGENT_READY.replace("CWD", "/somewhere/else")),
        );
        runner.on("pane list", ok(&PANE.replace("CWD", "/somewhere/else")));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        assert!(tick_project(&ctx, &f.project).unwrap());
        assert_eq!(runner.count("agent prompt"), 0);
        assert_eq!(runner.count("agent start"), 0);
        // The pane remains untouched.
        assert_eq!(runner.count("pane report-metadata"), 0);
    }

    #[test]
    fn a_name_dropped_while_the_agent_runs_is_restored_on_the_bound_pane() {
        let f = fixture(true);
        let runner = FakeRunner::new();
        // The agent is in the bound pane, but a startup timeout or a live
        // handoff left it without its recorded name.
        let unnamed =
            with_cwd(AGENT_READY, &f).replace(r#""name":"hp-demo-coordinator","#, r#""name":"","#);
        runner.on("agent list", ok(&unnamed));
        runner.on("pane list", ok(&with_cwd(PANE, &f)));
        runner.on(
            "agent rename",
            ok(r#"{"result":{"agent":{"pane_id":"w1:p1"}}}"#),
        );
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        runner.on("report-metadata", ok("{}"));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        assert!(tick_project(&ctx, &f.project).unwrap());
        let calls = runner.calls.borrow();
        let rename = calls
            .iter()
            .find(|c| c.display().contains("agent rename"))
            .expect("the recorded name is put back");
        assert!(rename.display().contains("w1:p1"), "{}", rename.display());
        assert!(
            rename.display().contains("hp-demo-coordinator"),
            "{}",
            rename.display()
        );
        assert!(rename.env.iter().any(
            |(k, v)| k == "HERDR_SOCKET_PATH" && v == &f.project.coordinator().unwrap().socket
        ));
        drop(calls);
        // The repair is durable, not just this tick's return value.
        assert_eq!(f.project.coordinator().unwrap().name_restored, 1);
    }

    #[test]
    fn shell_prompt_pane_never_starts_a_coordinator() {
        let f = fixture(true);
        let runner = FakeRunner::new();
        runner.on("agent list", ok(NO_AGENTS));
        runner.on("pane list", ok(&with_cwd(PANE, &f)));
        runner.on(
            "agent start",
            fail(1, r#"{"error":{"code":"timeout","message":"no agent"}}"#),
        );
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        for _ in 0..5 {
            let _ = tick_project(&ctx, &f.project);
        }
        assert_eq!(runner.count("agent start"), 0);
        assert_eq!(runner.count("agent prompt"), 0);
    }

    #[test]
    fn dead_coordinator_agent_in_existing_pane_restarts_once() {
        let f = fixture(false);
        f.project
            .update_coordinator(|c| {
                c.launch.kind = "claude".into();
                c.launch_attempts = 1;
                c.last_agent_seen_at = project::now();
            })
            .unwrap();
        let runner = FakeRunner::new();
        runner.on("agent list", ok(NO_AGENTS));
        runner.on("pane list", ok(&with_cwd(PANE, &f)));
        runner.on(
            "pane process-info",
            ok(r#"{"result":{"process_info":{"pane_id":"w1:p1","foreground_processes":[]}}}"#),
        );
        runner.on("agent start", ok(r#"{"result":{"agent":{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"/repo"}}}"#));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let _ = tick_project(&ctx, &f.project);
        let _ = tick_project(&ctx, &f.project);
        assert_eq!(runner.count("agent start"), 1);
        let record = f.project.coordinator().unwrap();
        assert_eq!(record.launch_attempts, 2);
        assert!(record.prime_pending);
        let ready = FakeRunner::new();
        ready.on("agent list", ok(&with_cwd(AGENT_READY, &f)));
        ready.on("pane list", ok(&with_cwd(PANE, &f)));
        ready.on("pane read", ok("❯ \n"));
        ready.on("agent prompt", ok(r#"{"result":{}}"#));
        let ctx = Ctx {
            runner: &ready,
            ..ctx
        };
        let _ = tick_project(&ctx, &f.project);
        let _ = tick_project(&ctx, &f.project);
        assert_eq!(ready.count("agent prompt"), 1);
        assert!(
            ready
                .calls
                .borrow()
                .iter()
                .any(|call| call.display().contains("context demo"))
        );
    }

    #[test]
    fn intentional_close_never_restarts_a_dead_process() {
        let f = fixture(false);
        f.project
            .update_coordinator(|c| {
                c.closed_by_rolf_at = project::now();
                c.last_agent_seen_at = project::now();
                c.launch.kind = "claude".into();
            })
            .unwrap();
        let runner = FakeRunner::new();
        runner.on("agent list", ok(NO_AGENTS));
        runner.on("pane list", ok(&with_cwd(PANE, &f)));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let _ = tick_project(&ctx, &f.project);
        assert_eq!(runner.count("agent start"), 0);
        assert_eq!(runner.count("pane process-info"), 0);
    }

    #[test]
    fn missing_coordinator_pane_is_unavailable_and_not_relaunched() {
        let f = fixture(false);
        let runner = FakeRunner::new();
        runner.on("agent list", ok(NO_AGENTS));
        runner.on("pane list", ok(r#"{"result":{"panes":[]}}"#));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let _ = tick_project(&ctx, &f.project);
        assert!(
            f.project
                .coordinator()
                .unwrap()
                .closed_by_rolf_at
                .is_empty()
        );
        let _ = tick_project(&ctx, &f.project);
        assert_eq!(
            crate::inbox::unhandled(&f.project)
                .iter()
                .filter(|i| i.summary.contains("coordinator_unavailable"))
                .count(),
            1
        );
        assert_eq!(runner.count("workspace create"), 0);
        assert_eq!(runner.count("agent start"), 0);
    }

    #[test]
    fn restarted_server_with_all_panes_gone_does_not_start_a_coordinator() {
        let f = fixture(false);
        f.project
            .update_coordinator(|c| {
                c.launch.recipe_id = "recorded-recipe".into();
                c.launch.kind = "claude".into();
            })
            .unwrap();
        let socket = f.project.coordinator().unwrap().socket;
        let replacement = f._home.path().join("new.sock");
        std::fs::write(&replacement, b"").unwrap();
        std::fs::rename(replacement, socket).unwrap();
        let runner = FakeRunner::new();
        runner.on("agent list", ok(NO_AGENTS));
        runner.on("pane list", ok(r#"{"result":{"panes":[]}}"#));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let _ = tick_project(&ctx, &f.project);
        let _ = tick_project(&ctx, &f.project);
        assert_eq!(runner.count("agent start"), 0);
        assert_eq!(runner.count("workspace create"), 0);
        assert_eq!(
            f.project.coordinator().unwrap().launch.recipe_id,
            "recorded-recipe"
        );
    }

    #[test]
    fn restarted_server_with_other_projects_panes_leaves_coordinator_idle() {
        let f = fixture(false);
        let socket = f.project.coordinator().unwrap().socket;
        let replacement = f._home.path().join("new.sock");
        std::fs::write(&replacement, b"").unwrap();
        std::fs::rename(replacement, socket).unwrap();
        let runner = FakeRunner::new();
        runner.on("agent list", ok(NO_AGENTS));
        runner.on(
            "pane list",
            ok(r#"{"result":{"panes":[{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","cwd":"/other"}]}}"#),
        );
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let _ = tick_project(&ctx, &f.project);
        assert!(
            f.project
                .coordinator()
                .unwrap()
                .closed_by_rolf_at
                .is_empty()
        );
        assert_eq!(runner.count("agent start"), 0);
        assert_eq!(runner.count("workspace create"), 0);
    }

    #[test]
    fn unreachable_session_reads_no_state() {
        let f = fixture(true);
        let runner = FakeRunner::new();
        runner.on("agent list", fail(1, "connection refused"));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        assert!(!tick_project(&ctx, &f.project).unwrap());

        // A socket file that is gone is not even called.
        std::fs::remove_file(f.project.coordinator().unwrap().socket).unwrap();
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        assert!(!tick_project(&ctx, &f.project).unwrap());
        assert!(runner.calls.borrow().is_empty());
    }
}
