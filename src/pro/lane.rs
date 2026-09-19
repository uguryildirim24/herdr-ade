//! Lane start, resume, reconcile and the stop switch (SPEC-pro-bridge v2,
//! "Lane lifecycle and turn protocol").
//!
//! A lane is a `codex` agent in its own tab whose own Codex process is pointed
//! at the bridge with `-c` overrides. Rolf's `~/.codex` is never written.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::herdr_cli::{self, Agent};
use super::sh::Runner;
use super::state::{self, Lane};
use super::{Env, Layout, MODEL, bridge, doctor, home};

/// `agent start` waits this long for a ready agent; a trust or sign-in prompt
/// shows as a blocked screen and the start times out.
const READY_TIMEOUT_MS: u64 = 120_000;

/// After the agent is ready, wait this long for Codex to write its session
/// rollout. A trust prompt never creates one, so a missing rollout means the
/// lane is not usable. The wait ends on an event; this is only the outer
/// bound so nothing hangs forever. A lane recipe's `ready_timeout_ms`
/// replaces it.
const ROLLOUT_TIMEOUT: Duration = Duration::from_secs(180);

/// The rollout poll interval.
const ROLLOUT_POLL: Duration = Duration::from_millis(250);

/// The exact trust prompt Codex shows for an untrusted directory (2026-09-19
/// live run, Codex 0.155.1). It does not always read as `blocked`, so the
/// screen is checked too.
pub const TRUST_PROMPT: &str = "Do you trust the contents of this directory?";

#[derive(Debug, Clone)]
pub struct StartOptions {
    pub name: String,
    pub cwd: Option<String>,
    /// A Codex config profile to launch instead of the Pro bridge route. A
    /// picture lane is `Some("gpt-image-gen")` and never sees the bridge.
    pub profile: Option<String>,
    /// Reference pictures Codex attaches at start (`--image`), used by the
    /// picture lane.
    pub images: Vec<PathBuf>,
    /// The lane recipe's `ready_timeout_ms`, when the caller has one. `None`
    /// uses [`ROLLOUT_TIMEOUT`].
    pub ready_timeout_ms: Option<u64>,
}

/// The four `-c` overrides every Pro Codex process carries (spec Design,
/// "Codex home (v1)"). They change nothing on disk.
pub fn codex_args(port: u16) -> Vec<String> {
    vec![
        "-c".into(),
        format!("model={MODEL}"),
        "-c".into(),
        format!("model_reasoning_effort={}", super::EFFORT),
        "-c".into(),
        format!("openai_base_url=http://127.0.0.1:{port}/v1"),
        "-c".into(),
        format!("tool_output_token_limit={}", super::TOOL_OUTPUT_TOKEN_LIMIT),
    ]
}

/// The `--profile` overrides for a lane that runs on Codex's own backend
/// (Codex 0.155 reads `<name>.config.toml` from the home).
pub fn profile_args(profile: &str) -> Vec<String> {
    vec!["--profile".into(), profile.into()]
}

/// The pictures a picture lane attaches at start: Codex's `--image <file>`
/// (repeatable; `codex --help`, 0.155.1).
pub fn image_args(files: &[PathBuf]) -> Vec<String> {
    let mut args = Vec::new();
    for file in files {
        args.push("--image".into());
        args.push(file.display().to_string());
    }
    args
}

/// The project slug from `HERDR_ADE_LAUNCH` (`<project>/<thread>/<n>/<hash>`).
fn launch_project(env: &Env) -> Option<String> {
    let launch = env.var("HERDR_ADE_LAUNCH")?;
    let slug = launch.split('/').next()?;
    (!slug.is_empty()).then(|| slug.to_string())
}

/// The project coordinator's pane from `coordinator.json`, for a caller that
/// is not itself a herdr pane. `ha` records it per project.
fn coordinator_pane(env: &Env) -> Option<String> {
    #[derive(Deserialize)]
    struct Record {
        #[serde(default)]
        pane_id: String,
    }
    let slug = launch_project(env)?;
    let path = super::ade_root(env)
        .ok()?
        .join(slug)
        .join(".state/coordinator.json");
    let text = std::fs::read_to_string(path).ok()?;
    let record: Record = serde_json::from_str(&text).ok()?;
    (!record.pane_id.is_empty()).then_some(record.pane_id)
}

/// The pane a lane started by this call sits under: the caller's own herdr
/// pane, else the project's coordinator pane, else none.
pub fn parent_pane(env: &Env) -> Option<String> {
    env.var("HERDR_PANE_ID")
        .map(str::to_string)
        .or_else(|| coordinator_pane(env))
}

/// The resume line for a profile lane: `codex resume --profile <name>`.
pub fn profile_resume_args(profile: &str, session_id: &str) -> Vec<String> {
    vec![
        "resume".into(),
        "--profile".into(),
        profile.into(),
        session_id.into(),
    ]
}

/// The start line, with `resume <id>` first for a resume (spec, "Resume after
/// a cold restart").
pub fn codex_resume_args(port: u16, session_id: &str) -> Vec<String> {
    let mut args = vec!["resume".to_string()];
    args.extend(codex_args(port));
    args.push(session_id.to_string());
    args
}

/// The workspace to create the lane tab in: the coordinator's, else the
/// parent pane's.
fn workspace_for(
    env: &Env,
    runner: &dyn Runner,
    bin: &str,
    parent: Option<&str>,
) -> Option<String> {
    if let Some(workspace) = env.var("HERDR_WORKSPACE_ID") {
        return Some(workspace.to_string());
    }
    let parent = parent?;
    herdr_cli::pane_get(runner, bin, parent)
        .ok()
        .map(|pane| pane.workspace_id)
        .filter(|w| !w.is_empty())
}

/// Is exactly `cwd` trusted in the lane Codex home?
/// `[projects."<path>"] trust_level = "trusted"`. Codex trusts exact project
/// paths only: a trusted ancestor such as `/home/agent` does not cover a
/// subdirectory, so the check never walks up.
pub fn trusted(env: &Env, cwd: &Path) -> bool {
    let config = env.lane_codex_home().join("config.toml");
    let Ok(text) = std::fs::read_to_string(&config) else {
        return false;
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return false;
    };
    let Some(projects) = table.get("projects").and_then(toml::Value::as_table) else {
        return false;
    };
    projects
        .get(&cwd.display().to_string())
        .and_then(|value| value.get("trust_level"))
        .and_then(toml::Value::as_str)
        == Some("trusted")
}

/// The newest rollout for this lane. Prefers a session-id match; otherwise the
/// newest file whose `session_meta.cwd` equals the lane cwd and whose stamp is
/// after the lane started.
pub fn find_rollout(
    codex_home: &Path,
    cwd: &str,
    after: jiff::Timestamp,
    session_id: Option<&str>,
) -> Option<PathBuf> {
    let mut best: Option<(jiff::Timestamp, PathBuf)> = None;
    let mut stack = vec![codex_home.join("sessions")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                continue;
            }
            let Some(meta) = session_meta(&path) else {
                continue;
            };
            let matches = match session_id {
                Some(id) => meta.id == id,
                None => meta.cwd == cwd && meta.timestamp >= after,
            };
            if !matches {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(stamp, _)| meta.timestamp > *stamp)
            {
                best = Some((meta.timestamp, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

#[derive(Debug, Deserialize)]
struct SessionMeta {
    #[serde(default)]
    id: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug)]
struct Meta {
    id: String,
    cwd: String,
    timestamp: jiff::Timestamp,
}

fn session_meta(path: &Path) -> Option<Meta> {
    use std::io::BufRead;
    let file = File::open(path).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(file).read_line(&mut line).ok()?;
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let payload = value.get("payload")?;
    let meta: SessionMeta = serde_json::from_value(payload.clone()).ok()?;
    let timestamp = meta
        .timestamp
        .as_deref()
        .and_then(|text| text.parse::<jiff::Timestamp>().ok())
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    Some(Meta {
        id: meta.id,
        cwd: meta.cwd,
        timestamp,
    })
}

/// Point `lane.rollout` at the lane's newest rollout, or return `None` when
/// Codex has not written one yet. Also fills in `session_id` when it is empty.
pub fn refresh_rollout(env: &Env, lane: &mut Lane) -> Option<PathBuf> {
    let existing = lane
        .rollout
        .as_deref()
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    let path = existing.or_else(|| {
        let started = super::parse_rfc3339(&lane.started_at).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
        find_rollout(
            &env.lane_codex_home(),
            &lane.cwd,
            started,
            lane.session_id.as_deref(),
        )
    });
    if let Some(path) = &path {
        if lane.session_id.is_none()
            && let Some(meta) = session_meta(path)
            && !meta.id.is_empty()
        {
            lane.session_id = Some(meta.id);
        }
        lane.rollout = Some(path.display().to_string());
    }
    path
}

/// How the rollout wait ended. Every arm is an event, not a clock tick.
enum RolloutWait {
    /// Codex wrote the session rollout: the lane is usable.
    Ready(PathBuf),
    /// The pane shows Codex's trust prompt.
    TrustPrompt,
    /// Herdr reports the agent blocked; the reason is its last screen line.
    Blocked(String),
    /// Herdr no longer lists the lane's Codex agent.
    Gone,
    /// The outer bound passed with no rollout.
    TimedOut,
}

/// The monotonic clock the rollout wait reads, so tests run in fake time.
trait Clock {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// The outer bound for the rollout wait: the lane recipe's `ready_timeout_ms`
/// when it carries one, else the shipped constant. Zero means "use the
/// constant".
fn rollout_timeout(recipe_ms: Option<u64>) -> Duration {
    recipe_ms
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(ROLLOUT_TIMEOUT)
}

/// Poll for the rollout after the agent is ready. A trust prompt never writes
/// one, so this is the second half of "never report ready before the session
/// exists". The wait ends on the rollout, the trust prompt, a blocked agent or
/// a dead agent, not only when the bound passes.
fn wait_for_rollout(
    env: &Env,
    lane: &mut Lane,
    runner: &dyn Runner,
    bin: &str,
    timeout: Duration,
    clock: &dyn Clock,
) -> RolloutWait {
    let deadline = clock.now() + timeout;
    loop {
        if let Some(path) = refresh_rollout(env, lane) {
            return RolloutWait::Ready(path);
        }
        if trust_prompt_showing(runner, bin, &lane.pane_id) {
            return RolloutWait::TrustPrompt;
        }
        match herdr_cli::agent_find(runner, bin, &lane.name) {
            Ok(Some(agent)) if agent.blocked() => {
                return RolloutWait::Blocked(screen_reason(runner, bin, &lane.pane_id));
            }
            Ok(None) => return RolloutWait::Gone,
            _ => {}
        }
        // The pane no longer running Codex is death, the same test `reconcile`
        // uses. A failed call is not death; the bound still ends the wait.
        let alive = herdr_cli::process_info(runner, bin, &lane.pane_id)
            .map(|info| info.runs("codex"))
            .unwrap_or(true);
        if !alive {
            return RolloutWait::Gone;
        }
        if clock.now() >= deadline {
            return RolloutWait::TimedOut;
        }
        clock.sleep(ROLLOUT_POLL);
    }
}

/// The trust prompt currently on the pane's screen.
fn trust_prompt_showing(runner: &dyn Runner, bin: &str, pane: &str) -> bool {
    herdr_cli::pane_read(runner, bin, pane)
        .map(|text| text.contains(TRUST_PROMPT))
        .unwrap_or(false)
}

/// `herdr-pro start`: doctor, one serialized Codex start, then record.
pub fn start(env: &Env, layout: &Layout, runner: &dyn Runner, opts: &StartOptions) -> Result<Lane> {
    layout.ensure()?;
    state::check_name(&opts.name)?;
    // Serialize the whole decision, including the shared-home trust write and
    // the lane-name check. Two starts must not overwrite each other's project
    // entry or both claim one name.
    let _lock = state::FileLock::acquire(&layout.start_lock())?;
    if let Ok(existing) = Lane::read(layout, &opts.name)
        && existing.stopped
    {
        bail!("lane `{}` was stopped; use a new lane name", opts.name);
    }
    // A bridge lane gates on the bridge; a profile lane (the picture maker)
    // runs on Codex's own backend and gates on nothing here.
    let extra = match opts.profile.as_deref() {
        Some(profile) => {
            let mut args = profile_args(profile);
            args.extend(image_args(&opts.images));
            args
        }
        None => {
            if let Err(error) = doctor::gate(env, layout, runner) {
                return Err(anyhow::anyhow!("WAITING pro-bridge {error:#}"));
            }
            let (port, health) = bridge::health_any(runner)?;
            if !health.accepting {
                bail!("WAITING pro-bridge the bridge is draining");
            }
            if state::cooldown_active(layout, jiff::Timestamp::now()) {
                bail!("WAITING pro-bridge cooldown is active");
            }
            codex_args(port)
        }
    };
    if state::name_taken(layout, &opts.name) {
        bail!("lane `{}` already exists and is not gone", opts.name);
    }
    let cwd = match opts.cwd.as_deref().filter(|c| !c.is_empty()) {
        Some(cwd) => env.expand_tilde(cwd),
        None => std::env::current_dir().context("could not read the current directory")?,
    };
    let cwd = std::path::absolute(&cwd).with_context(|| format!("bad cwd {}", cwd.display()))?;
    let cwd_text = cwd.display().to_string();
    // The plugin owns the Pro home and trusts the exact lane cwd there, so
    // Codex never shows its trust prompt for a lane.
    home::trust(layout, &cwd)
        .with_context(|| format!("could not trust `{cwd_text}` in the Pro home"))?;
    if !trusted(env, &cwd) {
        bail!(
            "WAITING pro-bridge `{cwd_text}` is not trusted in {}; trust it once (or pass a trusted --cwd), never press through the prompt",
            env.lane_codex_home().join("config.toml").display()
        );
    }

    let bin = env.herdr_bin();
    let parent = parent_pane(env);
    let workspace = workspace_for(env, runner, &bin, parent.as_deref()).unwrap_or_default();
    let env_pairs: Vec<String> = env.codex_home_env().into_iter().collect();
    let pane = herdr_cli::tab_create(runner, &bin, &workspace, &cwd_text, &opts.name, &env_pairs)
        .context("could not create the lane tab")?;
    let started = jiff::Timestamp::now();
    let agent = match herdr_cli::agent_start(
        runner,
        &bin,
        &herdr_cli::StartSpec {
            name: &opts.name,
            kind: "codex",
            pane: &pane.pane_id,
            parent: parent.as_deref(),
            extra: &extra,
            ready_timeout_ms: READY_TIMEOUT_MS,
        },
    ) {
        Ok(agent) => agent,
        Err(error) => {
            let error = blocked_reason(runner, &bin, &pane.pane_id, error);
            let _ = herdr_cli::tab_close(runner, &bin, &pane.tab_id);
            return Err(error);
        }
    };
    if agent.blocked() {
        let reason = screen_reason(runner, &bin, &pane.pane_id);
        let _ = herdr_cli::tab_close(runner, &bin, &pane.tab_id);
        bail!(
            "WAITING pro-bridge the lane is blocked in pane {}: {reason}",
            pane.pane_id
        );
    }
    if trust_prompt_showing(runner, &bin, &pane.pane_id) {
        let _ = herdr_cli::tab_close(runner, &bin, &pane.tab_id);
        bail!(
            "WAITING pro-bridge `{cwd_text}` shows \"{TRUST_PROMPT}\" in pane {}; trust that exact directory in {} once, then start again (never press through the prompt)",
            pane.pane_id,
            env.lane_codex_home().join("config.toml").display()
        );
    }

    let session_id = agent
        .agent_session
        .as_ref()
        .map(|s| s.id.clone())
        .filter(|id| !id.is_empty());
    let mut lane = Lane {
        name: opts.name.clone(),
        pane_id: pane.pane_id.clone(),
        tab_id: pane.tab_id.clone(),
        workspace_id: pane.workspace_id.clone(),
        parent: parent.clone(),
        cwd: cwd_text,
        profile: opts.profile.clone(),
        session_id: session_id.clone(),
        rollout: None,
        started_at: started.to_string(),
        state: "ready".into(),
        stopped: false,
        last_turn: None,
        ready_timeout_ms: opts.ready_timeout_ms,
    };
    if opts.profile.is_none() {
        let timeout = rollout_timeout(opts.ready_timeout_ms);
        match wait_for_rollout(env, &mut lane, runner, &bin, timeout, &SystemClock) {
            RolloutWait::Ready(_) => {}
            RolloutWait::TrustPrompt => {
                let _ = herdr_cli::tab_close(runner, &bin, &pane.tab_id);
                bail!(
                    "WAITING pro-bridge `{}` shows \"{TRUST_PROMPT}\" in pane {}; trust that exact directory in {} once, then start again (never press through the prompt)",
                    lane.cwd,
                    pane.pane_id,
                    env.lane_codex_home().join("config.toml").display()
                );
            }
            RolloutWait::Blocked(reason) => {
                let _ = herdr_cli::tab_close(runner, &bin, &pane.tab_id);
                bail!(
                    "WAITING pro-bridge the lane is blocked in pane {}: {reason}",
                    pane.pane_id
                );
            }
            RolloutWait::Gone => {
                let _ = herdr_cli::tab_close(runner, &bin, &pane.tab_id);
                bail!(
                    "WAITING pro-bridge the lane's Codex agent died in pane {}",
                    pane.pane_id
                );
            }
            RolloutWait::TimedOut => {
                let reason = screen_reason(runner, &bin, &pane.pane_id);
                let _ = herdr_cli::tab_close(runner, &bin, &pane.tab_id);
                bail!(
                    "WAITING pro-bridge the lane has no Codex rollout after {}s in pane {}: {reason}",
                    timeout.as_secs(),
                    pane.pane_id
                );
            }
        }
    }
    if let Err(error) = lane.write(layout) {
        let _ = herdr_cli::tab_close(runner, &bin, &lane.tab_id);
        return Err(error);
    }
    Ok(lane)
}

fn blocked_reason(
    runner: &dyn Runner,
    bin: &str,
    pane: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    let screen = screen_reason(runner, bin, pane);
    anyhow::anyhow!("WAITING pro-bridge the lane did not become ready: {error:#}; screen: {screen}")
}

/// The last visible line of a stuck pane, for a WAITING line.
fn screen_reason(runner: &dyn Runner, bin: &str, pane: &str) -> String {
    herdr_cli::pane_read(runner, bin, pane)
        .ok()
        .and_then(|text| text.lines().last().map(str::trim).map(str::to_string))
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| "no readable screen".into())
}

/// `herdr-pro resume`: start the lane again and resume its Codex thread.
pub fn resume(env: &Env, layout: &Layout, runner: &dyn Runner, name: &str) -> Result<Lane> {
    // Re-read the lane only after owning the start lock. Otherwise two resume
    // processes can both observe `gone` before either writes `ready`.
    let _lock = state::FileLock::acquire(&layout.start_lock())?;
    let mut lane = Lane::read(layout, name)?;
    if lane.stopped {
        bail!("lane `{name}` was stopped and cannot be resumed");
    }
    if lane.state != "gone" {
        bail!("lane `{name}` is {}, not gone", lane.state);
    }
    let session = lane
        .session_id
        .clone()
        .filter(|id| !id.is_empty())
        .with_context(|| format!("lane `{name}` has no Codex session id to resume"))?;
    if let Some(other) = state::session_in_use(layout, &session, name) {
        bail!("Codex session {session} is already held by lane `{other}`");
    }
    let extra = match lane.profile.as_deref() {
        Some(profile) => profile_resume_args(profile, &session),
        None => {
            if state::cooldown_active(layout, jiff::Timestamp::now()) {
                bail!("WAITING pro-bridge cooldown is active");
            }
            if let Err(error) = doctor::gate(env, layout, runner) {
                bail!("WAITING pro-bridge {error:#}");
            }
            let (port, health) = bridge::health_any(runner)?;
            if !health.accepting {
                bail!("WAITING pro-bridge the bridge is draining");
            }
            codex_resume_args(port, &session)
        }
    };
    let cwd = PathBuf::from(&lane.cwd);
    home::trust(layout, &cwd)
        .with_context(|| format!("could not trust `{}` in the Pro home", lane.cwd))?;
    if !trusted(env, &cwd) {
        bail!("WAITING pro-bridge `{}` is not trusted", lane.cwd);
    }

    let bin = env.herdr_bin();
    let parent = parent_pane(env);
    let workspace = workspace_for(env, runner, &bin, parent.as_deref())
        .unwrap_or_else(|| lane.workspace_id.clone());
    let env_pairs: Vec<String> = env.codex_home_env().into_iter().collect();
    let pane = herdr_cli::tab_create(runner, &bin, &workspace, &lane.cwd, name, &env_pairs)
        .context("could not create the resume tab")?;
    let agent = match herdr_cli::agent_start(
        runner,
        &bin,
        &herdr_cli::StartSpec {
            name,
            kind: "codex",
            pane: &pane.pane_id,
            parent: parent.as_deref(),
            extra: &extra,
            ready_timeout_ms: READY_TIMEOUT_MS,
        },
    ) {
        Ok(agent) => agent,
        Err(error) => {
            let error = blocked_reason(runner, &bin, &pane.pane_id, error);
            let _ = herdr_cli::tab_close(runner, &bin, &pane.tab_id);
            return Err(error);
        }
    };
    if agent.blocked() {
        let reason = screen_reason(runner, &bin, &pane.pane_id);
        let _ = herdr_cli::tab_close(runner, &bin, &pane.tab_id);
        bail!(
            "WAITING pro-bridge the resumed lane is blocked in pane {}: {reason}",
            pane.pane_id
        );
    }
    if trust_prompt_showing(runner, &bin, &pane.pane_id) {
        let _ = herdr_cli::tab_close(runner, &bin, &pane.tab_id);
        bail!(
            "WAITING pro-bridge `{}` shows \"{TRUST_PROMPT}\" in pane {}; trust that exact directory once, then resume",
            lane.cwd,
            pane.pane_id
        );
    }
    lane.pane_id = pane.pane_id;
    lane.tab_id = pane.tab_id;
    lane.workspace_id = pane.workspace_id;
    lane.parent = parent;
    lane.state = "ready".into();
    lane.stopped = false;
    if lane.profile.is_none() {
        let timeout = rollout_timeout(lane.ready_timeout_ms);
        match wait_for_rollout(env, &mut lane, runner, &bin, timeout, &SystemClock) {
            RolloutWait::Ready(_) => {}
            RolloutWait::TrustPrompt => {
                let _ = herdr_cli::tab_close(runner, &bin, &lane.tab_id);
                bail!(
                    "WAITING pro-bridge `{}` shows \"{TRUST_PROMPT}\" in pane {}; trust that exact directory once, then resume",
                    lane.cwd,
                    lane.pane_id
                );
            }
            RolloutWait::Blocked(reason) => {
                let _ = herdr_cli::tab_close(runner, &bin, &lane.tab_id);
                bail!(
                    "WAITING pro-bridge the resumed lane is blocked in pane {}: {reason}",
                    lane.pane_id
                );
            }
            RolloutWait::Gone => {
                let _ = herdr_cli::tab_close(runner, &bin, &lane.tab_id);
                bail!(
                    "WAITING pro-bridge the resumed lane's Codex agent died in pane {}",
                    lane.pane_id
                );
            }
            RolloutWait::TimedOut => {
                let reason = screen_reason(runner, &bin, &lane.pane_id);
                let _ = herdr_cli::tab_close(runner, &bin, &lane.tab_id);
                bail!(
                    "WAITING pro-bridge the resumed lane has no Codex rollout after {}s in pane {}: {reason}",
                    timeout.as_secs(),
                    lane.pane_id
                );
            }
        }
    }
    if let Err(error) = lane.write(layout) {
        let _ = herdr_cli::tab_close(runner, &bin, &lane.tab_id);
        return Err(error);
    }
    Ok(lane)
}

/// `herdr-pro reconcile`: mark lanes whose pane no longer runs Codex as gone
/// and return one resume line each. It never starts anything.
pub fn reconcile(env: &Env, layout: &Layout, runner: &dyn Runner) -> Result<Vec<String>> {
    let bin = env.herdr_bin();
    let mut lines = Vec::new();
    for mut lane in Lane::list(layout)? {
        if lane.stopped || lane.state == "gone" {
            continue;
        }
        let alive = herdr_cli::process_info(runner, &bin, &lane.pane_id)
            .map(|info| info.runs("codex"))
            .unwrap_or(false);
        if alive {
            continue;
        }
        lane.state = "gone".into();
        lane.write(layout)?;
        lines.push(format!("herdr-pro resume {}", lane.name));
    }
    Ok(lines)
}

/// `herdr-pro stop`: the plugin's stop switch. Mark the lane stopped and close
/// its tab so reconcile never brings it back.
pub fn stop(env: &Env, layout: &Layout, runner: &dyn Runner, name: &str) -> Result<Lane> {
    let _start = state::FileLock::acquire(&layout.start_lock())?;
    let _turn = state::FileLock::acquire(&layout.turn_lock())?;
    let mut lane = Lane::read(layout, name)?;
    lane.stopped = true;
    lane.state = "gone".into();
    lane.write(layout)?;
    if !lane.tab_id.is_empty() && !lane.pane_id.is_empty() {
        let _ = herdr_cli::tab_close(runner, &env.herdr_bin(), &lane.tab_id);
    }
    Ok(lane)
}

/// Whether the lane's herdr pane is ready for a prompt.
pub fn agent_ready(env: &Env, runner: &dyn Runner, lane: &Lane) -> Result<Agent> {
    let agent = herdr_cli::agent_find(runner, &env.herdr_bin(), &lane.name)?
        .with_context(|| format!("herdr has no agent named `{}`", lane.name))?;
    Ok(agent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pro::sh::fake::{FakeRunner, fail, ok};

    /// An `Env` and `Layout` over one temp dir with the same state root, the
    /// way `herdr-pro` builds them in production.
    fn test_env(dir: &Path) -> (Env, Layout) {
        let state = dir.join("pro");
        let env = Env::for_test(
            dir,
            &[
                ("HERDR_BIN_PATH", "/h/herdr"),
                ("HERDR_PRO_STATE_DIR", state.to_str().unwrap()),
            ],
        );
        let layout = Layout::for_test(state);
        layout.ensure().unwrap();
        (env, layout)
    }

    #[test]
    fn codex_args_carry_the_route_and_the_tool_limit() {
        let args = codex_args(17841);
        let line = args.join(" ");
        assert!(line.contains("model=chatgpt-web/pro"), "{line}");
        assert!(line.contains("model_reasoning_effort=ultra"), "{line}");
        assert!(
            line.contains("openai_base_url=http://127.0.0.1:17841/v1"),
            "{line}"
        );
        assert!(line.contains("tool_output_token_limit=60000"), "{line}");
        let resume = codex_resume_args(17841, "abc");
        assert_eq!(resume[0], "resume");
        assert_eq!(resume.last().unwrap(), "abc");
        assert!(resume.contains(&"-c".to_string()));
    }

    #[test]
    fn trust_requires_the_exact_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path(), &[]);
        let home = env.lane_codex_home();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.toml"),
            format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                dir.path().display()
            ),
        )
        .unwrap();
        let deep = dir.path().join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        // A trusted ancestor does not cover the subdirectory: Codex trusts
        // exact project paths only.
        assert!(trusted(&env, dir.path()));
        assert!(!trusted(&env, &deep));
        std::fs::write(home.join("config.toml"), "").unwrap();
        assert!(!trusted(&env, dir.path()));
    }

    #[test]
    fn a_trust_prompt_on_screen_is_blocked_and_closes_the_tab() {
        let dir = tempfile::tempdir().unwrap();
        let (env, layout) = test_env(dir.path());
        let runner = FakeRunner::new();
        runner.on(
            ":17841/healthz",
            ok(r#"{"version":"5.0.8","mode":"browser-only","accepting_turns":true}"#),
        );
        runner.on("codex login status", ok("Logged in\n"));
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","cwd":"/w"}}}"#),
        );
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"w1:p2","name":"pro","agent":"codex","agent_status":"idle"}}}"#),
        );
        runner.on(
            "pane read",
            ok("Do you trust the contents of this directory?\n1. Yes, continue\n2. No, quit\n"),
        );
        runner.on("tab close", ok(r#"{"result":{}}"#));
        let error = start(
            &env,
            &layout,
            &runner,
            &StartOptions {
                name: "pro".into(),
                cwd: Some(dir.path().display().to_string()),
                profile: None,
                images: Vec::new(),
                ready_timeout_ms: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("trust"), "{error}");
        assert!(error.to_string().contains("w1:p2"), "{error}");
        assert_eq!(runner.count("tab close"), 1);
        assert_eq!(runner.count("agent prompt"), 0);
    }

    #[test]
    fn start_waits_for_the_rollout_before_ready() {
        let dir = tempfile::tempdir().unwrap();
        let (env, layout) = test_env(dir.path());
        // The rollout exists before the lane starts, so the poll returns at
        // once; matching is by session id.
        let sessions = layout.codex_home().join("sessions/2026/09/19");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("rollout-abc.jsonl"),
            format!(
                "{{\"timestamp\":\"2026-09-19T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"abc\",\"cwd\":\"{}\",\"timestamp\":\"2026-09-19T10:00:00Z\"}}}}\n",
                dir.path().display()
            ),
        )
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            ":17841/healthz",
            ok(r#"{"version":"5.0.8","mode":"browser-only","accepting_turns":true}"#),
        );
        runner.on("codex login status", ok("Logged in\n"));
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","cwd":"/w"}}}"#),
        );
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"w1:p2","name":"pro","agent":"codex","agent_status":"idle","agent_session":{"id":"abc"}}}}"#),
        );
        runner.on("pane read", ok("codex> \n"));
        let lane = start(
            &env,
            &layout,
            &runner,
            &StartOptions {
                name: "pro".into(),
                cwd: Some(dir.path().display().to_string()),
                profile: None,
                images: Vec::new(),
                ready_timeout_ms: None,
            },
        )
        .unwrap();
        assert!(
            lane.rollout
                .as_deref()
                .unwrap()
                .ends_with("rollout-abc.jsonl")
        );
        assert_eq!(lane.session_id.as_deref(), Some("abc"));
    }

    /// A fake clock: `now` never moves on its own and `sleep` jumps it
    /// forward, so the wait can run in a second of real time with any bound.
    struct FakeClock {
        origin: Instant,
        elapsed: std::cell::Cell<Duration>,
    }

    impl FakeClock {
        fn new() -> Self {
            FakeClock {
                origin: Instant::now(),
                elapsed: std::cell::Cell::new(Duration::ZERO),
            }
        }

        fn elapsed(&self) -> Duration {
            self.elapsed.get()
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            self.origin + self.elapsed.get()
        }

        fn sleep(&self, duration: Duration) {
            self.elapsed.set(self.elapsed.get() + duration);
        }
    }

    /// A bridge lane record with the rollout still missing.
    fn test_lane(dir: &Path) -> Lane {
        Lane {
            name: "pro".into(),
            pane_id: "w1:p2".into(),
            tab_id: "w1:t2".into(),
            workspace_id: "w1".into(),
            parent: None,
            cwd: dir.display().to_string(),
            profile: None,
            session_id: None,
            rollout: None,
            started_at: "2026-09-19T09:00:00Z".into(),
            state: "ready".into(),
            stopped: false,
            last_turn: None,
            ready_timeout_ms: None,
        }
    }

    #[test]
    fn a_rollout_later_than_the_old_thirty_second_bound_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let (env, _layout) = test_env(dir.path());
        let mut lane = test_lane(dir.path());
        let clock = FakeClock::new();
        let runner = FakeRunner::new();
        runner.on("pane read", ok("codex> \n"));
        // The rollout file appears on the 130th `agent list` poll, i.e. after
        // 130 * 250 ms = 32.5 s of fake time: past the old 30 s bound.
        let sessions = env.lane_codex_home().join("sessions/2026/09/19");
        let cwd = lane.cwd.clone();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let seen = calls.clone();
        runner.on_fn(
            |cmd| cmd.display().contains("agent list"),
            move |_| {
                seen.set(seen.get() + 1);
                if seen.get() == 130 {
                    std::fs::create_dir_all(&sessions).unwrap();
                    std::fs::write(
                        sessions.join("rollout-late.jsonl"),
                        format!(
                            "{{\"timestamp\":\"2026-09-19T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"late\",\"cwd\":\"{cwd}\",\"timestamp\":\"2026-09-19T10:00:00Z\"}}}}\n"
                        ),
                    )
                    .unwrap();
                }
                Ok(ok(r#"{"result":{"agents":[{"name":"pro","agent_status":"idle"}]}}"#))
            },
        );
        let outcome = wait_for_rollout(
            &env,
            &mut lane,
            &runner,
            "/h/herdr",
            ROLLOUT_TIMEOUT,
            &clock,
        );
        assert!(
            matches!(outcome, RolloutWait::Ready(_)),
            "the late rollout was not found"
        );
        assert!(
            clock.elapsed() > Duration::from_secs(30),
            "the wait ended before the old bound: {:?}",
            clock.elapsed()
        );
        assert_eq!(lane.session_id.as_deref(), Some("late"));
    }

    #[test]
    fn a_trust_prompt_during_the_wait_fails_closed_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let (env, _layout) = test_env(dir.path());
        let mut lane = test_lane(dir.path());
        let clock = FakeClock::new();
        let runner = FakeRunner::new();
        runner.on(
            "pane read",
            ok("Do you trust the contents of this directory?\n1. Yes, continue\n2. No, quit\n"),
        );
        runner.on(
            "agent list",
            ok(r#"{"result":{"agents":[{"name":"pro","agent_status":"idle"}]}}"#),
        );
        let outcome = wait_for_rollout(
            &env,
            &mut lane,
            &runner,
            "/h/herdr",
            ROLLOUT_TIMEOUT,
            &clock,
        );
        assert!(matches!(outcome, RolloutWait::TrustPrompt));
        assert_eq!(clock.elapsed(), Duration::ZERO);
        assert_eq!(runner.count("pane read"), 1);
    }

    #[test]
    fn the_outer_bound_still_fails_without_a_rollout() {
        let dir = tempfile::tempdir().unwrap();
        let (env, _layout) = test_env(dir.path());
        let mut lane = test_lane(dir.path());
        let clock = FakeClock::new();
        let runner = FakeRunner::new();
        runner.on("pane read", ok("codex> \n"));
        runner.on(
            "agent list",
            ok(r#"{"result":{"agents":[{"name":"pro","agent_status":"idle"}]}}"#),
        );
        let bound = Duration::from_secs(3);
        let outcome = wait_for_rollout(&env, &mut lane, &runner, "/h/herdr", bound, &clock);
        assert!(matches!(outcome, RolloutWait::TimedOut));
        assert!(clock.elapsed() >= bound, "{:?}", clock.elapsed());
    }

    #[test]
    fn a_dead_agent_during_the_wait_fails_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let (env, _layout) = test_env(dir.path());
        let mut lane = test_lane(dir.path());
        let clock = FakeClock::new();
        let runner = FakeRunner::new();
        runner.on("pane read", ok("codex> \n"));
        runner.on(
            "agent list",
            ok(r#"{"result":{"agents":[{"name":"pro","agent_status":"idle"}]}}"#),
        );
        runner.on(
            "pane process-info",
            ok(r#"{"result":{"process_info":{"foreground_processes":[{"pid":1,"name":"zsh"}]}}}"#),
        );
        let outcome = wait_for_rollout(
            &env,
            &mut lane,
            &runner,
            "/h/herdr",
            ROLLOUT_TIMEOUT,
            &clock,
        );
        assert!(matches!(outcome, RolloutWait::Gone));
        assert_eq!(clock.elapsed(), Duration::ZERO);
    }

    #[test]
    fn the_bound_is_the_recipe_value_else_the_constant() {
        assert_eq!(rollout_timeout(None), ROLLOUT_TIMEOUT);
        assert_eq!(rollout_timeout(Some(0)), ROLLOUT_TIMEOUT);
        assert_eq!(rollout_timeout(Some(45_000)), Duration::from_secs(45));
        assert_eq!(ROLLOUT_TIMEOUT, Duration::from_secs(180));
    }

    #[test]
    fn reconcile_marks_a_dead_pane_gone_and_prints_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let (env, layout) = test_env(dir.path());
        Lane {
            name: "pro".into(),
            pane_id: "w1:p2".into(),
            tab_id: "w1:t2".into(),
            workspace_id: "w1".into(),
            parent: None,
            cwd: "/w".into(),
            profile: None,
            session_id: Some("abc".into()),
            rollout: None,
            started_at: jiff::Timestamp::now().to_string(),
            state: "ready".into(),
            stopped: false,
            last_turn: None,
            ready_timeout_ms: None,
        }
        .write(&layout)
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "pane process-info",
            ok(r#"{"result":{"process_info":{"foreground_processes":[{"pid":1,"name":"zsh"}]}}}"#),
        );
        let lines = reconcile(&env, &layout, &runner).unwrap();
        assert_eq!(lines, vec!["herdr-pro resume pro"]);
        assert_eq!(Lane::read(&layout, "pro").unwrap().state, "gone");
    }

    #[test]
    fn reconciling_a_live_lane_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (env, layout) = test_env(dir.path());
        Lane {
            name: "pro".into(),
            pane_id: "w1:p2".into(),
            tab_id: "w1:t2".into(),
            workspace_id: "w1".into(),
            parent: None,
            cwd: "/w".into(),
            profile: None,
            session_id: Some("abc".into()),
            rollout: None,
            started_at: jiff::Timestamp::now().to_string(),
            state: "ready".into(),
            stopped: false,
            last_turn: None,
            ready_timeout_ms: None,
        }
        .write(&layout)
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "pane process-info",
            ok(r#"{"result":{"process_info":{"foreground_processes":[{"pid":1,"name":"codex"}]}}}"#),
        );
        assert!(reconcile(&env, &layout, &runner).unwrap().is_empty());
        assert_eq!(Lane::read(&layout, "pro").unwrap().state, "ready");
    }

    #[test]
    fn start_trusts_the_exact_cwd_in_the_pro_home() {
        let dir = tempfile::tempdir().unwrap();
        let (env, layout) = test_env(dir.path());
        let runner = FakeRunner::new();
        runner.on(
            ":17841/healthz",
            ok(r#"{"version":"5.0.8","mode":"browser-only","accepting_turns":true}"#),
        );
        runner.on("codex login status", ok("Logged in\n"));
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        let cwd = dir.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        // No `tab create` rule, so start fails there; the exact trust write is
        // what this checks. The plugin owns the Pro home, so the lane never
        // hits Codex's trust prompt.
        let error = start(
            &env,
            &layout,
            &runner,
            &StartOptions {
                name: "pro".into(),
                cwd: Some(cwd.display().to_string()),
                profile: None,
                images: Vec::new(),
                ready_timeout_ms: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("tab"), "{error}");
        assert!(trusted(&env, &cwd));
    }

    #[test]
    fn a_blocked_start_reports_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let (env, layout) = test_env(dir.path());
        let runner = FakeRunner::new();
        runner.on(
            ":17841/healthz",
            ok(r#"{"version":"5.0.8","mode":"browser-only","accepting_turns":true}"#),
        );
        runner.on("codex login status", ok("Logged in\n"));
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","cwd":"/w"}}}"#),
        );
        runner.on_fn(
            |cmd| cmd.display().contains("agent start"),
            |_| Ok(fail(1, "timed out waiting for ready")),
        );
        runner.on("pane read", ok("Sign in with ChatGPT\n"));
        let error = start(
            &env,
            &layout,
            &runner,
            &StartOptions {
                name: "pro".into(),
                cwd: Some(dir.path().display().to_string()),
                profile: None,
                images: Vec::new(),
                ready_timeout_ms: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("WAITING pro-bridge"), "{error}");
        assert!(error.to_string().contains("Sign in"), "{error}");
    }

    /// An `Env` and `Layout` with the given variables on top of `test_env`'s.
    fn nest_env(dir: &Path, extra: &[(&str, &str)]) -> (Env, Layout) {
        let state = dir.join("pro");
        let mut vars = vec![
            ("HERDR_BIN_PATH", "/h/herdr"),
            ("HERDR_PRO_STATE_DIR", state.to_str().unwrap()),
        ];
        vars.extend_from_slice(extra);
        let env = Env::for_test(dir, &vars);
        let layout = Layout::for_test(state);
        layout.ensure().unwrap();
        (env, layout)
    }

    fn profile_start(dir: &Path, images: Vec<PathBuf>) -> StartOptions {
        StartOptions {
            name: home::IMAGE_PROFILE.into(),
            cwd: Some(dir.display().to_string()),
            profile: Some(home::IMAGE_PROFILE.into()),
            images,
            ready_timeout_ms: None,
        }
    }

    #[test]
    fn start_nests_the_lane_under_the_callers_own_pane() {
        let dir = tempfile::tempdir().unwrap();
        let (env, layout) = nest_env(
            dir.path(),
            &[("HERDR_PANE_ID", "wC:p1"), ("HERDR_WORKSPACE_ID", "wC")],
        );
        let runner = FakeRunner::new();
        runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"pane_id":"wC:p2","tab_id":"wC:t2","workspace_id":"wC","cwd":"/w"}}}"#),
        );
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"wC:p2","name":"gpt-image-gen","agent":"codex","agent_status":"idle"}}}"#),
        );
        runner.on("pane read", ok("codex> \n"));
        let lane = start(
            &env,
            &layout,
            &runner,
            &profile_start(dir.path(), Vec::new()),
        )
        .unwrap();
        assert_eq!(lane.parent.as_deref(), Some("wC:p1"));
        let calls = runner.calls.borrow();
        let line = calls
            .iter()
            .map(|cmd| cmd.display())
            .find(|line| line.contains("agent start"))
            .unwrap();
        assert!(line.contains("--parent wC:p1"), "{line}");
    }

    #[test]
    fn start_reads_the_coordinator_pane_when_the_caller_is_not_a_herdr_pane() {
        let dir = tempfile::tempdir().unwrap();
        let ade = dir.path().join("ade");
        let (env, layout) = nest_env(
            dir.path(),
            &[
                ("HERDR_ADE_ROOT", ade.to_str().unwrap()),
                ("HERDR_ADE_LAUNCH", "demo/coordinator/1/abcd"),
            ],
        );
        let state = ade.join("demo/.state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("coordinator.json"), r#"{"pane_id":"wD:p9"}"#).unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"pane_id":"wD:p2","tab_id":"wD:t2","workspace_id":"wD","cwd":"/w"}}}"#),
        );
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"wD:p2","name":"gpt-image-gen","agent":"codex","agent_status":"idle"}}}"#),
        );
        runner.on("pane read", ok("codex> \n"));
        let lane = start(
            &env,
            &layout,
            &runner,
            &profile_start(dir.path(), Vec::new()),
        )
        .unwrap();
        assert_eq!(lane.parent.as_deref(), Some("wD:p9"));
        let calls = runner.calls.borrow();
        let line = calls
            .iter()
            .map(|cmd| cmd.display())
            .find(|line| line.contains("agent start"))
            .unwrap();
        assert!(line.contains("--parent wD:p9"), "{line}");
    }

    #[test]
    fn start_attaches_the_with_pictures_to_the_profile_line() {
        let dir = tempfile::tempdir().unwrap();
        let (env, layout) = test_env(dir.path());
        let image = dir.path().join("ref.png");
        std::fs::write(&image, b"png").unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"pane_id":"wC:p2","tab_id":"wC:t2","workspace_id":"wC","cwd":"/w"}}}"#),
        );
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"wC:p2","name":"gpt-image-gen","agent":"codex","agent_status":"idle"}}}"#),
        );
        runner.on("pane read", ok("codex> \n"));
        start(
            &env,
            &layout,
            &runner,
            &profile_start(dir.path(), vec![image.clone()]),
        )
        .unwrap();
        let calls = runner.calls.borrow();
        let line = calls
            .iter()
            .map(|cmd| cmd.display())
            .find(|line| line.contains("agent start"))
            .unwrap();
        assert!(
            line.contains(&format!("--image {}", image.display())),
            "{line}"
        );
    }

    #[test]
    fn find_rollout_matches_cwd_and_timestamp_then_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions/2026/09/19");
        std::fs::create_dir_all(&sessions).unwrap();
        let write = |name: &str, id: &str, cwd: &str, at: &str| {
            let path = sessions.join(name);
            std::fs::write(
                &path,
                format!(
                    "{{\"timestamp\":\"{at}\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"{cwd}\",\"timestamp\":\"{at}\"}}}}\n"
                ),
            )
            .unwrap();
            path
        };
        write("rollout-a.jsonl", "id-a", "/w", "2026-09-19T01:00:00Z");
        let newer = write("rollout-b.jsonl", "id-b", "/w", "2026-09-19T02:00:00Z");
        let after: jiff::Timestamp = "2026-09-19T00:00:00Z".parse().unwrap();
        assert_eq!(find_rollout(dir.path(), "/w", after, None).unwrap(), newer);
        assert_eq!(
            find_rollout(dir.path(), "/w", after, Some("id-a")).unwrap(),
            sessions.join("rollout-a.jsonl")
        );
        assert!(find_rollout(dir.path(), "/other", after, None).is_none());
    }
}
