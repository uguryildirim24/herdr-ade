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
/// `review` must not block while replacing a ticker: the ticker's own
/// pass calls `advance`, so waiting here would deadlock against the ticker
/// waiting on `advance`'s lock. Ordinary thread starts and explicit `ticker
/// start` calls attempt to replace a stale-version ticker when it can stop.
pub(crate) fn ensure(ctx: &Ctx) -> Result<()> {
    let root = &ctx.root;
    if !ctx.detached_ticker || project::list_slugs(root).is_empty() || install_in_progress(ctx) {
        return Ok(());
    }
    ensure_free(
        root,
        std::env::var_os("HERDR_ADE_TICKER_SUPERVISOR").is_some(),
        spawn,
    )
}

fn ensure_free(
    root: &Path,
    supervised: bool,
    spawn_ticker: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if lock_state(root) == LockState::Free {
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

/// Spawns the detached loop unless there is nothing to watch. It creates
/// nothing when the root does not exist or contains no projects, so a linked
/// plugin's `[[startup]]` is harmless in sessions that have no projects.
pub(crate) fn start(ctx: &Ctx) -> Result<()> {
    if std::env::var_os("HERDR_ADE_INSTALL_TICKER").is_some() {
        return start_for_install(ctx);
    }
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

fn mark_clean_exit(root: &Path, log: &Log) {
    if let Err(error) = project::write_atomic(&clean_exit_path(root), b"") {
        log.line(&format!(
            "could not mark intentional ticker exit: {error:#}"
        ));
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
    // A later unexpected exit must not inherit an earlier intentional exit.
    let _ = std::fs::remove_file(clean_exit_path(root));
    if recovery_path(root).exists() {
        let _ = std::fs::remove_file(recovery_path(root));
        let notice = format!(
            "ticker_unavailable: project transitions are queued; the ticker was restarted by launchd StartInterval at {}.",
            info.started
        );
        log.line(&notice);
        for slug in project::list_slugs(root) {
            if let Ok(project) = Project::load(root, &slug)
                && project.status() == Status::Active
                && let Err(error) =
                    inbox::write(&project, "ticker-unavailable", "ticker", &notice, "")
            {
                log.line(&format!("{slug}: recovery notice: {error:#}"));
            }
        }
    }
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
            mark_clean_exit(root, &log);
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
                mark_clean_exit(root, &log);
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
                mark_clean_exit(root, &log);
                return Ok(());
            }
            Some(true) => last_reachable = Instant::now(),
            Some(false) if last_reachable.elapsed() > IDLE_EXIT => {
                log.line("no project has had a reachable session for five minutes; exiting");
                mark_clean_exit(root, &log);
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
    if let Err(error) = crate::review::reclassify_old_changes(ctx, |message| log.line(message)) {
        log.line(&format!("one-time change reclassification: {error:#}"));
    }
    if let Err(error) = crate::branches::sweep_once(ctx, |message| log.line(message)) {
        log.line(&format!("one-time branch sweep: {error:#}"));
    }
    memory.machine_views.clear();
    let mut reachable = Vec::new();
    let mut readiness = BTreeMap::new();
    for slug in project::list_slugs(&ctx.root) {
        if !step(&format!("cheap project {slug}")) {
            return None;
        }
        let Ok(project) = Project::load(&ctx.root, &slug) else {
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
        resume_provider_starts(ctx, &project, &mut readiness, |error| {
            log.line(&format!("{slug}: {error:#}"));
        });
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
    // Courier imports sealed events and their report artifacts together.
    // Check reviews now, before remote state, launches, or plan work can
    // delay them. A lost local coordinator socket must not hide a box seal.
    for slug in project::list_slugs(&ctx.root) {
        if !step(&format!("reviews {slug}")) {
            return None;
        }
        let Ok(project) = Project::load(&ctx.root, &slug) else {
            continue;
        };
        if project.status() == Status::Active
            && let Err(error) = crate::review::tick(ctx, &project)
        {
            log.line(&format!("{slug}: reviews: {error:#}"));
        }
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
    resume_provider_starts(ctx, project, &mut BTreeMap::new(), |_| {});
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

/// Retry placement without using the routing retry or creating duplicate
/// panes. One readiness result is shared across projects for this pass.
pub(crate) fn resume_provider_starts(
    ctx: &Ctx,
    project: &Project,
    cache: &mut BTreeMap<(String, String), Result<(), String>>,
    mut report: impl FnMut(anyhow::Error),
) {
    for lane in thread::list(project) {
        if lane.provider_wait_started.is_empty()
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
        if thread::seconds_since(&lane.provider_wait_started, jiff::Timestamp::now()) >= 3600 {
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
                crate::contracts::FailureClass::Provider,
                false,
            ) {
                report(error);
            }
            continue;
        }
        let machine = if lane.is_remote() {
            lane.machine_route()
        } else {
            crate::contracts::MACHINE_LOCAL
        };
        let key = (machine.to_string(), lane.launch.recipe_id.clone());
        let ready = cache.entry(key).or_insert_with(|| {
            let result = if lane.is_remote() {
                threads::box_launch_ready_for(ctx, machine, &lane.launch)
            } else {
                crate::doctor::recipe_ready_local(ctx, &lane.launch)
            };
            result.map_err(|error| format!("{error:#}"))
        });
        if ready.is_err() {
            continue;
        }
        if lane.status == thread::Status::Open {
            if let Err(error) = thread::update(project, &lane.id, |t| {
                t.provider_wait_started.clear();
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
                && crate::prompt::writer_lock(input.project).is_ok_and(|_writer| {
                    crate::prompt::coordinator_prompt_clear(
                        input.project,
                        input.herdr,
                        &coordinator.pane_id,
                    )
                    .unwrap_or(false)
                        && crate::prompt::mark_automated_prompt(
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

fn connection_error(screen: &str) -> Option<&'static str> {
    let screen = screen.to_ascii_lowercase();
    if [
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
    {
        Some(
            if screen.contains("provider") || screen.contains("service unavailable") {
                "provider"
            } else {
                "connection"
            },
        )
    } else {
        None
    }
}

/// Close only the delivered follow-ups overtaking this exact seal. An unknown
/// machine/git observation leaves the seal pending, never presumed unchanged.
pub(crate) fn restore_unchanged_seal(
    ctx: &Ctx,
    project: &Project,
    lane: &thread::Thread,
) -> Result<()> {
    let events = crate::events::list(project);
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
    if done.sha.is_empty() || done.artifact.is_empty() || lane.worktree_path.is_empty() {
        return Ok(());
    }
    let folder = &lane.worktree_path;
    let report = Path::new(&done.report_path);
    let report = if report.is_absolute() {
        report.to_path_buf()
    } else {
        Path::new(folder).join(report)
    };
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
                "cd {} && git rev-parse HEAD && git status --porcelain --untracked-files=all && sha256sum -- {}",
                crate::remote::quote(folder),
                crate::remote::quote(&report.to_string_lossy())
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
            && output
                .stdout
                .lines()
                .nth(1)
                .is_some_and(|line| line.split_whitespace().next() == Some(done.artifact.as_str()))
            && output.stdout.lines().count() == 2
    } else {
        let git = |args: &[&str]| -> Result<String> {
            let output = ctx.runner.run(
                &crate::runner::Cmd::new("git", Duration::from_secs(20))
                    .args(["-C", folder])
                    .args(args.iter().copied()),
            )?;
            if !output.success() {
                bail!("git seal check: {}", output.error_text());
            }
            Ok(output.stdout.trim().to_string())
        };
        git(&["rev-parse", "HEAD"])? == done.sha
            && git(&["status", "--porcelain", "--untracked-files=all"])?.is_empty()
            && std::fs::read(report).is_ok_and(|bytes| thread::sha256_hex(&bytes) == done.artifact)
    };
    if unchanged {
        thread::update_checked(project, &lane.id, |current| {
            if current.attempt != lane.attempt {
                bail!("lane changed during seal check");
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

fn thread_pass(
    input: &LaunchPass<'_>,
    prefix: &str,
    hashes: Option<&std::collections::BTreeMap<String, String>>,
    refresh_tokens: bool,
    box_progress: Option<&std::collections::BTreeMap<(String, String), steps::LaneProgress>>,
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
        if t.status == thread::Status::Failed {
            let records = thread::list(project);
            if let Some(agent) = thread::recoverable_agent(t, &records, agents, panes) {
                thread::update_checked(project, &t.id, |current| {
                    if current.status == thread::Status::Failed
                        && current.attempt == t.attempt
                        && current.pane_id == t.pane_id
                        && !thread::list(project).iter().any(|other| {
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
        if t.parked || !t.provider_wait_started.is_empty() {
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
        if !t.startup_wait_started.is_empty() {
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
        // A CLI that lost its transport can still have a live, idle session.
        // Resume that session instead of replacing the process or consuming a
        // launch retry. Only the CLI's own visible error is evidence of loss.
        if state == "idle"
            && t.status == thread::Status::Open
            && !t.prompt_pending
            && t.bootstrap == "acknowledged"
            && !t.connection_waiting
            && t.report_hash.is_empty()
            && crate::events::latest_done_event(
                &crate::events::list(project),
                &t.id,
                t.attempt.max(1),
            )
            .is_none()
            && let Ok(screen) = herdr.pane_read_text(&t.pane_id, "visible")
            && let Some(kind) = connection_error(&screen)
        {
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
                    record.failure_class = crate::contracts::FailureClass::LostConnection;
                    record.provider_failure_kind = (kind == "provider").then(|| kind.into());
                    record.last_group = thread::Group::WaitingOnYou.token().into();
                })?;
                crate::inbox::write(
                    project,
                    "lane-connection",
                    &t.id,
                    "Connection recovery paused after three in-place resumes within an hour; check the lane pane.",
                    "",
                )?;
                continue;
            }
            if recent
                .last()
                .is_none_or(|at| thread::seconds_since(at, now) >= 60)
            {
                // Herdr sends to this exact pane; no new session or attempt.
                herdr.agent_prompt(&t.pane_id, "The connection dropped. Continue the current task in this session from where you stopped; do not start a new attempt.")?;
                thread::update(project, &t.id, |record| {
                    record.connection_resumes = recent;
                    record.connection_resumes.push(project::now());
                    record.failure_class = crate::contracts::FailureClass::LostConnection;
                    record.provider_failure_kind = (kind == "provider").then(|| kind.into());
                })?;
                continue;
            }
        }
        let mut delivered = false;
        if t.prompt_pending && ready {
            // The CLI and ticker can observe the same ready agent. Serialize
            // the first prompt and recheck its attempt before either sends it.
            let lock_path = project.state_dir().join(format!("brief-{}.lock", t.id));
            let lock = std::fs::File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(lock_path)?;
            lock.lock()?;
            let current = thread::load(project, &t.id)?;
            if current.attempt == t.attempt
                && current.pane_id == t.pane_id
                && current.status == thread::Status::Open
                && current.prompt_pending
            {
                match herdr.agent_prompt_wait_started(
                    &t.pane_id,
                    &thread::launch_prompt(prefix, slug, &current),
                    agent_start_timeout(&t.launch),
                ) {
                    Ok(()) => {
                        delivered = true;
                        thread::update_checked(project, &t.id, |record| {
                            if record.attempt == current.attempt
                                && record.pane_id == current.pane_id
                            {
                                record.prompt_pending = false;
                            }
                            Ok(())
                        })?;
                    }
                    Err(error) if error.code == "agent_not_ready" => {
                        // Registration can disappear between the list and the
                        // prompt. Keep the brief pending for the next pass.
                    }
                    Err(error) => {
                        pass.error = pass
                            .error
                            .or(Some(anyhow::anyhow!("{}: brief prompt: {error}", t.id)))
                    }
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
                    crate::events::latest_done_event(&crate::events::list(project), &t.id, attempt)
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
                            thread.connection_waiting = false;
                            thread.connection_resumes.clear();
                            thread.failure_class = crate::contracts::FailureClass::Unknown;
                            thread.provider_failure_kind = None;
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
        // After startup, a sustained blocked state needs a human. Do not
        // interact with the lane's pane: only the startup trust matcher can
        // answer its exact dialog. The group transition arms one alert per
        // blocked spell and a return to working re-arms the next one.
        if after.status == thread::Status::Open
            && after.startup_wait_started.is_empty()
            && state == "blocked"
            && group == thread::Group::WaitingOnYou
            && t.last_group != group.token()
        {
            let notice = format!(
                "BLOCKED {} awaits interactive approval in {}; no keys were sent. Answer it there or retry after resolving the permission.",
                t.id, t.pane_id
            );
            let sent = if let Some(coordinator) = project.coordinator() {
                agents
                    .iter()
                    .any(|agent| coordinator::agent_matches(&coordinator, agent) && agent.ready())
                    && steps::deliver_coordinator_prompt(
                        project,
                        herdr,
                        &coordinator.pane_id,
                        &notice,
                    )?
            } else {
                false
            };
            if !sent {
                inbox::write(project, "lane-notice", &t.id, &notice, "")?;
            }
        }
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
                            ctx.runner
                                .run(
                                    &crate::runner::Cmd::new("git", Duration::from_secs(5)).args([
                                        "-C",
                                        folder,
                                        "rev-parse",
                                        &format!("refs/heads/{}", t.branch),
                                    ]),
                                )
                                .ok()
                                .filter(|out| out.success())
                                .map(|out| out.stdout.trim().to_string())
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
                                coordinator::agent_matches(&coordinator, agent) && agent.ready()
                            }) && steps::deliver_coordinator_prompt(
                                project,
                                herdr,
                                &coordinator.pane_id,
                                &notice,
                            )?
                        } else {
                            false
                        };
                        if !sent {
                            inbox::write(project, "lane-notice", &t.id, &notice, "")?;
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
        // Only absence of the pane proves a local process is gone. Herdr may
        // temporarily omit agent state while the terminal and process still
        // exist (including after an interactive startup timeout); that state
        // is Unknown and must never authorize closing the pane.
        let process_gone = !t.is_remote()
            && (after.startup_wait_started.is_empty()
                || thread::seconds_since(&after.startup_wait_started, now).max(0) as u64 * 1000
                    >= agent_start_timeout(&after.launch))
            && after.report_hash.is_empty()
            && !threads::parkable(project, &after)
            && !live.pane_exists;
        if process_gone && !whole_session_missing {
            // The pane list may predate this record: a concurrent start or
            // retry can place a tab after the pass took its snapshot. Absence
            // must be observed again after placement before closing anything.
            let current = thread::load(project, &t.id)?;
            if current.status != thread::Status::Open
                || current.attempt != t.attempt
                || current.pane_id != t.pane_id
                || current.tab_id != t.tab_id
                || current.workspace_id != t.workspace_id
            {
                continue;
            }
            match herdr.pane_list() {
                Ok(panes)
                    if panes.iter().any(|pane| {
                        pane.pane_id == current.pane_id
                            && pane.tab_id == current.tab_id
                            && pane.workspace_id == current.workspace_id
                    }) =>
                {
                    continue;
                }
                Ok(_) => {}
                Err(error) => {
                    pass.error = pass.error.or(Some(anyhow::anyhow!(
                        "{}: recheck missing pane: {error}",
                        t.id
                    )));
                    continue;
                }
            }
            let recover = !t.launch.recipe_id.is_empty();
            if let Err(error) = threads::fail_start_checked(
                ctx,
                project,
                &t.id,
                "the pane or agent is gone without a report",
                crate::contracts::FailureClass::ProcessGone,
                recover,
                Some(&current),
            ) {
                pass.error = pass
                    .error
                    .or(Some(error.context(format!("{}: process recovery", t.id))));
            }
            continue;
        }
        // A delivered correction may simply acknowledge the existing seal. Only
        // restore it after the agent has returned to idle and the sealed git
        // state and report have been checked on the lane's own machine.
        if !delivered
            && state == "idle"
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

/// Start a just-placed attempt in the caller, without a later ticker or
/// courier pass. The periodic path still owns unfinished startup and retries.
pub(crate) fn launch_thread_now(ctx: &Ctx, project: &Project, id: &str) -> Result<()> {
    launch_thread_with_wait(ctx, project, id, Duration::from_secs(20))
}

fn launch_thread_with_wait(ctx: &Ctx, project: &Project, id: &str, wait: Duration) -> Result<()> {
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
        if live.agent_state.is_some() || !live.pane_exists || !t.startup_wait_started.is_empty() {
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
            errors.extend(
                thread::update(pass.project, &t.id, |record| record.error = message.clone()).err(),
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
        // The CLI and the ticker can race on the same newly placed pane.
        // Claim the launch under the record lock before issuing `agent start`.
        let mut claimed = false;
        match thread::update_checked(pass.project, &t.id, |current| {
            if current.attempt != t.attempt
                || current.pane_id != t.pane_id
                || current.status != thread::Status::Open
                || !current.prompt_pending
                || !current.startup_wait_started.is_empty()
            {
                return Ok(());
            }
            current.launch_attempts += 1;
            if current.error == "provider ready" || current.error.starts_with("disk_low:") {
                current.error.clear();
            }
            current.trust_answered = false;
            current.startup_wait_started = project::now();
            claimed = true;
            Ok(())
        }) {
            Ok(_) if claimed => {
                pending.push(t);
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
    let starts: Vec<_> = pending
        .iter()
        .map(|t| crate::herdr::AgentStart {
            name: &t.agent_name,
            kind: &t.launch.kind,
            pane: &t.pane_id,
            agent_args: &t.launch.args,
            launch_bin: None,
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
                && !t.parked
                && (t.provider_wait_started.is_empty()
                    || t.status == thread::Status::Open
                    || !remote && t.status == thread::Status::Failed)
                && (matches!(t.status, thread::Status::Open | thread::Status::Starting)
                    || !remote && t.status == thread::Status::Failed)
        })
        .collect()
}

pub(crate) fn socket_inode(path: &std::path::Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map_or(0, |meta| meta.ino())
}

/// Returns `Ok(None)` when the project's session cannot be reached: then no
/// state is read, so nothing is ever reported as gone.
fn tick_cheap(ctx: &Ctx, project: &Project, refresh_tokens: bool) -> Result<Option<Seen>> {
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
            let _writer = crate::prompt::writer_lock(project)?;
            if crate::prompt::coordinator_prompt_clear(project, &herdr, &record.pane_id)? {
                crate::prompt::mark_automated_prompt(project, &record.pane_id, &prompt)?;
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
        None,
    )?;
    first_error = first_error.or(pass.error);
    if let Err(error) = crate::threads::tick(project, &herdr, &agents) {
        first_error = first_error.or(Some(error));
    }
    // The ops pass (A2) and the reviews pass (A3) run in the slow pass,
    // outside the project lock (SPEC-ADE item 57).
    let pane_alive = panes.iter().any(|p| coordinator::pane_matches(&record, p));
    let inode = socket_inode(Path::new(&record.socket));
    if pane_alive && inode != 0 && record.server_socket_inode != inode {
        project.update_coordinator(|c| c.server_socket_inode = inode)?;
    }
    // A reused pane id after a server restart is not the original process.
    let bound_pane =
        pane_alive && (record.server_socket_inode == 0 || record.server_socket_inode == inode);
    if let Err(error) = coordinator::recover(project, &herdr, &record, agent.as_ref(), bound_pane) {
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
    let state_pass = thread_pass(&state_input, &prefix, None, true, Some(&view.progress))
        .map_err(|e| format!("{e:#}"))?;
    errors.extend(state_pass.error);
    // Inspect the courier snapshot before launching. A vanished pre-launch
    // pane must not consume an agent start against a terminal that no longer
    // exists; the recovery transition owns the next placement.
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

/// Nudges only a bound, live, idle coordinator. The state tracks changes
/// even if a lane starts and finishes between two ticker passes.
fn plan_nudge(
    project: &Project,
    herdr: &Herdr<'_>,
    agents: &[Agent],
    state: &mut steps::State,
) -> Result<()> {
    let lanes = thread::list(project);
    let mut lane_ids: Vec<_> = lanes
        .iter()
        .filter(|lane| lane.role != "reviewer")
        .map(|lane| lane.id.clone())
        .collect();
    lane_ids.sort();
    let asks = crate::ask::open_asks(project);
    // Answering an ask re-arms the normal nudge, even without a plan edit.
    let request = format!(
        "{}|{}",
        crate::prompt::latest_request_id(project),
        asks.iter()
            .map(|ask| format!("{}:{}", ask.id, ask.revision))
            .collect::<Vec<_>>()
            .join(",")
    );
    let plan = crate::plan::load(project)?;
    let revision = plan.as_ref().map_or(0, |p| p.revision);
    if state.plan_revision != revision
        || state.plan_lane_ids != lane_ids
        || state.plan_request != request
    {
        state.plan_nudged = false;
    }
    state.plan_revision = revision;
    state.plan_lane_ids = lane_ids;
    state.plan_request = request;

    if lanes.iter().any(|lane| {
        lane.role != "reviewer"
            && matches!(lane.status, thread::Status::Starting | thread::Status::Open)
    }) {
        state.plan_nudged = false;
        return Ok(());
    }
    if state.plan_nudged {
        return Ok(());
    }
    if crate::review::list(project)?
        .iter()
        .any(|review| !review.phase.closed())
    {
        return Ok(());
    }
    let Some(coordinator) = project.coordinator() else {
        return Ok(());
    };
    if !coordinator.closed_by_rolf_at.is_empty()
        || coordinator.prime_pending
        || coordinator::paused_by_provider(project, &coordinator)
    {
        return Ok(());
    }
    if !agents.iter().any(|agent| {
        coordinator::agent_matches(&coordinator, agent) && agent.agent_status == "idle"
    }) {
        return Ok(());
    }
    let Some(ref plan) = plan else {
        return Ok(());
    };
    // Transitions refresh the persisted plan. Re-deriving every step just to
    // decide whether to nudge rereads all task/review evidence each pass.
    let left: Vec<_> = plan
        .steps
        .iter()
        .filter(|step| step.state != crate::contracts::StepState::Done)
        .collect();
    if left.is_empty() {
        return Ok(());
    }
    // An unbound ask has no plan evidence of independence. Every linked ask
    // must map to at least one step before any step can be suggested.
    let tasks = if asks.is_empty() {
        Vec::new()
    } else {
        let (tasks, errors) = crate::task::list_with_errors(project);
        if !errors.is_empty() {
            return Ok(());
        }
        tasks
    };
    let mut ask_steps = std::collections::BTreeSet::new();
    for ask in &asks {
        let Some(task_id) = &ask.task else {
            return Ok(());
        };
        let bound: Vec<_> = crate::plan::all_steps(plan)
            .filter(|step| {
                step.tasks.contains(task_id)
                    || tasks.iter().any(|task| {
                        &task.id == task_id && task.plan_step.as_deref() == Some(&step.id)
                    })
            })
            .map(|step| step.id.clone())
            .collect();
        if bound.is_empty() {
            return Ok(());
        }
        for id in bound {
            ask_steps.insert(id.clone());
            for parent in &plan.steps {
                if parent.subtasks.iter().any(|sub| sub.id == id) {
                    ask_steps.insert(parent.id.clone());
                }
            }
        }
    }
    let next = left
        .iter()
        .filter(|step| step.state == crate::contracts::StepState::Left)
        .find(|step| {
            if !ask_steps.is_empty() {
                // Any direct or transitive --after edge to an ask-bound step
                // makes this step dependent on Rolf's unanswered call.
                let mut seen = std::collections::BTreeSet::new();
                let mut pending = vec![step.id.as_str()];
                while let Some(id) = pending.pop() {
                    if !seen.insert(id) {
                        continue;
                    }
                    if ask_steps.contains(id) {
                        return false;
                    }
                    let Some(bound) = crate::plan::all_steps(plan).find(|s| s.id == id) else {
                        return false;
                    };
                    pending.extend(bound.after.iter().map(String::as_str));
                }
            }
            // Use the same evidence gate as `thread start` for bound tasks.
            let gate = step
                .tasks
                .iter()
                .try_for_each(|job| crate::plan::check_prerequisites(project, job));
            if gate.is_err() {
                return false;
            }
            let waiting: Vec<_> = step
                .after
                .iter()
                .filter(|id| {
                    crate::plan::all_steps(plan)
                        .find(|candidate| &candidate.id == *id)
                        .is_none_or(|candidate| {
                            candidate.state != crate::contracts::StepState::Done
                        })
                })
                .collect();
            if !waiting.is_empty() {
                return false;
            }
            true
        });
    if !asks.is_empty() && next.is_none() {
        return Ok(());
    }
    let line = if !asks.is_empty() {
        let next = next.expect("open asks require an independent step");
        format!(
            "{} Rolf has an unanswered ask; independent step {} is ready. Start only work that does not require that answer.",
            steps::TICKER_PROMPT_PREFIX,
            next.id
        )
    } else if let Some(next) = next {
        format!(
            "{} Nothing is running and {} steps are left. Next: {} {}. Start its lanes, or ask Rolf if it needs his call.",
            steps::TICKER_PROMPT_PREFIX,
            left.len(),
            next.id,
            next.text
        )
    } else {
        let waiting = left
            .iter()
            .map(|step| {
                if step.state == crate::contracts::StepState::Running {
                    format!("{} is still running", step.id)
                } else {
                    format!("{} waits for {}", step.id, step.after.join(", "))
                }
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "{} Nothing is running and {} steps are left. {}.",
            steps::TICKER_PROMPT_PREFIX,
            left.len(),
            waiting
        )
    };
    if steps::deliver_coordinator_prompt(project, herdr, &coordinator.pane_id, &line)? {
        state.plan_nudged = true;
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
    // D5 recovery and delivery (X1 to X5), then asks (D6, D17, D18).
    // Reviews ran immediately after courier import. Each takes the project
    // lock only for its own file writes.
    stop_after_state!("ops");
    errors.extend(
        crate::ops::tick(ctx, project)
            .err()
            .map(|e| e.context("ops")),
    );
    errors.extend(crate::ask::tick(ctx, project).err());
    errors.extend(crate::threads::park_completed(ctx, project).err());
    stop_after_state!("plan nudge");
    errors.extend(plan_nudge(project, &herdr, &seen.agents, &mut state).err());
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
    fn progress_notices_are_once_per_spell_and_commit_clock_is_independent() {
        let now = jiff::Timestamp::now();
        let config = ProgressThresholds::default();
        assert_eq!((config.stall_minutes, config.no_commit_minutes), (20, 90));
        let custom: ProgressThresholds =
            toml::from_str("stall_minutes = 2\nno_commit_minutes = 4").unwrap();
        assert_eq!((custom.stall_minutes, custom.no_commit_minutes), (2, 4));
        let mut lane = thread::Thread {
            repo: "/repo".into(),
            ..Default::default()
        };
        let mut seen = steps::LaneProgress {
            pane: "w:p".into(),
            screen: "a".into(),
            head: "one".into(),
        };
        assert!(
            progress_notice_lines(
                &progress_notices(&mut lane, &seen, now, &config),
                "t-1",
                "w:p"
            )
            .is_empty()
        );
        lane.progress_since = (now - jiff::Span::new().minutes(21)).to_string();
        lane.no_commit_since = (now - jiff::Span::new().minutes(91)).to_string();
        let lines = progress_notice_lines(
            &progress_notices(&mut lane, &seen, now, &config),
            "t-1",
            "w:p",
        );
        assert_eq!(
            lines,
            vec![
                "STALLED t-1 has shown no new output and no new commit for 21 min in w:p; check it, then ha thread prompt, ha thread retry --reason, or cancel.",
                "t-1 has worked 91 min with no commit; check it is still on its task.",
            ]
        );
        assert!(
            progress_notice_lines(
                &progress_notices(&mut lane, &seen, now, &config),
                "t-1",
                "w:p"
            )
            .is_empty()
        );
        seen.screen = "b".into();
        assert!(
            progress_notices(&mut lane, &seen, now, &config)
                .stalled
                .is_none()
        );
        assert!(!lane.stall_notified);
        assert!(lane.no_commit_notified);
        lane.progress_since = (now - jiff::Span::new().minutes(21)).to_string();
        assert!(
            progress_notices(&mut lane, &seen, now, &config)
                .stalled
                .is_some()
        );
        seen.head = "two".into();
        progress_notices(&mut lane, &seen, now, &config);
        assert!(!lane.no_commit_notified);
        assert!(!lane.stall_notified);
        reset_progress(&mut lane);
        assert!(lane.progress_since.is_empty());
        lane.repo.clear();
        progress_notices(&mut lane, &seen, now, &config);
        lane.no_commit_since = (now - jiff::Span::new().minutes(91)).to_string();
        assert!(
            progress_notices(&mut lane, &seen, now, &config)
                .no_commit
                .is_none()
        );
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
        assert!(
            failed.error.contains("provider_wait_expired"),
            "{}",
            failed.error
        );
        assert!(failed.error.contains("one hour"), "{}", failed.error);
    }

    #[test]
    fn deferred_launch_waits_for_disk_then_submits() {
        use crate::scenarios::{World, pane_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("lane");
        let lane = world.thread(&project, &cwd, |record| {
            record.prompt_pending = true;
            record.launch.kind = "claude".into();
        });
        let runner = FakeRunner::new();
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
        free.set(20);
        launch_pass(&pass, &mut true, false, &mut errors);
        assert_eq!(runner.count("agent start"), 1);
        assert_eq!(thread::load(&project, &lane.id).unwrap().launch_attempts, 1);
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
    }

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
            progress: Default::default(),
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
        assert!(!saved.identity.pane_id.is_empty());
        assert!(!saved.startup_wait_started.is_empty());

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
    fn idle_connection_error_resumes_in_place_and_stops_after_three_in_an_hour() {
        let fixture = fixture(false);
        let runner = FakeRunner::new();
        runner.on("pane read", ok("WebSocket closed\n"));
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let record = thread::allocate(&fixture.project, |t| {
            t.status = thread::Status::Open;
            t.bootstrap = "acknowledged".into();
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
        let herdr = Herdr::new("herdr", "", &runner);
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
        let herdr = Herdr::new("herdr", "", &runner);
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
        assert_eq!(runner.count("agent prompt"), 1);
        assert!(
            runner
                .calls
                .borrow()
                .iter()
                .any(|call| call.display().contains(&format!(
                    "BLOCKED {} awaits interactive approval in w1:p2; no keys were sent",
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
    fn a_ready_box_agent_replaces_the_starting_tab_title() {
        let fixture = fixture(false);
        let runner = FakeRunner::new();
        runner.on("tab rename", ok(r#"{"result":{}}"#));
        let record = thread::allocate(&fixture.project, |t| {
            t.status = thread::Status::Starting;
            t.machine = "box".into();
            t.machine_id = "abc".into();
            t.startup_wait_started = project::now();
            t.pane_id = "w2:p1".into();
            t.tab_id = "w2:t1".into();
            t.workspace_id = "w2".into();
            t.cwd = "/box/lane".into();
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
        let pane = Pane {
            pane_id: record.pane_id.clone(),
            tab_id: record.tab_id.clone(),
            workspace_id: record.workspace_id.clone(),
            cwd: record.cwd.clone(),
        };
        let agent = Agent {
            pane_id: record.pane_id.clone(),
            tab_id: record.tab_id.clone(),
            workspace_id: record.workspace_id.clone(),
            cwd: record.cwd.clone(),
            name: record.agent_name.clone(),
            agent_status: "idle".into(),
            ..Agent::default()
        };
        thread_pass(
            &LaunchPass {
                ctx: &ctx,
                project: &fixture.project,
                herdr: &herdr,
                threads: std::slice::from_ref(&record),
                agents: &[agent],
                panes: &[pane],
            },
            "ha",
            None,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            thread::load(&fixture.project, &record.id).unwrap().status,
            thread::Status::Open
        );
        assert!(runner.calls.borrow().iter().any(|c| {
            c.display()
                .contains(&format!("tab rename {} {}", record.tab_id, record.id))
        }));
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
        let herdr = Herdr::new("herdr", "", &runner);
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
        let herdr = Herdr::new("herdr", "", &runner);
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

    /// `ensure` never writes the stop file, so `review` cannot deadlock
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
        let herdr = Herdr::new("herdr", "", &runner);
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

    fn nudge_pass(f: &Fixture, runner: &FakeRunner, agents: &[Agent]) {
        let herdr = Herdr::new(
            f.env.herdr_bin(),
            &f.project.coordinator().unwrap().socket,
            runner,
        );
        let mut state = steps::load_state(&f.project);
        plan_nudge(&f.project, &herdr, agents, &mut state).unwrap();
        steps::save_state(&f.project, &state).unwrap();
    }

    fn nudge_setup() -> (Fixture, FakeRunner, Agent) {
        let f = fixture(false);
        let runner = FakeRunner::new();
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
    fn idle_plan_sends_once_and_names_the_next_step() {
        let (f, runner, agent) = nudge_setup();
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        crate::plan::step_add(&ctx, "demo", "First outcome", vec![], vec![], None).unwrap();
        crate::plan::step_add(&ctx, "demo", "Second outcome", vec![], vec![], None).unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        nudge_pass(&f, &runner, &[agent]);
        assert_eq!(runner.count("agent prompt"), 1);
        let calls = runner.calls.borrow();
        assert!(calls.iter().any(|call| {
            call.display()
                .contains("2 steps are left. Next: s-1 First outcome")
        }));
    }

    #[test]
    fn idle_nudge_skips_blocked_steps_and_names_waiting_edges() {
        let (f, runner, agent) = nudge_setup();
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        crate::plan::step_add(&ctx, "demo", "First", vec![], vec![], None).unwrap();
        crate::plan::step_add(&ctx, "demo", "Blocked", vec![], vec!["s-1".into()], None).unwrap();
        crate::plan::step_add(&ctx, "demo", "Free", vec![], vec![], None).unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        assert!(
            runner
                .calls
                .borrow()
                .iter()
                .any(|call| call.display().contains("Next: s-1 First"))
        );
        // Mark the first step running; a free third step remains available.
        let path = crate::plan::plan_path(&f.project);
        let mut plan = crate::plan::load(&f.project).unwrap().unwrap();
        plan.steps[0].state = crate::contracts::StepState::Running;
        std::fs::write(&path, toml::to_string(&plan).unwrap()).unwrap();
        let mut state = steps::State::default();
        let herdr = Herdr::new(
            f.env.herdr_bin(),
            &f.project.coordinator().unwrap().socket,
            &runner,
        );
        plan_nudge(&f.project, &herdr, std::slice::from_ref(&agent), &mut state).unwrap();
        assert!(
            runner
                .calls
                .borrow()
                .iter()
                .any(|call| call.display().contains("Next: s-3 Free"))
        );
        plan.steps[2].state = crate::contracts::StepState::Done;
        std::fs::write(&path, toml::to_string(&plan).unwrap()).unwrap();
        plan_nudge(&f.project, &herdr, &[agent], &mut steps::State::default()).unwrap();
        assert!(
            runner
                .calls
                .borrow()
                .iter()
                .any(|call| call.display().contains("s-2 waits for s-1"))
        );
    }

    #[test]
    fn a_new_lane_or_plan_revision_or_rolf_message_rearms_the_nudge() {
        let (f, runner, agent) = nudge_setup();
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        crate::plan::step_add(&ctx, "demo", "Next outcome", vec![], vec![], None).unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        let lane = thread::allocate(&f.project, |t| {
            t.role = "worker".into();
            t.status = thread::Status::Open;
        })
        .unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        assert_eq!(runner.count("agent prompt"), 1);
        thread::update(&f.project, &lane.id, |t| {
            t.status = thread::Status::Resolved
        })
        .unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        assert_eq!(runner.count("agent prompt"), 2);
        crate::plan::step_add(&ctx, "demo", "Another outcome", vec![], vec![], None).unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        assert_eq!(runner.count("agent prompt"), 3);
        crate::prompt::record_test_request(&f.project, "q-100", "Keep going").unwrap();
        nudge_pass(&f, &runner, &[agent]);
        assert_eq!(runner.count("agent prompt"), 4);
    }

    #[test]
    fn an_open_ask_allows_only_independent_steps_and_answer_rearms_normal_nudge() {
        let (f, runner, agent) = nudge_setup();
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        crate::prompt::record_test_request(&f.project, "q-100", "Go").unwrap();
        let task = crate::task::add(
            &f.project,
            "Call for Rolf",
            vec!["request:q-100".into()],
            vec!["Decide".into()],
            None,
            None,
        )
        .unwrap();
        crate::plan::step_add(
            &ctx,
            "demo",
            "Ask-tied",
            vec![task.id.clone()],
            vec![],
            None,
        )
        .unwrap();
        crate::plan::step_add(&ctx, "demo", "Dependent", vec![], vec!["s-1".into()], None).unwrap();
        crate::plan::step_add(&ctx, "demo", "Independent", vec![], vec![], None).unwrap();
        let ask = crate::ask::ask(
            &ctx,
            "demo",
            crate::ask::NewAsk {
                question: "Which?".into(),
                choices: vec!["One".into(), "Two".into()],
                what: None,
                means: None,
                task: Some(task.id),
            },
        )
        .unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        assert_eq!(runner.count("agent prompt"), 1);
        let prompts = runner.calls.borrow();
        let line = prompts
            .iter()
            .find(|call| call.display().contains("independent step"))
            .unwrap()
            .display();
        assert!(line.contains("independent step s-3 is ready"));
        assert!(line.contains("Start only work that does not require that answer"));
        assert!(!line.contains("s-1"));
        assert!(!line.contains("s-2"));
        drop(prompts);
        crate::ask::answer(&ctx, "demo", &ask.id, ask.revision, 1, "Rolf").unwrap();
        nudge_pass(&f, &runner, &[agent]);
        assert_eq!(runner.count("agent prompt"), 2);
        assert!(
            runner
                .calls
                .borrow()
                .iter()
                .any(|call| call.display().contains("Next: s-1 Ask-tied"))
        );
    }

    #[test]
    fn plan_nudge_skips_ask_review_running_done_unbound_and_busy() {
        let (f, runner, mut agent) = nudge_setup();
        let ctx = Ctx {
            env: &f.env,
            root: f.root.clone(),
            config_dir: f.root.join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        // A plan with no unfinished steps cannot prompt.
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
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
        let ask = crate::ask::ask(
            &ctx,
            "demo",
            crate::ask::NewAsk {
                question: "Which?".into(),
                choices: vec!["One".into(), "Two".into()],
                what: None,
                means: None,
                task: None,
            },
        )
        .unwrap();
        nudge_pass(&f, &runner, std::slice::from_ref(&agent));
        assert_eq!(runner.count("agent prompt"), 0);
        crate::ask::withdraw(&ctx, "demo", &ask.id, "No longer needed", "Rolf").unwrap();
        let review = crate::review::Review {
            id: "review-1".into(),
            repo: String::new(),
            integration: String::new(),
            base: String::new(),
            candidate_branch: String::new(),
            members: vec![],
            gates: vec![],
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
            push: false,
            install: false,
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
        assert_eq!(runner.count("agent prompt"), 0);
        std::fs::remove_file(crate::review::path(&f.project, "review-1")).unwrap();
        // An unbound project does not start a coordinator or get a prompt.
        let unbound = project::create(&f.root, "unbound", "", vec![]).unwrap();
        crate::plan::step_add(&ctx, "unbound", "Outcome", vec![], vec![], None).unwrap();
        let herdr = Herdr::new(
            f.env.herdr_bin(),
            &f.project.coordinator().unwrap().socket,
            &runner,
        );
        let mut state = steps::State::default();
        plan_nudge(&unbound, &herdr, &[agent.clone()], &mut state).unwrap();
        f.project
            .update_coordinator(|c| c.closed_by_rolf_at = project::now())
            .unwrap();
        nudge_pass(&f, &runner, &[agent]);
        assert_eq!(runner.count("agent prompt"), 0);
        assert_eq!(runner.count("agent start"), 0);
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
        runner.on("agent list", ok(&with_cwd(r#"{"result":{"agents":[{"pane_id":"w1:p9","tab_id":"w1:t9","workspace_id":"w1","name":"hp-demo-coordinator","agent":"claude","agent_status":"idle"},{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","name":"","cwd":"CWD","agent":"claude","agent_status":"idle"}]}}"#, &f)));
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
            ok(r#"{"result":{"process_info":{"foreground_processes":[]}}}"#),
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
