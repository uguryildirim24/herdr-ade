//! The ticker: one background loop per projects root.
//!
//! Everything it does is "check on an interval, compare with last time, act".
//! It exits on request through a stop file, never through signals.

use std::collections::BTreeMap;
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
const IDLE_EXIT: Duration = Duration::from_secs(300);
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

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct Progress {
    pid: u32,
    started: String,
    sequence: u64,
    step: String,
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
}

/// Probes the lock without keeping it. The file is never created here.
pub(crate) fn lock_state(root: &Path) -> LockState {
    let Ok(mut file) = File::options().read(true).write(true).open(lock_path(root)) else {
        return LockState::Free;
    };
    match file.try_lock() {
        Ok(()) => LockState::Free,
        Err(_) => {
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
    }
}

/// Ensures a ticker is running without waiting for a running one to stop.
/// `round advance` must not block while replacing a ticker: the ticker's own
/// pass calls `advance`, so waiting here would deadlock against the ticker
/// waiting on `advance`'s lock. Ordinary thread starts and explicit `ticker
/// start` calls attempt to replace a stale-version ticker when it can stop.
pub(crate) fn ensure(ctx: &Ctx) -> Result<()> {
    let root = &ctx.root;
    if !ctx.detached_ticker || project::list_slugs(root).is_empty() || install_in_progress(ctx) {
        return Ok(());
    }
    if lock_state(root) == LockState::Free {
        let _ = std::fs::remove_file(stop_path(root));
        spawn(root)?;
    }
    Ok(())
}

/// Spawns the detached loop unless there is nothing to watch. It creates
/// nothing when the root does not exist or contains no projects, so a linked
/// plugin's `[[startup]]` is harmless in sessions that have no projects.
pub(crate) fn start(ctx: &Ctx) -> Result<()> {
    // The installer replaces the ticker before releasing its lock. Commands
    // may record pending work during that window; they must not compete with
    // the replacement or refuse the work. After a failed install, the next
    // ordinary start reaches start_inner and starts a free ticker as usual.
    if install_in_progress(ctx) {
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
    if !ctx.detached_ticker || project::list_slugs(root).is_empty() {
        return Ok(false);
    }
    let state = lock_state(root);
    // A newly acquired lock is published just after initialization. Do not
    // mistake its as-yet-empty record for an old build and stop the winner.
    if matches!(&state, LockState::Held(info) if info.pid == 0 || info.version.is_empty()) {
        return Ok(false);
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
    detached_command(root)?
        .spawn()
        .context("could not start the ticker")?;
    Ok(())
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
    match lock_state(root) {
        LockState::Free => println!("ticker: not running (root {})", root.display()),
        LockState::Held(info) => {
            println!("ticker: running");
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

/// The loop. Exits when another ticker holds the lock, when the stop file
/// appears, or when no project has had a reachable session for five minutes.
pub(crate) fn run(ctx: &Ctx) -> Result<()> {
    if project::list_slugs(&ctx.root).is_empty() {
        return Ok(());
    }
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
    let mut last_reachable = Instant::now();
    let mut memory = Memory::new(ctx);
    let _thread_records = thread::ListCache::new();

    // Publish only after acquiring the lock, before the first pass: that
    // pass may block, but this process is already running the new image.
    lock.set_len(0)?;
    lock.write_all(serde_json::to_string_pretty(&info)?.as_bytes())?;
    lock.flush()?;
    log.line(&format!(
        "ticker {} started (pid {})",
        info.version, info.pid
    ));
    let mut progress = Progress {
        pid: info.pid,
        started: info.started.clone(),
        ..Progress::default()
    };
    let mut step = |name: &str| {
        progress.sequence += 1;
        progress.step = name.to_string();
        if let Err(error) = project::write_json(&progress_path(root), &progress) {
            log.line(&format!("could not publish ticker progress: {error:#}"));
        }
        !stop_path(root).exists()
    };
    match tick_with_steps(ctx, &log, &mut memory, &mut step) {
        None => {
            log.line("stop file found; exiting");
            return Ok(());
        }
        Some(true) => last_reachable = Instant::now(),
        Some(false) => {}
    }

    loop {
        // Sleep in short slices so a stop request is honoured promptly.
        let wake = Instant::now() + TICK;
        while Instant::now() < wake {
            if stop_path(root).exists() {
                log.line("stop file found; exiting");
                return Ok(());
            }
            if wake_path(root).exists() {
                let _ = std::fs::remove_file(wake_path(root));
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        match tick_with_steps(ctx, &log, &mut memory, &mut step) {
            None => {
                log.line("stop file found; exiting");
                return Ok(());
            }
            Some(true) => last_reachable = Instant::now(),
            Some(false) if last_reachable.elapsed() > IDLE_EXIT => {
                log.line("no project has had a reachable session for five minutes; exiting");
                return Ok(());
            }
            Some(false) => {}
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
    memory.tick += 1;
    memory.machine_views.clear();
    let mut reachable = Vec::new();
    for slug in project::list_slugs(&ctx.root) {
        if !step(&format!("cheap project {slug}")) {
            return None;
        }
        let Ok(project) = Project::load(&ctx.root, &slug) else {
            continue;
        };
        if project.status() != Status::Active {
            continue;
        }
        match tick_cheap(
            ctx,
            &project,
            memory.tick == 1 || memory.tick.is_multiple_of(8),
        ) {
            Ok(Some(seen)) => reachable.push((project, seen)),
            Ok(None) => {}
            Err(error) => log.line(&format!("{slug}: {error:#}")),
        }
    }
    // One courier pass per due machine, covering every project with lanes on
    // it (SPEC-remote §4.3). This runs before the per-project slow pass so the
    // first project cannot starve the cadence of the others.
    let project_refs: Vec<&Project> = reachable.iter().map(|(project, _)| project).collect();
    if !step("machine phase") {
        return None;
    }
    for error in machine_passes_with_steps(ctx, &project_refs, memory, log, step)? {
        log.line(&format!("{error:#}"));
    }
    if !step("slow phase") {
        return None;
    }
    for (project, seen) in &reachable {
        if !step(&format!("slow project {}", project.slug)) {
            return None;
        }
        let (errors, completed) = tick_slow_with_steps(ctx, project, seen, memory, step);
        for error in errors {
            log.line(&format!("{}: {error:#}", project.slug));
        }
        if !completed {
            return None;
        }
    }
    Some(!reachable.is_empty())
}

fn record_failed_observation(entries: &[(Project, Vec<thread::Thread>)], detail: &str, log: &Log) {
    let attempted = project::now();
    for (project, threads) in entries {
        for lane in threads {
            if let Err(error) = thread::update(project, &lane.id, |record| {
                record.observation_attempted = attempted.clone();
                record.observation_source = "courier".into();
                record.observation_error = crate::pr::sanitize(detail);
            }) {
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

/// One courier pass per saved machine that has lanes, once per fourth tick.
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
        let remote = open_threads(project, true);
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
            // This is local configuration evidence, not evidence about the
            // connection. Leave no remote view for the slow pass and replace
            // any stale connection classification with unknown.
            clear_lost_connections(&entries, log);
            for (project, _) in &entries {
                clear_poll_request(project, &machine);
            }
            continue;
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
                    if let Err(error) = thread::update(project, &lane.id, |record| {
                        record.failure_class = crate::contracts::FailureClass::LostConnection;
                        record.provider_failure_kind = None;
                        record.last_failure = detail.clone();
                        record.error = detail.clone();
                        record.last_group = thread::Group::WaitingOnYou.token().into();
                    }) {
                        log.line(&format!("{error:#}"));
                    }
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
        }
        let Some((first, _)) = entries.first() else {
            continue;
        };
        if let Err(error) = steps::write_machine_outage(first, &machine, event, memory) {
            errors.push(error.context("machine outage"));
        }
        for (project, _) in &entries {
            clear_poll_request(project, &machine);
        }
        memory
            .machine_views
            .insert(machine, outcome.map_err(|e| format!("{e:#}")));
    }
    Some(errors)
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
    memory.machine_views.clear();
    match tick_cheap(ctx, project, true)? {
        Some(seen) => {
            let log = Log {
                path: std::env::temp_dir().join(format!("hp-test-log-{}", std::process::id())),
            };
            let projects = [project];
            for error in machine_passes(ctx, &projects, memory, &log) {
                log.line(&format!("{error:#}"));
            }
            match tick_slow(ctx, project, &seen, memory).into_iter().next() {
                Some(error) => Err(error),
                None => Ok(true),
            }
        }
        None => Ok(false),
    }
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
    })?;
    if !thread.is_remote() {
        let notice = format!(
            "{} did not become ready during startup: {reason}. Read `thread show {} {}` before retrying.",
            thread.id, input.project.slug, thread.id
        );
        let sent = input.project.coordinator().is_some_and(|coordinator| {
            input
                .agents
                .iter()
                .any(|agent| agent.pane_id == coordinator.pane_id && agent.ready())
                && crate::talk::writer_lock(input.project).is_ok_and(|_writer| {
                    crate::talk::coordinator_prompt_clear(
                        input.project,
                        input.herdr,
                        &coordinator.pane_id,
                    )
                    .unwrap_or(false)
                        && crate::talk::mark_automated_prompt(
                            input.project,
                            &coordinator.pane_id,
                            &notice,
                        )
                        .is_ok()
                        && input
                            .herdr
                            .agent_prompt(&coordinator.pane_id, &notice)
                            .is_ok()
                })
        });
        if !sent {
            crate::inbox::write(input.project, "lane-notice", &thread.id, &notice, "")?;
        }
    }
    Ok(())
}

fn thread_pass(
    input: &LaunchPass<'_>,
    prefix: &str,
    hashes: Option<&std::collections::BTreeMap<String, String>>,
    refresh_tokens: bool,
) -> Result<Pass> {
    let ctx = input.ctx;
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
    // If the coordinator and every local lane disappeared together, the
    // session link failed. That is not evidence that each worker process died,
    // so session recovery owns it instead of starting replacement lanes.
    let whole_session_missing = threads.first().is_some_and(|thread| !thread.is_remote())
        && project.coordinator().is_some_and(|coordinator| {
            !agents
                .iter()
                .any(|agent| coordinator::agent_matches(&coordinator, agent))
                && !panes
                    .iter()
                    .any(|pane| coordinator::pane_matches(&coordinator, pane))
        })
        && threads.iter().any(|thread| !thread.pane_id.is_empty())
        && threads.iter().all(|thread| {
            thread.pane_id.is_empty() || !thread::live_state(thread, agents, panes, now).pane_exists
        });
    for t in threads {
        if t.queued_for_load {
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
        let mut live = thread::live_state(t, agents, panes, now);
        if !t.pane_id.is_empty() {
            pass.recorded_panes += 1;
            pass.missing_panes += usize::from(!live.pane_exists);
        }
        let state = live.agent_state.clone().unwrap_or_default();
        if state != t.last_state {
            live.state_secs = 0;
        }
        // A remote thread is polled once a minute, so `blocked` at a poll
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
        // A timed-out `agent start` can become ready on a later courier pass.
        // It must pass the same process check before receiving its brief.
        if t.is_remote() && t.prompt_pending && ready && t.checked_slice.is_empty() {
            let check = (|| {
                let process = box_agent_process(herdr, t)
                    .ok_or_else(|| anyhow::anyhow!("agent process missing"))?;
                let machine = crate::remote::declaration_for_route(
                    ctx.runner,
                    &ctx.env.herdr_bin(),
                    &ctx.config_dir,
                    t.machine_route(),
                )?;
                crate::remote::check_agent_slice(ctx.runner, &machine, slug, process.pid)
            })();
            match check {
                Ok(slice) => {
                    thread::update(project, &t.id, |record| record.checked_slice = slice)?;
                }
                Err(error) => {
                    if let Err(cleanup) = threads::fail_start(
                        ctx,
                        project,
                        &t.id,
                        &format!("box_slice_check: {error:#}"),
                        crate::contracts::FailureClass::Unknown,
                        false,
                    ) {
                        pass.error = pass.error.or(Some(
                            cleanup.context(format!("{}: box slice failed-start cleanup", t.id)),
                        ));
                    }
                    continue;
                }
            }
        }
        if !t.startup_wait_started.is_empty() {
            if ready {
                thread::update(project, &t.id, |t| {
                    t.startup_wait_started.clear();
                    t.status = thread::Status::Open;
                })?;
            } else if (state == "blocked" || (state.is_empty() && live.pane_exists))
                && thread::seconds_since(&t.startup_wait_started, now).max(0) as u64 * 1000
                    >= agent_start_timeout(&t.launch)
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
        let mut delivered = false;
        if t.prompt_pending && ready {
            match herdr.agent_prompt_wait_started(
                &t.pane_id,
                &thread::launch_prompt(prefix, slug, t),
                agent_start_timeout(&t.launch),
            ) {
                Ok(()) => {
                    delivered = true;
                    thread::update(project, &t.id, |thread| thread.prompt_pending = false)?;
                }
                Err(error) => {
                    pass.error = pass
                        .error
                        .or(Some(anyhow::anyhow!("{}: brief prompt: {error}", t.id)))
                }
            }
        } else if !t.prompt_pending && t.bootstrap == "acknowledged" && ready {
            // The matching bootstrap receipt proves the lane consumed this
            // attempt's brief. Mark each message uncertain before transport:
            // a crash or ambiguous transport result must never resend it.
            loop {
                let current = thread::load(project, &t.id)?;
                if current.status != thread::Status::Open
                    || current.prompt_pending
                    || current.bootstrap != "acknowledged"
                {
                    break;
                }
                let attempt = current.attempt.max(1);
                let Some((index, follow_up)) = current
                    .follow_ups
                    .iter()
                    .enumerate()
                    .find(|(_, follow_up)| {
                        follow_up.attempt == attempt
                            && matches!(
                                follow_up.state,
                                thread::FollowUpState::Queued | thread::FollowUpState::Uncertain
                            )
                    })
                    .map(|(index, follow_up)| (index, follow_up.clone()))
                else {
                    break;
                };
                if follow_up.state == thread::FollowUpState::Uncertain {
                    break;
                }
                if let Err(error) = crate::threads::sync_box_corrections(ctx, project, &current) {
                    pass.error = pass.error.or(Some(
                        error.context(format!("{}: correction barrier before queued prompt", t.id)),
                    ));
                    break;
                }
                thread::update_checked(project, &t.id, |thread| {
                    if thread.status != thread::Status::Open
                        || thread.attempt.max(1) != attempt
                        || thread.follow_ups.get(index) != Some(&follow_up)
                    {
                        anyhow::bail!("queued follow-up changed before delivery");
                    }
                    thread.follow_ups[index].state = thread::FollowUpState::Uncertain;
                    Ok(())
                })?;
                let after_seal =
                    crate::round::latest_done_event(&crate::events::list(project), &t.id, attempt)
                        .map(|event| event.id.clone())
                        .unwrap_or_default();
                match herdr.agent_prompt(&current.pane_id, &follow_up.text) {
                    Ok(()) => {
                        delivered = true;
                        crate::threads::record_answered_wait(
                            project,
                            &t.id,
                            attempt,
                            &follow_up.waiting_event,
                        )?;
                        thread::update_checked(project, &t.id, |thread| {
                            let Some(saved) = thread.follow_ups.get(index) else {
                                anyhow::bail!("queued follow-up disappeared during delivery");
                            };
                            if saved.attempt != attempt
                                || saved.text != follow_up.text
                                || saved.state != thread::FollowUpState::Uncertain
                            {
                                anyhow::bail!("queued follow-up changed during delivery");
                            }
                            thread.follow_ups[index].state = thread::FollowUpState::Delivered;
                            thread.follow_ups[index].delivered_at = project::now();
                            thread.follow_ups[index].after_seal = after_seal;
                            Ok(())
                        })?;
                    }
                    Err(error)
                        if !matches!(error.code.as_str(), "timeout" | "unreachable" | "failed") =>
                    {
                        // Herdr refused before typing (for example a newly
                        // blocked approval prompt), so this remains retryable.
                        thread::update_checked(project, &t.id, |thread| {
                            if let Some(saved) = thread.follow_ups.get_mut(index)
                                && saved.attempt == attempt
                                && saved.text == follow_up.text
                                && saved.state == thread::FollowUpState::Uncertain
                            {
                                saved.state = thread::FollowUpState::Queued;
                            }
                            Ok(())
                        })?;
                        pass.error = pass
                            .error
                            .or(Some(anyhow::anyhow!("{}: queued prompt: {error}", t.id)));
                        break;
                    }
                    Err(error) => {
                        let _ = inbox::write(
                            project,
                            "prompt-uncertain",
                            &t.id,
                            &format!(
                                "{} attempt {attempt} may have received a follow-up; check the lane before sending it again",
                                t.id
                            ),
                            "",
                        );
                        pass.error = pass
                            .error
                            .or(Some(anyhow::anyhow!("{}: queued prompt: {error}", t.id)));
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
        // Only absence of the pane proves a local process is gone. Herdr may
        // temporarily omit agent state while the terminal and process still
        // exist (including after an interactive startup timeout); that state
        // is Unknown and must never authorize closing the pane.
        let process_gone = !t.is_remote()
            && (after.startup_wait_started.is_empty()
                || thread::seconds_since(&after.startup_wait_started, now).max(0) as u64 * 1000
                    >= agent_start_timeout(&after.launch))
            && after.report_hash.is_empty()
            && !live.pane_exists;
        if process_gone && !whole_session_missing {
            let recover = !t.launch.recipe_id.is_empty();
            if let Err(error) = threads::fail_start(
                ctx,
                project,
                &t.id,
                "the pane or agent is gone without a report",
                crate::contracts::FailureClass::ProcessGone,
                recover,
            ) {
                pass.error = pass
                    .error
                    .or(Some(error.context(format!("{}: process recovery", t.id))));
            }
            continue;
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

fn agent_start_timeout(launch: &crate::contracts::Launch) -> u64 {
    if launch.ready_timeout_ms == 0 {
        crate::herdr::AGENT_START_TIMEOUT.as_millis() as u64
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

/// Only an agent executable, never a shell, scope launcher, or unrelated
/// foreground tool, is evidence for the box cgroup check.
fn box_agent_process(
    herdr: &Herdr<'_>,
    thread: &thread::Thread,
) -> Option<crate::contracts::ProcessIdentity> {
    let executable = match thread.launch.kind.as_str() {
        "dsh" => "dst",
        "cursor" => "cursor-agent",
        kind => kind,
    };
    let info = herdr.pane_process_info(&thread.pane_id).ok()?;
    let process = info.foreground_processes.iter().find(|process| {
        if process
            .argv0
            .as_deref()
            .unwrap_or(&process.name)
            .rsplit('/')
            .next()
            == Some(executable)
        {
            return true;
        }
        // Linux reports shebang-launched pi as node without argv0. Match its
        // CLI script, not an unrelated node process in the foreground job.
        thread.launch.kind == "pi"
            && process.argv.as_ref().is_some_and(|argv| {
                let [runtime, script, ..] = argv.as_slice() else {
                    return false;
                };
                matches!(runtime.rsplit('/').next(), Some("node" | "bun"))
                    && (script
                        .ends_with("/node_modules/@earendil-works/pi-coding-agent/dist/cli.js")
                        || script.ends_with(
                            "/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js",
                        ))
            })
    })?;
    Some(crate::contracts::ProcessIdentity {
        pid: process.pid,
        argv0: process
            .argv0
            .clone()
            .unwrap_or_else(|| process.name.clone()),
    })
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
    let mut machine = None;
    for t in pass.threads {
        if t.status != thread::Status::Open || !t.prompt_pending {
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
        if live.agent_state.is_some() || !live.pane_exists {
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
        // Provider-bridge credentials can expire between placement and start.
        // The adapter's readiness driver, not its agent-kind name, chooses the
        // extra check; command probes were already run during placement.
        let readiness = if t.launch.kind.is_empty() {
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
            let class = crate::pi_ade::failure_class(&error);
            let message = format!("{error:#}");
            errors.extend(
                threads::fail_start(pass.ctx, pass.project, &t.id, &message, class, true)
                    .err()
                    .map(|cleanup| cleanup.context(format!("{}: failed-start cleanup", t.id))),
            );
            continue;
        }
        if t.is_remote() && machine.is_none() {
            match crate::remote::declaration_for_route(
                pass.ctx.runner,
                &pass.ctx.env.herdr_bin(),
                &pass.ctx.config_dir,
                t.machine_route(),
            ) {
                Ok(declaration) => machine = Some(declaration),
                Err(error) => {
                    errors.push(error.context(format!("{}: box launch declaration", t.id)));
                    continue;
                }
            }
        }
        match thread::update(pass.project, &t.id, |t| {
            t.launch_attempts += 1;
            t.startup_wait_started = project::now();
        }) {
            Ok(_) => {
                pending.push(t);
                if one_at_a_time {
                    *may_start = false;
                    break;
                }
            }
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
    let exclusive_bin = machine
        .as_ref()
        .map(|machine| format!("{}/.state/slices/{}/bin", machine.root, pass.project.slug));
    let starts: Vec<_> = pending
        .iter()
        .map(|t| crate::herdr::AgentStart {
            name: &t.agent_name,
            kind: &t.launch.kind,
            pane: &t.pane_id,
            agent_args: &t.launch.args,
            launch_bin: exclusive_bin.as_deref(),
            parent: parent.as_deref(),
            // `agent start` need not hold the ticker for the whole observation
            // window: subsequent passes watch the pane for the remaining time.
            ready_timeout_ms: agent_start_timeout(&t.launch)
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
            // Herdr acknowledges a start before the interactive shell has
            // necessarily exec'd the command. Wait briefly for the real
            // agent process, not the preceding shell/systemd-run process.
            let deadline = Instant::now() + Duration::from_secs(5);
            let process = loop {
                let found = if t.is_remote() {
                    box_agent_process(&herdr, t)
                } else {
                    herdr
                        .pane_process_info(&t.pane_id)
                        .ok()
                        .and_then(|info| info.identity(&t.launch.kind))
                };
                if found.is_some() || !t.is_remote() || Instant::now() >= deadline {
                    break found;
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            let checked_slice = if let Some(machine) = &machine {
                let pid = process
                    .as_ref()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "box_slice_check: agent process missing; cannot check /proc/<pid>/cgroup"
                        )
                    })?
                    .pid;
                Some(crate::remote::check_agent_slice(
                    pass.ctx.runner,
                    machine,
                    &pass.project.slug,
                    pid,
                ).map_err(|error| anyhow::anyhow!("box_slice_check: {error:#}"))?)
            } else {
                None
            };
            thread::update(pass.project, &t.id, |record| {
                if let Some(slice) = &checked_slice {
                    record.checked_slice = slice.clone();
                }
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
                record.startup_wait_started.clear();
                thread::bind_identity(record, &socket, &bound, process);
            })?;
            if !t.launch.compact_reason.is_empty() {
                let _ = crate::board::publish_value(
                    pass.ctx,
                    pass.project,
                    "ade_last",
                    &t.launch.compact_reason,
                );
            }
            Ok(())
        });
        if let Err(error) = launched {
            if error.to_string().contains("agent_not_ready") {
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
            if t.is_remote() && error.to_string().contains("box_slice_check:") {
                let message = format!("{error:#}");
                errors.extend(
                    threads::fail_start(
                        pass.ctx,
                        pass.project,
                        &t.id,
                        &message,
                        crate::contracts::FailureClass::Unknown,
                        false,
                    )
                    .err(),
                );
            }
            errors.push(error.context(format!("{}: launch", t.id)));
        }
    }
    true
}

fn open_threads(project: &Project, remote: bool) -> Vec<thread::Thread> {
    thread::list(project)
        .into_iter()
        .filter(|t| {
            t.is_remote() == remote
                && !t.queued_for_load
                && matches!(t.status, thread::Status::Open | thread::Status::Starting)
        })
        .collect()
}

fn idle_nudge_gate(
    project: &Project,
    state: &steps::State,
    minutes: u64,
    now: jiff::Timestamp,
) -> Result<Option<String>> {
    if state.idle_nudge_last.is_empty() {
        return Ok(None);
    }
    let interval_secs = minutes.saturating_mul(60).min(i64::MAX as u64) as i64;
    let last = state.idle_nudge_last.parse::<jiff::Timestamp>().ok();
    if thread::seconds_since(&state.idle_nudge_last, now) < interval_secs {
        let until = last.and_then(|last| {
            jiff::Timestamp::from_second(last.as_second().saturating_add(interval_secs)).ok()
        });
        return Ok(Some(format!(
            "not due until {}",
            until.map_or_else(|| "unknown".into(), |t| t.to_string())
        )));
    }
    let command = crate::ledger::latest_coordinator_turn(project)?;
    let turned = [command.as_str(), state.coordinator_worked_at.as_str()]
        .iter()
        .filter_map(|time| time.parse::<jiff::Timestamp>().ok())
        .any(|time| last.is_some_and(|last| time > last));
    Ok((!turned).then(|| "no turn since last nudge".into()))
}

fn idle_nudge_due(
    project: &Project,
    state: &steps::State,
    minutes: u64,
    now: jiff::Timestamp,
) -> Result<bool> {
    Ok(idle_nudge_gate(project, state, minutes, now)?.is_none())
}

/// One explanation for both the ticker and the two human surfaces.
pub(crate) fn idle_nudge_status(
    project: &Project,
    state: &steps::State,
    policy: (&crate::project::Settings, u64),
    agent: Option<&Agent>,
    live: (&[Agent], &[Pane]),
    input_clear: Option<bool>,
    now: jiff::Timestamp,
) -> Result<String> {
    let reason = if !policy.0.nudge {
        Some("nudge off".into())
    } else if project.finished() || project.status() != project::Status::Active {
        Some("finished or paused".into())
    } else if project
        .coordinator()
        .is_some_and(|record| record.prime_pending)
        || agent.is_none_or(|agent| !agent.ready())
    {
        Some("agent working".into())
    } else if idle_lane_lines(project, live.0, live.1).is_none() {
        Some("lane working".into())
    } else if let Some(reason) = idle_nudge_gate(project, state, policy.1, now)? {
        Some(reason)
    } else if input_clear == Some(false) {
        Some("input held".into())
    } else if input_clear.is_none() {
        Some("input unavailable".into())
    } else {
        None
    };
    Ok(format!(
        "last idle nudge: {}; {}",
        if state.idle_nudge_last.is_empty() {
            "never"
        } else {
            &state.idle_nudge_last
        },
        reason.unwrap_or_else(|| "due".into())
    ))
}

pub(crate) fn idle_nudge_line(
    project: &Project,
    config_dir: &std::path::Path,
    herdr: &Herdr<'_>,
) -> Result<String> {
    let state = steps::load_state(project);
    let (settings, _) = project.read_project_md()?;
    let minutes = crate::project::coordinator_settings(config_dir)?.idle_nudge_minutes;
    let record = project.coordinator();
    // Without a live view the ticker cannot send; an empty invented view
    // would misreport a transport failure as an agent or lane still working.
    let agents = herdr.agent_list()?;
    let panes = herdr.pane_list()?;
    let agent = record.as_ref().and_then(|record| {
        agents
            .iter()
            .find(|agent| crate::coordinator::agent_matches(record, agent))
    });
    let input_clear = record
        .as_ref()
        .and_then(|record| herdr.pane_read_ansi(&record.pane_id, "visible").ok())
        .map(|screen| crate::talk::coordinator_input_clear(&screen));
    idle_nudge_status(
        project,
        &state,
        (&settings, minutes),
        agent,
        (&agents, &panes),
        input_clear,
        jiff::Timestamp::now(),
    )
}

fn idle_task_lines(project: &Project) -> Vec<String> {
    crate::task::views(project)
        .0
        .into_iter()
        .filter(|view| !view.terminal(project))
        .map(
            |view| match crate::task::active_wait(project, &view.record) {
                Some(wait) => format!(
                    "held: {} waits on {} {}",
                    view.record.id, wait.kind, wait.target
                ),
                None => format!("{}: {}", view.record.id, view.next),
            },
        )
        .collect()
}

fn idle_lane_lines(project: &Project, agents: &[Agent], panes: &[Pane]) -> Option<Vec<String>> {
    let mut lines = Vec::new();
    let now = jiff::Timestamp::now();
    for lane in thread::list(project) {
        if lane.status == thread::Status::Resolved {
            continue;
        }
        // A cached Working group may outlive a report. Keep startup and
        // pending prompts working, but use the observed agent state for open
        // lanes instead of that cached group.
        let working = if lane.status == thread::Status::Failed {
            false
        } else if lane.status == thread::Status::Starting {
            thread::recorded_group(&lane, now) == thread::Group::Working
        } else if lane.is_remote() {
            !lane.startup_wait_started.is_empty()
                || lane.prompt_pending
                || (lane.report_hash.is_empty()
                    && lane.last_group == thread::Group::Working.token()
                    && matches!(lane.last_state.as_str(), "working" | "blocked"))
        } else {
            let live = thread::live_state(&lane, agents, panes, now);
            thread::group(&lane, &live, now) == thread::Group::Working
        };
        if working {
            return None;
        }
        let (status, action) = if lane.status == thread::Status::Failed {
            ("failed", "inspect the failure and recover or resolve it")
        } else if !lane.report_hash.is_empty()
            && (lane.report_hash != lane.acked_report_hash || lane.pr_state == "open")
        {
            ("ready for review", "open a round")
        } else if lane.status == thread::Status::Starting
            || lane.prompt_pending
            || lane.last_group == thread::Group::WaitingOnYou.token()
        {
            ("waiting on you", "inspect the block and take the next step")
        } else {
            ("idle", "check its result and take the next step")
        };
        lines.push(format!("{} is {status}: {action}", lane.id));
    }
    Some(lines)
}

fn idle_nudge_text(lines: &[String]) -> String {
    let blocks = if lines.is_empty() {
        "No open tasks.".to_string()
    } else {
        lines.join("; ")
    };
    let next = if !lines.is_empty() && lines.iter().all(|line| line.starts_with("held: ")) {
        "Everything open is held. Find new work toward the project goal or mark the project finished with ha finish."
    } else if lines.iter().any(|line| line.starts_with("held: ")) {
        "Leave held tasks alone; act on listed lanes that need you, then find and start other useful work toward the project goal."
    } else {
        "Find and start the next useful step toward the project goal now."
    };
    format!(
        "{} {} {} Ask Rolf only for what truly needs him; keep other work moving.",
        steps::TICKER_PROMPT_PREFIX,
        blocks,
        next
    )
}

fn nudge_idle_coordinator(
    ctx: &Ctx,
    project: &Project,
    state: &mut steps::State,
    herdr: &Herdr,
    coordinator: &crate::project::Coordinator,
    agent: Option<&Agent>,
    live: (&[Agent], &[Pane]),
) -> Result<()> {
    if !coordinator.closed_by_rolf_at.is_empty()
        || coordinator.prime_pending
        || agent.is_none_or(|agent| !agent.ready())
    {
        return Ok(());
    }
    if project.finished() || project.status() != project::Status::Active {
        return Ok(());
    }
    let Some(lanes) = idle_lane_lines(project, live.0, live.1) else {
        return Ok(());
    };
    let mut next = idle_task_lines(project);
    next.extend(lanes);
    let settings = crate::project::coordinator_settings(&ctx.config_dir)?;
    if !idle_nudge_due(
        project,
        state,
        settings.idle_nudge_minutes,
        jiff::Timestamp::now(),
    )? {
        return Ok(());
    }
    let text = idle_nudge_text(&next);
    let _writer = crate::talk::writer_lock(project)?;
    if !crate::talk::coordinator_prompt_clear(project, herdr, &coordinator.pane_id)? {
        return Ok(());
    }
    crate::talk::mark_automated_prompt(project, &coordinator.pane_id, &text)?;
    // Keep the send start, not the return time: the prompted turn can read
    // context before a fast transport call returns, and that turn must count.
    let sent_at = project::now();
    herdr.agent_prompt(&coordinator.pane_id, &text)?;
    state.idle_nudge_last = sent_at;
    crate::ledger::coordinator_nudge(project, &next)
}

pub(crate) fn socket_inode(path: &std::path::Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map_or(0, |meta| meta.ino())
}

fn coordinator_retry_due(project: &Project, record: &crate::project::Coordinator) -> Result<bool> {
    let mut state = steps::load_state(project);
    if !state.coordinator_relaunch_last.is_empty()
        && thread::seconds_since(&state.coordinator_relaunch_last, jiff::Timestamp::now()) < 3600
    {
        return Ok(false);
    }
    state.coordinator_relaunch_last = project::now();
    steps::save_state(project, &state)?;
    crate::ledger::coordinator_relaunch(project, &record.pane_id)?;
    Ok(true)
}

fn relaunch_missing_coordinator(
    ctx: &Ctx,
    project: &Project,
    record: &crate::project::Coordinator,
) -> Result<()> {
    if !coordinator_retry_due(project, record)? {
        return Ok(());
    }
    coordinator::open(
        ctx,
        &project.slug,
        &coordinator::OpenOptions {
            session: crate::paths::SessionFlags {
                session: None,
                socket: Some(record.socket.clone().into()),
            },
            reprime: false,
            rebind: false,
            recipe: None,
            recipe_basis: None,
        },
    )
}

fn restart_coordinator_agent(
    project: &Project,
    herdr: &Herdr,
    record: &crate::project::Coordinator,
) -> Result<()> {
    if !coordinator_retry_due(project, record)? {
        return Ok(());
    }
    project.update_coordinator(|c| {
        c.launch_attempts += 1;
        c.generation += 1;
        c.prime_sent = false;
    })?;
    let launch = &record.launch;
    herdr.agent_start_opts(&crate::herdr::AgentStart {
        name: &record.agent_name,
        kind: &launch.kind,
        pane: &record.pane_id,
        agent_args: &launch.args,
        launch_bin: None,
        parent: None,
        ready_timeout_ms: launch.ready_timeout_ms,
    })?;
    Ok(())
}

/// Returns `Ok(None)` when the project's session cannot be reached: then no
/// state is read, so nothing is ever reported as gone.
fn tick_cheap(ctx: &Ctx, project: &Project, refresh_tokens: bool) -> Result<Option<Seen>> {
    let _scope = crate::ledger::Scope::new(&[project]);
    let Some(record) = project.coordinator() else {
        return Ok(None);
    };
    if record.socket.is_empty() || !Path::new(&record.socket).exists() {
        return Ok(None);
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, ctx.runner);
    let Ok(agents) = herdr.agent_list() else {
        return Ok(None);
    };
    let Ok(panes) = herdr.pane_list() else {
        return Ok(None);
    };
    let slug = &project.slug;
    let prefix = coordinator::current_prefix(&ctx.root)?;
    let mut first_error = None;

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
            let prompt = coordinator::priming_prompt(&prefix, slug);
            let _writer = crate::talk::writer_lock(project)?;
            if crate::talk::coordinator_prompt_clear(project, &herdr, &record.pane_id)? {
                crate::talk::mark_automated_prompt(project, &record.pane_id, &prompt)?;
                match herdr.agent_prompt(&record.pane_id, &prompt) {
                    Ok(()) => {
                        project.update_coordinator(|c| c.prime_sent = true)?;
                    }
                    Err(error) => first_error = Some(anyhow::anyhow!("priming prompt: {error}")),
                }
            }
        }
        if refresh_tokens {
            coordinator::report_tokens(&herdr, slug, &record.pane_id);
        }
    }

    if let Err(error) = crate::threads::start_queued(ctx, project) {
        first_error = first_error.or(Some(error.context("queued box start")));
    }
    if let Err(error) = crate::threads::retry_pending_cleanup(ctx, project) {
        eprintln!("note: pending cleanup will retry: {error:#}");
    }
    crate::threads::resolve_report_only(ctx, project);

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
    )?;
    first_error = first_error.or(pass.error);
    if let Err(error) = crate::threads::tick(project, &herdr, &agents) {
        first_error = first_error.or(Some(error));
    }
    // The ops pass (A2) and the rounds pass (A3) run in the slow pass,
    // outside the project lock (SPEC-ADE item 57).
    let pane_alive = panes.iter().any(|p| coordinator::pane_matches(&record, p));
    let inode = socket_inode(Path::new(&record.socket));
    if pane_alive && inode != 0 && record.server_socket_inode != inode {
        project.update_coordinator(|c| c.server_socket_inode = inode)?;
    }
    let coordinator_recorded = usize::from(!record.pane_id.is_empty());
    let coordinator_missing = usize::from(
        coordinator_recorded == 1
            && agent.is_none()
            && !panes.iter().any(|p| coordinator::pane_matches(&record, p)),
    );
    let recorded_panes = pass.recorded_panes + coordinator_recorded;
    let missing_panes = pass.missing_panes + coordinator_missing;

    if coordinator_recorded == 1 && project.status() == project::Status::Active {
        let restarted = record.server_socket_inode != 0
            && inode != 0
            && record.server_socket_inode != inode
            && panes.is_empty();
        if !pane_alive && agent.is_none() && !record.reopen_requested && !restarted {
            if record.closed_by_rolf_at.is_empty() {
                project.update_coordinator(|c| {
                    if c.closed_by_rolf_at.is_empty() && !c.reopen_requested {
                        c.closed_by_rolf_at = project::now();
                    }
                })?;
            }
        } else if record.closed_by_rolf_at.is_empty() && agent.is_none() {
            let result = if pane_alive {
                Ok(()) // the slow pass owns process restarts and launch capacity
            } else if restarted || record.reopen_requested {
                relaunch_missing_coordinator(ctx, project, &record)
            } else {
                Ok(())
            };
            if let Err(error) = result {
                crate::ledger::observe(
                    project,
                    "coordinator-relaunch",
                    slug,
                    &format!("{error:#}"),
                );
            }
        }
    }

    // Nudge (or notify) about inbox items `context` has not shown yet.
    if let Ok((settings, _)) = project.read_project_md() {
        let mut state = steps::load_state(project);
        let before = state.clone();
        if agent.as_ref().is_some_and(|a| a.agent_status == "working")
            && !state.idle_nudge_last.is_empty()
        {
            state.coordinator_worked_at = project::now();
        }
        let ready_pane = agent
            .as_ref()
            .filter(|a| a.ready())
            .map(|_| record.pane_id.as_str());
        if let Err(error) = steps::nudge(project, &mut state, &settings, &herdr, ready_pane) {
            first_error = first_error.or(Some(error.context("nudge")));
        }
        let inbox_prompted = settings.nudge && state.nudged != before.nudged;
        if settings.nudge
            && !inbox_prompted
            && let Err(error) = nudge_idle_coordinator(
                ctx,
                project,
                &mut state,
                &herdr,
                &record,
                agent.as_ref(),
                (&agents, &panes),
            )
        {
            first_error = first_error.or(Some(error.context("idle coordinator nudge")));
        }
        if state != before {
            steps::save_state(project, &state)?;
        }
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

    // Before the label/id fix, declaration lookup consumed an attempt without
    // submitting an agent and left the startup clock running. Reclaim an idle
    // pane once; a later real start that times out must not reset the bounded
    // launch counter forever.
    let mut threads = pass.threads.to_vec();
    for t in &mut threads {
        if !matches!(t.status, thread::Status::Open | thread::Status::Starting)
            || !t.prompt_pending
            || t.startup_wait_started.is_empty()
            || t.launch_attempts == 0
            || t.startup_recovery_used
            || (t.status == thread::Status::Starting
                && thread::seconds_since(&t.startup_wait_started, jiff::Timestamp::now()).max(0)
                    as u64
                    * 1000
                    < agent_start_timeout(&t.launch))
            || agents.iter().any(|agent| agent.pane_id == t.pane_id)
            || !panes.iter().any(|pane| thread::pane_matches(t, pane))
        {
            continue;
        }
        if !remote
            .pane_process_info(&t.pane_id)
            .is_ok_and(|info| info.pane_id == t.pane_id && info.foreground_processes.is_empty())
        {
            continue;
        }
        if crate::remote::declaration_for_route(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            t.machine_route(),
        )
        .is_ok()
        {
            let updated = thread::update(project, &t.id, |record| {
                record.launch_attempts = 0;
                record.startup_recovery_used = true;
                record.startup_wait_started.clear();
                record.status = thread::Status::Open;
            })
            .map_err(|e| format!("{e:#}"))?;
            *t = updated;
        }
    }
    let prefix = coordinator::current_prefix(&ctx.root).map_err(|e| format!("{e:#}"))?;
    let state_input = LaunchPass {
        ctx,
        project,
        herdr: &remote,
        threads: &threads,
        agents: &agents,
        panes: &panes,
    };
    let state_pass =
        thread_pass(&state_input, &prefix, None, true).map_err(|e| format!("{e:#}"))?;
    errors.extend(state_pass.error);
    let launched = launch_pass(
        &LaunchPass {
            ctx,
            project,
            herdr,
            threads: &threads,
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
    // The D8 BLOCKED/GONE lines for this machine's box lanes (SPEC-remote §4.3).
    errors.extend(steps::remote_attention(
        ctx,
        project,
        steps::RemoteView {
            machine_id: &view.machine_id,
            threads: &threads,
            agents: &agents,
            panes: &panes,
            boot_id: &view.boot_id,
            now: jiff::Timestamp::now(),
        },
    ));
    Ok(())
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
            herdr.tab_close(&record.tab_id).err().map(|error| {
                anyhow::anyhow!("{machine}: close managed tab {}: {error}", record.tab_id)
            })
        })
        .collect()
}

/// Copies and launches, remote machines, then inbox items, pull requests,
/// routines and housekeeping.
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
    let _scope = crate::ledger::Scope::new(&[project]);
    let mut errors = Vec::new();
    if !step("escalation") {
        return (errors, false);
    }
    errors.extend(crate::escalation::tick(ctx, project).err());
    let herdr = Herdr::new(ctx.env.herdr_bin(), &seen.socket, ctx.runner);
    let now = jiff::Timestamp::now();
    let mut may_start = true;

    if !step("coordinator launch") {
        return (errors, false);
    }
    if let Some(record) = project.coordinator()
        && record.closed_by_rolf_at.is_empty()
        && (record.prime_pending
            || !record.last_agent_seen_at.is_empty()
            || record.reopen_requested)
        && seen
            .panes
            .iter()
            .any(|p| coordinator::pane_matches(&record, p))
        && !seen.agents.iter().any(|a| a.pane_id == record.pane_id)
        && {
            let last = steps::load_state(project).coordinator_relaunch_last;
            last.is_empty() || thread::seconds_since(&last, now) >= 3600
        }
    {
        may_start = false;
        errors.extend(restart_coordinator_agent(project, &herdr, &record).err());
    }

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
    let remote_threads = open_threads(project, true);
    let mut machines: Vec<String> = remote_threads
        .iter()
        .map(|t| t.machine_route().to_string())
        .collect();
    machines.sort();
    machines.dedup();
    for machine in machines {
        if !step(&format!("remote state {machine}")) {
            return (errors, false);
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
        let threads: Vec<thread::Thread> = remote_threads
            .iter()
            .filter(|t| t.machine_route() == machine)
            .cloned()
            .collect();
        match remote_pass(
            &LaunchPass {
                ctx,
                project,
                herdr: &herdr,
                threads: &threads,
                agents: &[],
                panes: &[],
            },
            &machine,
            view,
            &mut may_start,
            &mut errors,
        ) {
            Ok(()) => {}
            Err(error) => errors.push(anyhow::anyhow!("{machine}: {error}")),
        }
    }

    stop_after_state!("session notice");
    errors.extend(steps::session_notice(project, &mut state, seen.session_lost).err());
    stop_after_state!("pull requests");
    errors.extend(steps::pull_requests(ctx, project, &mut state, memory, now));
    stop_after_state!("routines");
    let zoned = jiff::Zoned::now();
    match project.read_project_md() {
        Ok((_settings, _)) => {
            let commands = project
                .safety(&ctx.config_dir)
                .map(|s| s.routine_commands)
                .unwrap_or(false);
            errors.extend(steps::routines(
                ctx, project, &mut state, commands, None, &zoned,
            ));
        }
        Err(error) => {
            let text = std::fs::read(project.project_md()).unwrap_or_default();
            let problem = Some((thread::sha256_hex(&text), format!("{error:#}")));
            errors.extend(steps::routines(
                ctx, project, &mut state, false, problem, &zoned,
            ));
        }
    }
    // D5 recovery and delivery (X1 to X5), then rounds, asks and the
    // board (D6, D17, D18). Each takes the project lock only for its own
    // file writes; git and herdr run outside it.
    stop_after_state!("ops");
    errors.extend(
        crate::ops::tick(ctx, project)
            .err()
            .map(|e| e.context("ops")),
    );
    stop_after_state!("rounds");
    errors.extend(
        crate::round::tick(ctx, project)
            .err()
            .map(|e| e.context("rounds")),
    );
    inbox::prune_done(project, steps::DONE_RETENTION_DAYS);
    if state != before {
        errors.extend(steps::save_state(project, &state).err());
    }
    (errors, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Env;
    use crate::runner::fake::{FakeRunner, fail, ok, timeout};

    fn held(version: &str) -> LockState {
        LockState::Held(Info {
            version: version.into(),
            ..Info::default()
        })
    }

    #[test]
    fn every_adapter_uses_its_routed_start_timeout() {
        let launch = crate::contracts::Launch {
            kind: "made-up".into(),
            ready_timeout_ms: 90_000,
            ..crate::contracts::Launch::default()
        };
        assert_eq!(agent_start_timeout(&launch), 90_000);
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
            "cat /proc/42/cgroup",
            ok("0::/user.slice/herdr-ade-demo.slice/run.scope\n"),
        );
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
        let herdr = Herdr::new("herdr", "", &runner);
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
        assert_eq!(runner.count("cat /proc/42/cgroup"), 3);
        assert!(runner.calls.borrow().iter().any(|call| {
            call.display()
                .contains("--env PATH=/home/agent/.herdr-ade/.state/slices/demo/bin")
        }));
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
        assert_eq!(rebound.checked_slice, "herdr-ade-demo.slice");
        assert!(!rebound.identity.pane_id.is_empty());
    }

    #[test]
    fn exhausted_box_start_with_saved_id_and_label_key_recovers_on_next_pass() {
        let fixture = fixture(false);
        std::fs::create_dir_all(fixture.root.join("cfg")).unwrap();
        std::fs::write(
            fixture.root.join("cfg/config.toml"),
            crate::remote::TEST_MACHINE,
        )
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "machine list --json",
            ok(r#"[{"id":"machine-1","label":"buildbox","target":"box","session":"default","enabled":true}]"#),
        );
        let probes = std::rc::Rc::new(std::cell::Cell::new(0));
        let counts = probes.clone();
        runner.on_fn(
            |cmd| cmd.display().contains("pane process-info"),
            move |_| {
                let n = counts.get();
                counts.set(n + 1);
                if n == 1 {
                    Ok(ok(r#"{"result":{"process_info":{"pane_id":"w2:p1","foreground_processes":[{"pid":42,"name":"claude","argv0":"claude"}]}}}"#))
                } else {
                    Ok(ok(r#"{"result":{"process_info":{"pane_id":"w2:p1","foreground_processes":[]}}}"#))
                }
            },
        );
        runner.on(
            "cat /proc/42/cgroup",
            ok("0::/user.slice/herdr-ade-demo.slice/run.scope\n"),
        );
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"workspace_id":"w2","tab_id":"w2:t1","pane_id":"w2:p1"}}}"#),
        );
        let record = thread::allocate(&fixture.project, |t| {
            t.status = thread::Status::Starting;
            t.prompt_pending = true;
            t.machine = "buildbox".into();
            t.machine_id = "machine-1".into();
            t.workspace_id = "w2".into();
            t.tab_id = "w2:t1".into();
            t.pane_id = "w2:p1".into();
            t.cwd = "/box/lane".into();
            t.agent_name = "hp-demo-t-0001".into();
            t.launch.kind = "claude".into();
            t.launch_attempts = thread::MAX_LAUNCH_ATTEMPTS;
            t.startup_wait_started = "2020-01-01T00:00:00Z".into();
        })
        .unwrap();
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "", &runner);
        let view = steps::CourierOutcome {
            machine_id: "machine-1".into(),
            boot_id: "boot".into(),
            agents: Some(vec![]),
            panes: Some(vec![Pane {
                workspace_id: record.workspace_id.clone(),
                tab_id: record.tab_id.clone(),
                pane_id: record.pane_id.clone(),
                cwd: record.cwd.clone(),
            }]),
        };
        let mut errors = Vec::new();
        remote_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: &[record],
                agents: &[],
                panes: &[],
            },
            "machine-1",
            &view,
            &mut true,
            &mut errors,
        )
        .unwrap();
        assert_eq!(runner.count("agent start"), 1, "{errors:#?}");
        let saved = thread::load(&fixture.project, "t-0001").unwrap();
        assert_eq!(saved.launch_attempts, 1);
        assert!(saved.startup_recovery_used);
        assert_eq!(saved.checked_slice, "herdr-ade-demo.slice");
        assert!(!saved.identity.pane_id.is_empty());
        assert!(saved.startup_wait_started.is_empty());

        // A later submitted start can itself time out. It must not be
        // reclaimed again and thereby evade the launch limit indefinitely.
        let exhausted = thread::update(&fixture.project, &saved.id, |t| {
            t.status = thread::Status::Starting;
            t.launch_attempts = thread::MAX_LAUNCH_ATTEMPTS;
            t.startup_wait_started = "2020-01-01T00:00:00Z".into();
        })
        .unwrap();
        remote_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: &[exhausted],
                agents: &[],
                panes: &[],
            },
            "machine-1",
            &view,
            &mut true,
            &mut errors,
        )
        .unwrap();
        assert_eq!(runner.count("agent start"), 1, "{errors:#?}");
        assert_eq!(
            thread::load(&fixture.project, &saved.id).unwrap().status,
            thread::Status::Failed
        );
    }

    #[test]
    fn box_agent_process_finds_shebang_pi_without_argv0() {
        let fixture = fixture(false);
        let runner = FakeRunner::new();
        runner.on("pane process-info", ok(r#"{"result":{"process_info":{"foreground_processes":[{"pid":41,"name":"node","argv":["/usr/bin/node","/tmp/unrelated.js"]},{"pid":42,"name":"node","argv":["/usr/bin/node","/home/agent/.local/lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js"]}]}}}"#));
        let herdr = Herdr::new("herdr", "", &runner);
        let record = thread::allocate(&fixture.project, |record| {
            record.pane_id = "w2:p1".into();
            record.launch.kind = "pi".into();
        })
        .unwrap();
        assert_eq!(box_agent_process(&herdr, &record).unwrap().pid, 42);
    }

    #[test]
    fn box_agent_outside_slice_fails_start_with_observed_cgroup() {
        let fixture = fixture(false);
        std::fs::create_dir_all(fixture.root.join("cfg")).unwrap();
        std::fs::write(
            fixture.root.join("cfg/config.toml"),
            crate::remote::TEST_MACHINE,
        )
        .unwrap();
        let runner = FakeRunner::new();
        runner.on("machine list --json", ok("[]"));
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"workspace_id":"w2","tab_id":"w2:t1","pane_id":"w2:p1"}}}"#),
        );
        runner.on("pane process-info", ok(r#"{"result":{"process_info":{"foreground_processes":[{"pid":42,"name":"claude","argv0":"claude"}]}}}"#));
        runner.on("cat /proc/42/cgroup", ok("0::/app.slice/herdr.service\n"));
        let record = thread::allocate(&fixture.project, |record| {
            record.status = thread::Status::Open;
            record.prompt_pending = true;
            record.machine = "buildbox".into();
            record.machine_id = "buildbox".into();
            record.workspace_id = "w2".into();
            record.tab_id = "w2:t1".into();
            record.pane_id = "w2:p1".into();
            record.cwd = "/box/lane".into();
            record.agent_name = "hp-demo-t-0001".into();
            record.launch.kind = "claude".into();
        })
        .unwrap();
        let ctx = Ctx {
            env: &fixture.env,
            root: fixture.root.clone(),
            config_dir: fixture.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "", &runner);
        let mut errors = Vec::new();
        launch_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: &[record],
                agents: &[],
                panes: &[Pane {
                    pane_id: "w2:p1".into(),
                    tab_id: "w2:t1".into(),
                    workspace_id: "w2".into(),
                    cwd: "/box/lane".into(),
                }],
            },
            &mut true,
            false,
            &mut errors,
        );
        let failed = thread::load(&fixture.project, "t-0001").unwrap();
        assert_eq!(failed.status, thread::Status::Failed);
        assert!(
            failed.error.contains("app.slice/herdr.service"),
            "{}; errors={errors:#?}",
            failed.error
        );
        assert!(failed.checked_slice.is_empty());
    }

    #[test]
    fn late_ready_box_agent_outside_slice_fails_before_prompt() {
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
        runner.on("cat /proc/42/cgroup", ok("0::/app.slice/herdr.service\n"));
        let record = thread::allocate(&fixture.project, |t| {
            t.status = thread::Status::Starting;
            t.prompt_pending = true;
            t.startup_wait_started = project::now();
            t.machine = "buildbox".into();
            t.machine_id = "buildbox".into();
            t.workspace_id = "w2".into();
            t.tab_id = "w2:t1".into();
            t.pane_id = "w2:p1".into();
            t.cwd = "/box/lane".into();
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
        let herdr = Herdr::new("herdr", "", &runner);
        let agent = Agent {
            pane_id: record.pane_id.clone(),
            tab_id: record.tab_id.clone(),
            workspace_id: record.workspace_id.clone(),
            cwd: record.cwd.clone(),
            name: record.agent_name.clone(),
            agent_status: "idle".into(),
            ..Agent::default()
        };
        let pass = thread_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: std::slice::from_ref(&record),
                agents: &[agent],
                panes: &[Pane {
                    pane_id: record.pane_id.clone(),
                    tab_id: record.tab_id.clone(),
                    workspace_id: record.workspace_id.clone(),
                    cwd: record.cwd.clone(),
                }],
            },
            "ha",
            None,
            true,
        )
        .unwrap();
        assert!(pass.error.is_some()); // This fixture has no live session for cleanup.
        let failed = thread::load(&fixture.project, "t-0001").unwrap();
        assert_eq!(failed.status, thread::Status::Failed);
        assert!(
            failed.error.contains("app.slice/herdr.service"),
            "{}",
            failed.error
        );
        assert_eq!(runner.count("agent prompt"), 0);
    }

    #[test]
    fn startup_block_keeps_the_screen_in_the_record_and_failure_list() {
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
        let herdr = Herdr::new("herdr", "", &runner);
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
        assert!(crate::ledger::list(&fixture.project).unwrap().is_empty());
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
            )
            .unwrap();
            assert_eq!(
                thread::load(&fixture.project, &record.id).unwrap().status,
                thread::Status::Starting
            );
            assert!(crate::ledger::list(&fixture.project).unwrap().is_empty());
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
        let failures = crate::ledger::list(&fixture.project).unwrap();
        assert!(
            failures
                .iter()
                .any(|e| e.kind == "thread-error" && e.detail.contains("Trust this folder?")),
            "{failures:?}"
        );
        assert!(!failures.iter().any(|e| e.kind == "launch-not-attempted"));
        assert!(errors.is_empty());
        assert_eq!(runner.count("agent start"), 1);
        assert_eq!(runner.count("agent prompt"), 1);
        assert!(
            runner
                .calls
                .borrow()
                .iter()
                .any(|call| call.display().contains("Trust this folder?")
                    && call.display().contains("agent prompt"))
        );
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
        let herdr = Herdr::new("herdr", "", &runner);
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
            assert!(crate::ledger::list(&fixture.project).unwrap().is_empty());
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
        let herdr = Herdr::new("herdr", "", &runner);
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
    fn a_detached_ticker_uses_the_projects_root_not_the_callers_folder() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("projects-root");
        std::fs::create_dir(&root).unwrap();
        let command = spawn_command(Path::new("/bin/true"), &root);
        assert_eq!(command.get_current_dir(), Some(root.as_path()));
    }

    #[test]
    fn start_decisions() {
        let mine = "0.1.0+abcdef0.20";
        assert_eq!(decide_start(&LockState::Free, mine), StartAction::Spawn);
        assert_eq!(
            decide_start(&held("0.1.0+abcdef0.10"), mine),
            StartAction::Nothing
        );
        assert_eq!(
            decide_start(&held("0.1.0+1234567.10"), mine),
            StartAction::StopThenSpawn
        );
    }

    /// `ensure` never writes the stop file, so `round advance` cannot deadlock
    /// waiting for a running ticker whose own pass waits on its lock.
    #[test]
    fn ensure_leaves_a_running_ticker_alone() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
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
        ensure(&ctx).unwrap();
        assert!(!stop_path(&root).exists(), "ensure writes no stop file");
        drop(file);
    }

    #[test]
    fn start_and_run_create_nothing_without_projects() {
        let home = tempfile::tempdir().unwrap();
        let missing = home.path().join("root");
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &env,
            root: missing.clone(),
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: true,
        };
        start(&ctx).unwrap();
        assert!(!missing.exists());
        run(&ctx).unwrap();
        assert!(!missing.exists());

        std::fs::create_dir(&missing).unwrap();
        start(&ctx).unwrap();
        run(&ctx).unwrap();
        assert_eq!(std::fs::read_dir(&missing).unwrap().count(), 0);
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
            LockState::Free => panic!("lock should be held"),
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
    fn stop_with_a_free_lock_removes_a_stale_stop_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(stop_path(root.path()), b"").unwrap();
        stop(root.path()).unwrap();
        assert!(!stop_path(root.path()).exists());
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
        start_for_install(&ctx).unwrap();
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
        ensure(&ctx).unwrap();
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
            fn socket_request(
                &self,
                socket: &Path,
                line: &str,
                timeout: Duration,
            ) -> Result<String> {
                self.fake.socket_request(socket, line, timeout)
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
        let start = Instant::now();
        let result = tick_with_steps(&ctx, &log, &mut memory, &mut |_| {
            !stop_path(&fixture.root).exists()
        });
        assert_eq!(result, None);
        assert!(start.elapsed() < Duration::from_millis(200));
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
                if name == "escalation" {
                    std::fs::write(stop_path(&fixture.root), b"").unwrap();
                }
                !stop_path(&fixture.root).exists() || name == "escalation"
            });
        assert!(!completed);
        assert!(errors.is_empty(), "{errors:#?}");
        assert_eq!(steps, ["escalation", "coordinator launch"]);
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
        start_for_install(&ctx).unwrap();
        assert!(start.elapsed() >= Duration::from_secs(5));
        release.join().unwrap();
        assert!(!stop_path(&root).exists());
    }

    #[test]
    fn ordinary_starts_defer_to_install_without_refusing() {
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
        start(&ctx).unwrap();
        ensure(&ctx).unwrap();
        assert!(!lock_path(&root).exists());
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
            "[coordinator]\nidle_nudge_minutes = 21\n",
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
    fn follow_ups_queued_during_start_arrive_after_the_brief_in_order() {
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
                op: format!("test-{sequence}"),
                thread: lane.id.clone(),
                attempt: 1,
                round: None,
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
            let dir = crate::round::events_dir(&f.project);
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
        let herdr = Herdr::new("herdr", "", &runner);
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
        thread::update(&f.project, &lane.id, |lane| {
            lane.bootstrap = "acknowledged".into()
        })
        .unwrap();
        let pass = run_pass();
        assert!(pass.error.is_none());

        let calls = runner.calls.borrow();
        let prompts: Vec<_> = calls
            .iter()
            .filter(|call| call.display().contains("agent prompt"))
            .map(|call| call.display())
            .collect();
        assert_eq!(prompts.len(), 3);
        assert!(prompts[0].contains("skill lane"), "{}", prompts[0]);
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
        let uncertain_runner = FakeRunner::new();
        uncertain_runner.on("agent prompt", timeout());
        uncertain_runner.on("report-metadata", ok(r#"{"result":{}}"#));
        let uncertain_ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &uncertain_runner,
            detached_ticker: false,
        };
        let uncertain_herdr = Herdr::new("herdr", "", &uncertain_runner);
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

    fn write_task(project: &Project, attempts: Vec<String>) {
        let task = crate::task::Task {
            id: "job-0001".into(),
            title: "Keep the project moving.".into(),
            authority: vec!["request:q-1".into()],
            acceptance: vec!["The next step is complete.".into()],
            attempts,
            created: project::now(),
            ..crate::task::Task::default()
        };
        std::fs::write(
            project.state_dir().join("tasks/job-0001.toml"),
            toml::to_string(&task).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn work_nudge_requires_a_ready_coordinator() {
        let f = fixture(false);
        write_task(&f.project, Vec::new());
        let runner = FakeRunner::new();
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let coordinator = f.project.coordinator().unwrap();
        let herdr = Herdr::new(ctx.env.herdr_bin(), &coordinator.socket, &runner);

        for status in ["working", "blocked"] {
            let agent = Agent {
                agent_status: status.into(),
                ..Agent::default()
            };
            nudge_idle_coordinator(
                &ctx,
                &f.project,
                &mut steps::State::default(),
                &herdr,
                &coordinator,
                Some(&agent),
                (&[], &[]),
            )
            .unwrap();
        }
        assert_eq!(runner.count("agent prompt"), 0);
    }

    #[test]
    fn another_work_nudge_requires_cooldown_and_a_coordinator_turn() {
        let f = fixture(false);
        let mut state = steps::State {
            idle_nudge_last: "2026-01-01T00:00:00Z".into(),
            ..steps::State::default()
        };
        let at_ten = "2026-01-01T00:10:00Z".parse().unwrap();
        let at_thirty = "2026-01-01T00:30:00Z".parse().unwrap();

        assert!(!idle_nudge_due(&f.project, &state, 20, at_ten).unwrap());
        assert!(!idle_nudge_due(&f.project, &state, 20, at_thirty).unwrap());
        crate::ledger::context_read(&f.project, "2026-01-01T00:05:00Z").unwrap();
        assert!(idle_nudge_due(&f.project, &state, 20, at_thirty).unwrap());

        state.idle_nudge_last = "2026-01-01T00:06:00Z".into();
        assert!(!idle_nudge_due(&f.project, &state, 20, at_thirty).unwrap());
        state.coordinator_worked_at = "2026-01-01T00:07:00Z".into();
        assert!(idle_nudge_due(&f.project, &state, 20, at_thirty).unwrap());
        state.coordinator_worked_at.clear();
        crate::ledger::coordinator_command(&f.project).unwrap();
        assert!(idle_nudge_due(&f.project, &state, 20, jiff::Timestamp::now()).unwrap());
        state.idle_nudge_last = project::now();
        assert!(
            idle_nudge_gate(&f.project, &state, 20, jiff::Timestamp::now())
                .unwrap()
                .unwrap()
                .starts_with("not due until")
        );
    }

    #[test]
    fn idle_nudge_status_explains_the_blocking_check() {
        let f = fixture(false);
        let (settings, _) = f.project.read_project_md().unwrap();
        let now = "2026-01-01T00:30:00Z".parse().unwrap();
        let state = steps::State {
            idle_nudge_last: "2026-01-01T00:00:00Z".into(),
            ..steps::State::default()
        };
        let agent = Agent {
            agent_status: "done".into(),
            ..Agent::default()
        };
        let line = |state: &steps::State, clear| {
            idle_nudge_status(
                &f.project,
                state,
                (&settings, 20),
                Some(&agent),
                (&[], &[]),
                clear,
                now,
            )
            .unwrap()
        };
        assert!(line(&state, Some(true)).ends_with("no turn since last nudge"));
        let state = steps::State {
            coordinator_worked_at: "2026-01-01T00:05:00Z".into(),
            ..state
        };
        assert!(line(&state, Some(false)).ends_with("input held"));
        assert!(line(&state, None).ends_with("input unavailable"));
        assert!(line(&state, Some(true)).ends_with("due"));
    }

    #[test]
    fn done_coordinator_is_nudged_once_per_interval_for_open_work() {
        let f = fixture(false);
        write_task(&f.project, Vec::new());
        let runner = FakeRunner::new();
        runner.on(
            "agent list",
            ok(&with_cwd(AGENT_READY, &f)
                .replace(r#""agent_status":"idle""#, r#""agent_status":"done""#)),
        );
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
        assert!(tick_project(&ctx, &f.project).unwrap());
        assert_eq!(runner.count("agent prompt"), 1);

        // A new context read and an elapsed interval permit another nudge,
        // even when the next action has not changed.
        let mut state = steps::load_state(&f.project);
        state.idle_nudge_last = "2026-01-01T00:00:00Z".into();
        steps::save_state(&f.project, &state).unwrap();
        crate::ledger::context_read(&f.project, &project::now()).unwrap();
        assert!(tick_project(&ctx, &f.project).unwrap());
        assert_eq!(runner.count("agent prompt"), 2);

        let calls = runner.calls.borrow();
        let prompt = calls
            .iter()
            .find(|call| call.display().contains("agent prompt"))
            .unwrap()
            .display();
        assert!(
            prompt.contains("job-0001: verify 1 acceptance condition(s)"),
            "{prompt}"
        );
        let ledger = std::fs::read_to_string(f.project.record_file("ledger.jsonl")).unwrap();
        assert!(ledger.contains("coordinator_nudge"), "{ledger}");
    }

    #[test]
    fn finished_lanes_with_an_idle_agent_nudge_the_done_coordinator() {
        let f = fixture(false);
        let mut agents = vec![serde_json::json!({
            "pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1",
            "name": "hp-demo-coordinator", "agent": "claude", "agent_status": "done",
            "cwd": f.project.dir().to_string_lossy(),
        })];
        let mut panes = vec![serde_json::json!({
            "pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1",
            "cwd": f.project.dir().to_string_lossy(),
        })];
        let dir = thread::threads_dir_for_write(&f.project).unwrap();
        for n in 95..=98 {
            let id = format!("t-{n:04}");
            let pane_id = format!("w1:p{n}");
            let lane = thread::Thread {
                id: id.clone(),
                status: thread::Status::Open,
                report_hash: format!("report-{n}"),
                last_group: if n == 95 {
                    "working"
                } else {
                    "ready-for-review"
                }
                .into(),
                last_state: if n == 95 { "idle" } else { "done" }.into(),
                // This lane's courier observation was idle, but the cached
                // group still says working. The cheap pass does not re-group
                // remote lanes before the coordinator nudge.
                machine: if n == 95 { "oci" } else { "" }.into(),
                workspace_id: "w1".into(),
                tab_id: "w1:t1".into(),
                pane_id: pane_id.clone(),
                agent_name: format!("hp-demo-{id}"),
                cwd: f.project.dir().to_string_lossy().into_owned(),
                ..thread::Thread::default()
            };
            std::fs::write(
                dir.join(format!("{id}.toml")),
                toml::to_string(&lane).unwrap(),
            )
            .unwrap();
            agents.push(serde_json::json!({
                "pane_id": pane_id, "tab_id": "w1:t1", "workspace_id": "w1",
                "name": lane.agent_name, "agent": "claude",
                "agent_status": if n == 95 { "idle" } else { "done" },
                "cwd": lane.cwd,
            }));
            panes.push(serde_json::json!({
                "pane_id": lane.pane_id, "tab_id": "w1:t1", "workspace_id": "w1",
                "cwd": f.project.dir().to_string_lossy(),
            }));
        }
        let runner = FakeRunner::new();
        runner.on(
            "agent list",
            ok(&serde_json::json!({"result": {"agents": agents}}).to_string()),
        );
        runner.on(
            "pane list",
            ok(&serde_json::json!({"result": {"panes": panes}}).to_string()),
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
        let mut state = steps::State {
            idle_nudge_last: "2026-01-01T00:00:00Z".into(),
            ..steps::State::default()
        };
        steps::save_state(&f.project, &state).unwrap();
        crate::ledger::context_read(&f.project, "2026-01-01T00:05:00Z").unwrap();
        assert!(tick_project(&ctx, &f.project).unwrap());
        assert!(tick_project(&ctx, &f.project).unwrap());
        assert_eq!(runner.count("agent prompt"), 1);
        let prompt = runner
            .calls
            .borrow()
            .iter()
            .find(|call| call.display().contains("agent prompt"))
            .unwrap()
            .display();
        for n in 95..=98 {
            assert!(
                prompt.contains(&format!("t-{n:04} is ready for review: open a round")),
                "{prompt}"
            );
        }
        state = steps::load_state(&f.project);
        assert_ne!(state.idle_nudge_last, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn remote_idle_group_does_not_keep_an_old_blocked_state_working() {
        let f = fixture(false);
        let lane = thread::allocate(&f.project, |lane| {
            lane.status = thread::Status::Open;
            lane.machine = "oci".into();
            lane.last_state = "blocked".into();
            lane.last_group = thread::Group::Idle.token().into();
        })
        .unwrap();
        let lines = idle_lane_lines(&f.project, &[], &[]).unwrap();
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("{} is idle", lane.id)))
        );
    }

    #[test]
    fn active_lanes_do_not_trigger_a_coordinator_nudge() {
        let f = fixture(false);
        let lane = thread::allocate(&f.project, |lane| {
            lane.status = thread::Status::Starting;
            lane.queued_for_load = true;
            lane.created = project::now();
        })
        .unwrap();
        assert!(idle_lane_lines(&f.project, &[], &[]).is_none());

        thread::update(&f.project, &lane.id, |lane| {
            lane.status = thread::Status::Open;
            lane.queued_for_load = false;
            lane.machine = "oci".into();
            lane.last_state = "blocked".into();
            lane.last_group = "working".into();
        })
        .unwrap();
        assert!(idle_lane_lines(&f.project, &[], &[]).is_none());

        thread::update(&f.project, &lane.id, |lane| {
            lane.report_hash = "report".into();
            // A sealed report is stronger than a stale courier observation.
        })
        .unwrap();
        assert!(
            idle_lane_lines(&f.project, &[], &[])
                .unwrap()
                .iter()
                .any(|line| line.contains("ready for review"))
        );
    }

    #[test]
    fn waiting_tasks_and_empty_queue_explain_the_next_step() {
        let f = fixture(false);
        write_task(&f.project, Vec::new());
        crate::task::set_wait(&f.project, "job-0001", "event", "next response").unwrap();
        let waiting = idle_nudge_text(&idle_task_lines(&f.project));
        assert!(
            waiting.contains("held: job-0001 waits on event next response"),
            "{waiting}"
        );
        assert!(waiting.contains("Everything open is held"), "{waiting}");
        assert!(waiting.contains("ha finish"), "{waiting}");
        crate::task::drop_task(&f.project, "job-0001", "Done.").unwrap();
        let empty = idle_nudge_text(&idle_task_lines(&f.project));
        assert!(empty.contains("No open tasks."), "{empty}");
        assert!(
            empty.contains("Ask Rolf only for what truly needs him"),
            "{empty}"
        );
    }

    #[test]
    fn held_tasks_are_listed_once_without_being_offered_as_work() {
        let f = fixture(false);
        write_task(&f.project, Vec::new());
        crate::task::set_wait(&f.project, "job-0001", "event", "provider response").unwrap();
        let mut lines = idle_task_lines(&f.project);
        let only_held = idle_nudge_text(&lines);
        assert_eq!(only_held.matches("held: job-0001").count(), 1);
        assert!(only_held.contains("Everything open is held"), "{only_held}");
        assert!(only_held.contains("ha finish"), "{only_held}");
        assert!(!only_held.contains("job-0001: verify"), "{only_held}");

        let task = crate::task::Task {
            id: "job-0002".into(),
            title: "Check another step".into(),
            authority: vec!["request:q-1".into()],
            acceptance: vec!["The other step is complete.".into()],
            created: project::now(),
            ..crate::task::Task::default()
        };
        std::fs::write(
            f.project.state_dir().join("tasks/job-0002.toml"),
            toml::to_string(&task).unwrap(),
        )
        .unwrap();
        lines = idle_task_lines(&f.project);
        let with_work = idle_nudge_text(&lines);
        assert_eq!(with_work.matches("held: job-0001").count(), 1);
        assert!(with_work.contains("job-0002: verify"), "{with_work}");
        assert!(
            with_work.contains("find and start other useful work"),
            "{with_work}"
        );
        assert!(
            !with_work.contains("Everything open is held"),
            "{with_work}"
        );
    }

    #[test]
    fn stalled_review_does_not_silence_an_idle_coordinator() {
        let f = fixture(false);
        let round = crate::contracts::RoundRecord {
            round: "r1".into(),
            phase: crate::contracts::RoundPhase::UnderReview,
            ..Default::default()
        };
        std::fs::create_dir_all(crate::round::rounds_dir(&f.project)).unwrap();
        std::fs::write(
            crate::round::round_path(&f.project, "r1"),
            toml::to_string(&round).unwrap(),
        )
        .unwrap();
        let runner = FakeRunner::new();
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let record = f.project.coordinator().unwrap();
        let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, &runner);
        nudge_idle_coordinator(
            &ctx,
            &f.project,
            &mut steps::State::default(),
            &herdr,
            &record,
            Some(&Agent {
                agent_status: "idle".into(),
                ..Agent::default()
            }),
            (&[], &[]),
        )
        .unwrap();
        assert_eq!(runner.count("agent prompt"), 1);
    }

    #[test]
    fn finished_project_stops_nudges_until_rolf_writes() {
        let f = fixture(false);
        let runner = FakeRunner::new();
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let record = f.project.coordinator().unwrap();
        let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, &runner);
        let agent = Agent {
            agent_status: "idle".into(),
            ..Agent::default()
        };
        f.project.set_finished(true).unwrap();
        assert!(
            coordinator::digest(&ctx, &f.project, "ha")
                .unwrap()
                .0
                .contains("Idle nudges are off")
        );
        nudge_idle_coordinator(
            &ctx,
            &f.project,
            &mut steps::State::default(),
            &herdr,
            &record,
            Some(&agent),
            (&[], &[]),
        )
        .unwrap();
        assert_eq!(runner.count("agent prompt"), 0);
        crate::talk::record_pane_request(&f.project, "Please continue.").unwrap();
        assert!(!f.project.finished());
        nudge_idle_coordinator(
            &ctx,
            &f.project,
            &mut steps::State::default(),
            &herdr,
            &record,
            Some(&agent),
            (&[], &[]),
        )
        .unwrap();
        assert_eq!(runner.count("agent prompt"), 1);
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
        // The board (workspace tokens) still publishes; the pane is untouched.
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
    fn shell_prompt_pane_retries_at_most_hourly() {
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
        assert_eq!(runner.count("agent start"), 1); // hourly bound, even after failure
        assert_eq!(runner.count("agent prompt"), 0);
    }

    #[test]
    fn dead_coordinator_agent_in_existing_pane_retries_hourly() {
        let f = fixture(false);
        f.project
            .update_coordinator(|c| {
                c.launch.kind = "claude".into();
                c.last_agent_seen_at = project::now();
            })
            .unwrap();
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
        let _ = tick_project(&ctx, &f.project);
        let _ = tick_project(&ctx, &f.project);
        assert_eq!(runner.count("agent start"), 1);
        assert!(
            f.project
                .coordinator()
                .unwrap()
                .closed_by_rolf_at
                .is_empty()
        );
    }

    #[test]
    fn closed_coordinator_is_marked_once_and_not_relaunched() {
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
        let mark = f.project.coordinator().unwrap().closed_by_rolf_at;
        assert!(!mark.is_empty());
        let _ = tick_project(&ctx, &f.project);
        assert_eq!(f.project.coordinator().unwrap().closed_by_rolf_at, mark);
        assert_eq!(runner.count("workspace create"), 0);
        assert!(
            steps::load_state(&f.project)
                .coordinator_relaunch_last
                .is_empty()
        );
        let board = crate::board::compute(&ctx, &f.project);
        assert!(
            board
                .iter()
                .any(|(key, value)| key == "ade_stage" && value.contains("closed by Rolf"))
        );
    }

    #[test]
    fn restarted_server_with_all_panes_gone_relaunches_at_most_hourly() {
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
        // Even if open fails, the attempt is recorded before launching.
        let _ = tick_project(&ctx, &f.project);
        let last = steps::load_state(&f.project).coordinator_relaunch_last;
        assert!(!last.is_empty());
        let _ = tick_project(&ctx, &f.project);
        assert_eq!(
            steps::load_state(&f.project).coordinator_relaunch_last,
            last
        );
        let ledger = std::fs::read_to_string(f.project.record_file("ledger.jsonl")).unwrap();
        assert_eq!(ledger.matches("coordinator_relaunch").count(), 1);
        assert_eq!(
            f.project.coordinator().unwrap().launch.recipe_id,
            "recorded-recipe"
        );
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

    #[test]
    fn log_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log {
            path: dir.path().join("log"),
        };
        let long = "x".repeat(10_000);
        for _ in 0..150 {
            log.line(&long);
        }
        let size = std::fs::metadata(&log.path).unwrap().len();
        assert!(size <= LOG_CAP, "{size}");
        assert!(size > LOG_CAP / 4);
    }
}
