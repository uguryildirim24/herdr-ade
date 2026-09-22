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

use crate::coordinator::{self, MAX_LAUNCH_ATTEMPTS};
use crate::herdr::{Agent, Herdr, Pane};
use crate::paths::Ctx;
use crate::project::{self, Project, Status};
use crate::steps::{self, Memory};
use crate::{inbox, thread, threads};

const TICK: Duration = crate::pi::doctor::READINESS_CACHE_TTL;
const STOP_WAIT: Duration = Duration::from_secs(60);
const HANDOFF_READY_WAIT: Duration = Duration::from_secs(10);
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

fn handoff_path(root: &Path) -> PathBuf {
    root.join(".ticker.handoff")
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

/// The `ticker start` decision. A healthy ticker of the same version is never
/// replaced; a different version, or a stop in progress, is stopped first so
/// `open` never ends with no ticker.
fn decide_start(lock: &LockState, my_version: &str, stop_file_exists: bool) -> StartAction {
    match lock {
        LockState::Free => StartAction::Spawn,
        LockState::Held(info) if info.version == my_version && !stop_file_exists => {
            StartAction::Nothing
        }
        LockState::Held(_) => StartAction::StopThenSpawn,
    }
}

/// Ensures a ticker is running without waiting for a running one to stop.
/// `round advance` must not block while replacing a ticker: the ticker's own
/// pass calls `advance`, so waiting here would deadlock against the ticker
/// waiting on `advance`'s lock. Ordinary thread starts and explicit `ticker
/// start` calls still replace a stale-version ticker.
pub(crate) fn ensure(ctx: &Ctx) -> Result<()> {
    let root = &ctx.root;
    if !ctx.detached_ticker || project::list_slugs(root).is_empty() {
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
    let root = &ctx.root;
    if !ctx.detached_ticker || project::list_slugs(root).is_empty() {
        return Ok(());
    }
    let stop_exists = stop_path(root).exists();
    match decide_start(&lock_state(root), crate::VERSION, stop_exists) {
        StartAction::Nothing => Ok(()),
        StartAction::Spawn => {
            // A leftover stop file would make the new ticker exit at once.
            let _ = std::fs::remove_file(stop_path(root));
            spawn(root)
        }
        StartAction::StopThenSpawn => replace(root),
    }
}

unsafe extern "C" {
    fn setsid() -> i32;
}

/// `ticker run`, detached: null stdio and a new session, so it does not die
/// with the process group of whatever started it (an agent's shell tool).
fn spawn_command(binary: &Path, root: &Path, handoff: bool) -> Command {
    let mut command = Command::new(binary);
    command.arg("--root").arg(root).args(["ticker", "run"]);
    if handoff {
        command.arg("--handoff");
    }
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

fn detached_command(root: &Path, handoff: bool) -> Result<Command> {
    use std::os::unix::process::CommandExt;
    let binary = std::env::current_exe().context("could not find this binary's own path")?;
    let mut command = spawn_command(&binary, root, handoff);
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
    detached_command(root, false)?
        .spawn()
        .context("could not start the ticker")?;
    Ok(())
}

/// Start the replacement far enough to prove that it can initialize before
/// asking the current ticker to leave. The initialized child waits for the
/// parent's release marker, so there is always one viable ticker throughout
/// the handoff.
fn replace(root: &Path) -> Result<()> {
    let ready = handoff_path(root);
    let _ = std::fs::remove_file(&ready);
    let mut child = detached_command(root, true)?
        .spawn()
        .context("could not start the replacement ticker")?;
    let deadline = Instant::now() + HANDOFF_READY_WAIT;
    loop {
        if let Some(status) = child
            .try_wait()
            .context("could not inspect the replacement ticker")?
        {
            let _ = std::fs::remove_file(&ready);
            bail!("replacement ticker exited before handoff readiness ({status})");
        }
        if std::fs::read(&ready).is_ok_and(|value| value == b"ready") {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("replacement ticker did not become ready before the handoff");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    if let Err(error) = stop(root) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&ready);
        return Err(error);
    }
    if let Err(error) = std::fs::write(&ready, b"go") {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&ready);
        return Err(error).context("could not release the replacement ticker");
    }
    let deadline = Instant::now() + HANDOFF_READY_WAIT;
    loop {
        if matches!(lock_state(root), LockState::Held(ref info) if info.version == crate::VERSION) {
            return Ok(());
        }
        if let Some(status) = child
            .try_wait()
            .context("could not inspect the replacement ticker")?
        {
            let _ = std::fs::remove_file(&ready);
            bail!("replacement ticker exited during handoff ({status})");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&ready);
            bail!("replacement ticker did not take the ticker lock");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Asks the running ticker to exit and waits for the lock to be released.
pub(crate) fn stop(root: &Path) -> Result<()> {
    if lock_state(root) == LockState::Free {
        let _ = std::fs::remove_file(stop_path(root));
        return Ok(());
    }
    std::fs::write(stop_path(root), b"")?;
    let deadline = Instant::now() + STOP_WAIT;
    while Instant::now() < deadline {
        if lock_state(root) == LockState::Free {
            let _ = std::fs::remove_file(stop_path(root));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = std::fs::remove_file(stop_path(root));
    bail!(
        "the ticker did not exit within {} seconds",
        STOP_WAIT.as_secs()
    )
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
            if info.version != crate::VERSION {
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
pub(crate) fn run(ctx: &Ctx, handoff: bool) -> Result<()> {
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
    if handoff {
        let marker = handoff_path(root);
        std::fs::write(&marker, b"ready")?;
        let deadline = Instant::now() + STOP_WAIT + HANDOFF_READY_WAIT;
        loop {
            // The parent writes `go` only after the old ticker has released
            // its lock. Until then this fully initialized child stays viable.
            let released = std::fs::read(&marker).is_ok_and(|value| value == b"go");
            if released && lock.try_lock().is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = std::fs::remove_file(&marker);
                bail!("ticker handoff timed out waiting for the previous ticker");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = std::fs::remove_file(&marker);
    } else if lock.try_lock().is_err() {
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
    lock.set_len(0)?;
    lock.write_all(serde_json::to_string_pretty(&info)?.as_bytes())?;
    lock.flush()?;

    let log = Log {
        path: log_path(root),
    };
    log.line(&format!(
        "ticker {} started (pid {})",
        info.version, info.pid
    ));
    let mut last_reachable = Instant::now();
    let mut memory = Memory::new(ctx);
    loop {
        if stop_path(root).exists() {
            log.line("stop file found; exiting");
            return Ok(());
        }
        if tick(ctx, &log, &mut memory) {
            last_reachable = Instant::now();
        } else if last_reachable.elapsed() > IDLE_EXIT {
            log.line("no project has had a reachable session for five minutes; exiting");
            return Ok(());
        }
        // Sleep in short slices so a stop request is honoured promptly.
        let wake = Instant::now() + TICK;
        while Instant::now() < wake {
            if stop_path(root).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// One pass over every active project. Cheap work (state, prompts, tokens)
/// comes first for every project, then slow work (copies, launches), so one
/// slow project does not delay the others' sidebar. Returns whether any
/// project's session was reachable. A failure in one project never stops the
/// others.
pub(crate) fn tick(ctx: &Ctx, log: &Log, memory: &mut Memory) -> bool {
    memory.tick += 1;
    memory.machine_views.clear();
    let mut reachable = Vec::new();
    for slug in project::list_slugs(&ctx.root) {
        let Ok(project) = Project::load(&ctx.root, &slug) else {
            continue;
        };
        if project.status() != Status::Active {
            continue;
        }
        match tick_cheap(ctx, &project) {
            Ok(Some(seen)) => reachable.push((project, seen)),
            Ok(None) => {}
            Err(error) => log.line(&format!("{slug}: {error:#}")),
        }
    }
    // One courier pass per due machine, covering every project with lanes on
    // it (SPEC-remote §4.3). This runs before the per-project slow pass so the
    // first project cannot starve the cadence of the others.
    let project_refs: Vec<&Project> = reachable.iter().map(|(project, _)| project).collect();
    for error in machine_passes(ctx, &project_refs, memory, log) {
        log.line(&format!("{error:#}"));
    }
    for (project, seen) in &reachable {
        for error in tick_slow(ctx, project, seen, memory) {
            log.line(&format!("{}: {error:#}", project.slug));
        }
    }
    !reachable.is_empty()
}

/// One courier pass per saved machine that has lanes, once per fourth tick.
/// The cadence lives per machine in `Memory`, not per project, and every
/// project with lanes on that machine shares the one SSH trip.
fn machine_passes(
    ctx: &Ctx,
    projects: &[&Project],
    memory: &mut Memory,
    log: &Log,
) -> Vec<anyhow::Error> {
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
        if !memory.machine_is_due(&machine) {
            continue;
        }
        let projects: Vec<&Project> = entries.iter().map(|(project, _)| project).collect();
        let outcome = steps::courier(ctx, &projects, &machine);
        let reason = outcome.as_ref().err().map(|e| format!("{e:#}"));
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
        } else if matches!(&event, Some(steps::OutageEvent::Recovered)) {
            for (project, threads) in &entries {
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
        let Some((first, _)) = entries.first() else {
            continue;
        };
        if let Err(error) = steps::write_machine_outage(first, &machine, event, memory) {
            errors.push(error.context("machine outage"));
        }
        memory
            .machine_views
            .insert(machine, outcome.map_err(|e| format!("{e:#}")));
    }
    errors
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
    match tick_cheap(ctx, project)? {
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

fn thread_pass(
    input: &LaunchPass<'_>,
    prefix: &str,
    hashes: Option<&std::collections::BTreeMap<String, String>>,
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
        if t.status == thread::Status::Starting {
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

        let mut delivered = false;
        if t.prompt_pending
            && live
                .agent_state
                .as_deref()
                .is_some_and(crate::herdr::ready_state)
        {
            match herdr.agent_prompt(&t.pane_id, &thread::launch_prompt(prefix, slug, t)) {
                Ok(()) => delivered = true,
                Err(error) => {
                    pass.error = pass
                        .error
                        .or(Some(anyhow::anyhow!("{}: brief prompt: {error}", t.id)))
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
        let report_hash = fresh_hash.unwrap_or_else(|| t.report_hash.clone());
        let after = thread::Thread {
            prompt_pending: t.prompt_pending && !delivered,
            report_hash,
            ..t.clone()
        };
        // In the tick that delivers a prompt the agent still reads as idle; it
        // has just been given work, so it is Working, not Idle.
        let group = if delivered {
            thread::Group::Working
        } else {
            thread::group(&after, &live, now)
        };
        let process_gone = !t.is_remote()
            && after.report_hash.is_empty()
            && (!live.pane_exists
                || (live.agent_state.is_none() && !t.prompt_pending && !t.last_state.is_empty()));
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
        if delivered || state != t.last_state || group.token() != t.last_group {
            thread::update(project, &t.id, |t| {
                if delivered {
                    t.prompt_pending = false;
                }
                if state != t.last_state {
                    t.last_state = state.clone();
                    t.last_state_change = project::now();
                }
                if t.failure_class == crate::contracts::FailureClass::ProcessGone {
                    t.failure_class = crate::contracts::FailureClass::Unknown;
                    t.error.clear();
                }
                t.last_group = group.token().to_string();
            })?;
        }
        if live.pane_exists {
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

/// Launches pending threads whose pane is at a shell prompt. Local starts stay
/// one per pass; independent box starts are submitted as one parallel batch.
/// Prompts still go only to agents listed before this launch pass.
fn launch_pass(
    pass: &LaunchPass<'_>,
    may_start: &mut bool,
    one_at_a_time: bool,
    errors: &mut Vec<anyhow::Error>,
) {
    let now = jiff::Timestamp::now();
    let mut pending = Vec::new();
    for t in pass.threads {
        if t.status != thread::Status::Open || !t.prompt_pending {
            continue;
        }
        let live = thread::live_state(t, pass.agents, pass.panes, now);
        if live.agent_state.is_some() || !live.pane_exists {
            continue;
        }
        if t.launch_attempts >= thread::MAX_LAUNCH_ATTEMPTS {
            let reason = format!(
                "no `{}` agent appeared in the pane after {} launch attempts",
                t.agent,
                thread::MAX_LAUNCH_ATTEMPTS
            );
            errors.extend(
                threads::fail_start(
                    pass.ctx,
                    pass.project,
                    &t.id,
                    &reason,
                    crate::contracts::FailureClass::ProcessGone,
                    false,
                )
                .err()
                .map(|error| error.context(format!("{}: failed-start cleanup", t.id))),
            );
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
        match thread::update(pass.project, &t.id, |t| t.launch_attempts += 1) {
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
        return;
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
            parent: parent.as_deref(),
            ready_timeout_ms: agent_start_timeout(&t.launch),
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
                thread::bind_identity(record, &socket, &agent, process);
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
        errors.extend(
            launched
                .err()
                .map(|error| error.context(format!("{}: launch", t.id))),
        );
    }
}

fn open_threads(project: &Project, remote: bool) -> Vec<thread::Thread> {
    thread::list(project)
        .into_iter()
        .filter(|t| {
            t.is_remote() == remote
                && matches!(t.status, thread::Status::Open | thread::Status::Starting)
        })
        .collect()
}

/// Returns `Ok(None)` when the project's session cannot be reached: then no
/// state is read, so nothing is ever reported as gone.
fn tick_cheap(ctx: &Ctx, project: &Project) -> Result<Option<Seen>> {
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
        // One priming line per binding. Transport is not the receipt: an ADE
        // binding clears `prime_pending` only on its `ha context` receipt, and
        // a submitted line is never re-sent on a timer (SPEC-ADE D14).
        if record.prime_pending && !record.prime_sent && agent.ready() {
            let prompt = coordinator::priming_prompt(&prefix, slug);
            crate::talk::mark_automated_prompt(project, &record.pane_id, &prompt)?;
            match herdr.agent_prompt(&record.pane_id, &prompt) {
                Ok(()) => {
                    project.update_coordinator(|c| c.prime_sent = true)?;
                }
                Err(error) => first_error = Some(anyhow::anyhow!("priming prompt: {error}")),
            }
        }
        coordinator::report_tokens(&herdr, slug, &record.pane_id);
    }

    if let Err(error) = crate::threads::retry_pending_cleanup(ctx, project) {
        first_error = first_error.or(Some(error.context("pending cancellation cleanup")));
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
    )?;
    first_error = first_error.or(pass.error);
    if let Err(error) = crate::threads::tick(project, &herdr, &agents) {
        first_error = first_error.or(Some(error));
    }
    // The ops pass (A2) and the rounds pass (A3) run in the slow pass,
    // outside the project lock (SPEC-ADE item 57).
    let coordinator_recorded = usize::from(!record.pane_id.is_empty());
    let coordinator_missing = usize::from(
        coordinator_recorded == 1
            && agent.is_none()
            && !panes.iter().any(|p| coordinator::pane_matches(&record, p)),
    );
    let recorded_panes = pass.recorded_panes + coordinator_recorded;
    let missing_panes = pass.missing_panes + coordinator_missing;

    // Nudge (or notify) about inbox items `context` has not shown yet.
    if let Ok((settings, _)) = project.read_project_md() {
        let mut state = steps::load_state(project);
        let before = state.clone();
        let ready_pane = agent
            .as_ref()
            .filter(|a| a.ready())
            .map(|_| record.pane_id.as_str());
        if let Err(error) = steps::nudge(project, &mut state, &settings, &herdr, ready_pane) {
            first_error = first_error.or(Some(error.context("nudge")));
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
    let threads = pass.threads;
    let remote = herdr.on_machine(machine);
    let (agents, panes) = match (&view.agents, &view.panes) {
        (Some(agents), Some(panes)) => (agents.clone(), panes.clone()),
        // The box server did not answer: the sealed events were still imported,
        // but no lane state changes and no GONE is invented (SPEC-remote §4.3).
        _ => return Ok(()),
    };

    let prefix = coordinator::current_prefix(&ctx.root).map_err(|e| format!("{e:#}"))?;
    let state_input = LaunchPass {
        ctx,
        project,
        herdr: &remote,
        threads,
        agents: &agents,
        panes: &panes,
    };
    let state_pass = thread_pass(&state_input, &prefix, None).map_err(|e| format!("{e:#}"))?;
    errors.extend(state_pass.error);
    launch_pass(
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
    // The D8 BLOCKED/GONE lines for this machine's box lanes (SPEC-remote §4.3).
    errors.extend(steps::remote_attention(
        ctx,
        project,
        steps::RemoteView {
            machine_id: &view.machine_id,
            threads,
            agents: &agents,
            panes: &panes,
            boot_id: &view.boot_id,
            now: jiff::Timestamp::now(),
        },
    ));
    Ok(())
}

/// Copies and launches, remote machines, then inbox items, pull requests,
/// routines, auto-resolve and housekeeping.
fn tick_slow(ctx: &Ctx, project: &Project, seen: &Seen, memory: &mut Memory) -> Vec<anyhow::Error> {
    let _scope = crate::ledger::Scope::new(&[project]);
    let mut errors = Vec::new();
    errors.extend(crate::escalation::tick(ctx, project).err());
    let herdr = Herdr::new(ctx.env.herdr_bin(), &seen.socket, ctx.runner);
    let now = jiff::Timestamp::now();
    let mut may_start = true;

    errors.extend(
        crate::steps::config_changed(project, &crate::project::policy_hash(&ctx.config_dir))
            .err()
            .map(|e| e.context("config digest")),
    );

    if let Some(record) = project.coordinator().filter(|c| c.prime_pending) {
        let pane_alive = seen
            .panes
            .iter()
            .any(|p| coordinator::pane_matches(&record, p));
        let pane_has_agent = seen.agents.iter().any(|a| a.pane_id == record.pane_id);
        if pane_alive && !pane_has_agent && record.launch_attempts < MAX_LAUNCH_ATTEMPTS {
            may_start = false;
            let started = (|| -> Result<()> {
                project.update_coordinator(|c| {
                    c.launch_attempts += 1;
                    c.generation += 1;
                    c.prime_sent = false;
                })?;
                // The recipe stored at `open`, never rebuilt from settings
                // that may have changed since (SPEC-ADE D2).
                let launch = &record.launch;
                herdr.agent_start_opts(&crate::herdr::AgentStart {
                    name: &record.agent_name,
                    kind: &launch.kind,
                    pane: &record.pane_id,
                    agent_args: &launch.args,
                    parent: None,
                    ready_timeout_ms: launch.ready_timeout_ms,
                })?;
                Ok(())
            })();
            errors.extend(started.err());
        }
    }

    // Local threads: copy home when the report changed, then launches.
    let local = open_threads(project, false);
    for t in local.iter().filter(|t| t.status == thread::Status::Open) {
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
    let remote_threads = open_threads(project, true);
    let mut machines: Vec<String> = remote_threads
        .iter()
        .map(|t| t.machine_route().to_string())
        .collect();
    machines.sort();
    machines.dedup();
    for machine in machines {
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

    errors.extend(steps::session_notice(project, &mut state, seen.session_lost).err());
    errors.extend(steps::pull_requests(ctx, project, &mut state, memory, now));
    let zoned = jiff::Zoned::now();
    match project.read_project_md() {
        Ok((settings, _)) => {
            let commands = project
                .safety(&ctx.config_dir)
                .map(|s| s.routine_commands)
                .unwrap_or(false);
            errors.extend(steps::routines(
                ctx, project, &mut state, commands, None, &zoned,
            ));
            errors.extend(steps::auto_resolve(ctx, project, &settings, memory, now));
        }
        Err(error) => {
            let text = std::fs::read(project.project_md()).unwrap_or_default();
            let problem = Some((thread::sha256_hex(&text), format!("{error:#}")));
            errors.extend(steps::routines(
                ctx, project, &mut state, false, problem, &zoned,
            ));
        }
    }
    // D5 recovery and delivery (X1 to X5), then rounds, asks, talk and the
    // board (D6, D17, D18). Each takes the project lock only for its own
    // file writes; git and herdr run outside it.
    errors.extend(
        crate::ops::tick(ctx, project)
            .err()
            .map(|e| e.context("ops")),
    );
    errors.extend(
        crate::round::tick(ctx, project)
            .err()
            .map(|e| e.context("rounds")),
    );
    inbox::prune_done(project, steps::DONE_RETENTION_DAYS);
    if state != before {
        errors.extend(steps::save_state(project, &state).err());
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Env;
    use crate::runner::fake::{FakeRunner, fail, ok};

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
        let runner = FakeRunner::new();
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
                record.machine = "oci".into();
                record.machine_id = "machine-1".into();
                record.workspace_id = "w2".into();
                record.tab_id = tab_id.clone();
                record.pane_id = pane_id.clone();
                record.cwd = cwd.clone();
                record.agent = "claude".into();
                record.agent_name = format!("hp-demo-{id}");
                record.launch.kind = "claude".into();
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
    }

    #[test]
    fn a_detached_ticker_uses_the_projects_root_not_the_callers_folder() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("projects-root");
        std::fs::create_dir(&root).unwrap();
        let command = spawn_command(Path::new("/bin/true"), &root, false);
        assert_eq!(command.get_current_dir(), Some(root.as_path()));
    }

    #[test]
    fn start_decisions() {
        assert_eq!(
            decide_start(&LockState::Free, "v1", false),
            StartAction::Spawn
        );
        assert_eq!(
            decide_start(&LockState::Free, "v1", true),
            StartAction::Spawn
        );
        assert_eq!(decide_start(&held("v1"), "v1", false), StartAction::Nothing);
        assert_eq!(
            decide_start(&held("v0"), "v1", false),
            StartAction::StopThenSpawn
        );
        // A stop in progress: finish it, then spawn.
        assert_eq!(
            decide_start(&held("v1"), "v1", true),
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
        run(&ctx, false).unwrap();
        assert!(!missing.exists());

        std::fs::create_dir(&missing).unwrap();
        start(&ctx).unwrap();
        run(&ctx, false).unwrap();
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
    fn shell_prompt_pane_gets_at_most_three_launch_attempts() {
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
        assert_eq!(runner.count("agent start"), 3);
        assert_eq!(runner.count("agent prompt"), 0);
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
