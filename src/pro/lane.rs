//! Lane start, resume, reconcile and the stop switch (SPEC-pro-bridge v2,
//! "Lane lifecycle and turn protocol").
//!
//! A lane is a `codex` agent in its own tab whose own Codex process is pointed
//! at the bridge with `-c` overrides. Rolf's `~/.codex` is never written.

use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::herdr_cli::{self, Agent};
use super::sh::Runner;
use super::state::{self, Lane};
use super::{Env, Layout, MODEL, bridge, doctor};

/// `agent start` waits this long for a ready agent; a trust or sign-in prompt
/// shows as a blocked screen and the start times out.
const READY_TIMEOUT_MS: u64 = 120_000;

/// The exact trust prompt Codex shows for an untrusted directory (2026-09-19
/// live run, Codex 0.155.1). It does not always read as `blocked`, so the
/// screen is checked too.
pub const TRUST_PROMPT: &str = "Do you trust the contents of this directory?";

#[derive(Debug, Clone)]
pub struct StartOptions {
    pub name: String,
    pub parent: Option<String>,
    pub cwd: Option<String>,
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

fn write_lane_with_rollout(
    env: &Env,
    layout: &Layout,
    lane: &mut Lane,
    session_id: Option<&str>,
    started: jiff::Timestamp,
) {
    let rollout = find_rollout(&env.lane_codex_home(), &lane.cwd, started, session_id);
    if let Some(rollout) = rollout {
        if lane.session_id.is_none()
            && let Some(meta) = session_meta(&rollout)
            && !meta.id.is_empty()
        {
            lane.session_id = Some(meta.id);
        }
        lane.rollout = Some(rollout.display().to_string());
    }
    let _ = layout;
}

/// `herdr-pro start`: doctor, one serialized Codex start, then record.
pub fn start(env: &Env, layout: &Layout, runner: &dyn Runner, opts: &StartOptions) -> Result<Lane> {
    layout.ensure()?;
    state::check_name(&opts.name)?;
    if let Ok(existing) = Lane::read(layout, &opts.name)
        && existing.stopped
    {
        bail!("lane `{}` was stopped; use a new lane name", opts.name);
    }
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
    if state::name_taken(layout, &opts.name) {
        bail!("lane `{}` already exists and is not gone", opts.name);
    }

    let cwd = match opts.cwd.as_deref().filter(|c| !c.is_empty()) {
        Some(cwd) => env.expand_tilde(cwd),
        None => std::env::current_dir().context("could not read the current directory")?,
    };
    let cwd = std::path::absolute(&cwd).with_context(|| format!("bad cwd {}", cwd.display()))?;
    let cwd_text = cwd.display().to_string();
    if !trusted(env, &cwd) {
        bail!(
            "WAITING pro-bridge `{cwd_text}` is not trusted in {}; trust it once (or pass a trusted --cwd), never press through the prompt",
            env.lane_codex_home().join("config.toml").display()
        );
    }

    let bin = env.herdr_bin();
    let _lock = state::FileLock::acquire(&layout.start_lock())?;
    let workspace = workspace_for(env, runner, &bin, opts.parent.as_deref()).unwrap_or_default();
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
            parent: opts.parent.as_deref(),
            extra: &codex_args(port),
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
        bail!("WAITING pro-bridge the lane is blocked in pane {}: {reason}", pane.pane_id);
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
        parent: opts.parent.clone(),
        cwd: cwd_text,
        session_id: session_id.clone(),
        rollout: None,
        started_at: started.to_string(),
        state: "ready".into(),
        stopped: false,
        last_turn: None,
    };
    write_lane_with_rollout(env, layout, &mut lane, session_id.as_deref(), started);
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

/// The trust prompt currently on the pane's screen.
fn trust_prompt_showing(runner: &dyn Runner, bin: &str, pane: &str) -> bool {
    herdr_cli::pane_read(runner, bin, pane)
        .map(|text| text.contains(TRUST_PROMPT))
        .unwrap_or(false)
}

/// `herdr-pro resume`: start the lane again and resume its Codex thread.
pub fn resume(env: &Env, layout: &Layout, runner: &dyn Runner, name: &str) -> Result<Lane> {
    let mut lane = Lane::read(layout, name)?;
    if lane.stopped {
        bail!("lane `{name}` was stopped and cannot be resumed");
    }
    if lane.state != "gone" {
        bail!("lane `{name}` is {}, not gone", lane.state);
    }
    if state::cooldown_active(layout, jiff::Timestamp::now()) {
        bail!("WAITING pro-bridge cooldown is active");
    }
    let session = lane
        .session_id
        .clone()
        .filter(|id| !id.is_empty())
        .with_context(|| format!("lane `{name}` has no Codex session id to resume"))?;
    if let Some(other) = state::session_in_use(layout, &session, name) {
        bail!("Codex session {session} is already held by lane `{other}`");
    }
    if let Err(error) = doctor::gate(env, layout, runner) {
        bail!("WAITING pro-bridge {error:#}");
    }
    let (port, health) = bridge::health_any(runner)?;
    if !health.accepting {
        bail!("WAITING pro-bridge the bridge is draining");
    }
    let cwd = PathBuf::from(&lane.cwd);
    if !trusted(env, &cwd) {
        bail!("WAITING pro-bridge `{}` is not trusted", lane.cwd);
    }

    let bin = env.herdr_bin();
    let _lock = state::FileLock::acquire(&layout.start_lock())?;
    let workspace = workspace_for(env, runner, &bin, lane.parent.as_deref())
        .unwrap_or_else(|| lane.workspace_id.clone());
    let env_pairs: Vec<String> = env.codex_home_env().into_iter().collect();
    let pane = herdr_cli::tab_create(runner, &bin, &workspace, &lane.cwd, name, &env_pairs)
        .context("could not create the resume tab")?;
    let started = jiff::Timestamp::now();
    let agent = match herdr_cli::agent_start(
        runner,
        &bin,
        &herdr_cli::StartSpec {
            name,
            kind: "codex",
            pane: &pane.pane_id,
            parent: lane.parent.as_deref(),
            extra: &codex_resume_args(port, &session),
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
        bail!("WAITING pro-bridge the resumed lane is blocked in pane {}: {reason}", pane.pane_id);
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
    lane.state = "ready".into();
    lane.stopped = false;
    write_lane_with_rollout(env, layout, &mut lane, Some(&session), started);
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
        std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
        std::fs::write(
            dir.path().join(".codex/config.toml"),
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
        std::fs::write(dir.path().join(".codex/config.toml"), "").unwrap();
        assert!(!trusted(&env, dir.path()));
    }

    #[test]
    fn a_trust_prompt_on_screen_is_blocked_and_closes_the_tab() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
        std::fs::write(
            dir.path().join(".codex/config.toml"),
            format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
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
                parent: Some("w1:p1".into()),
                cwd: Some(dir.path().display().to_string()),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("trust"), "{error}");
        assert!(error.to_string().contains("w1:p2"), "{error}");
        assert_eq!(runner.count("tab close"), 1);
        assert_eq!(runner.count("agent prompt"), 0);
    }

    #[test]
    fn reconcile_marks_a_dead_pane_gone_and_prints_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        Lane {
            name: "pro".into(),
            pane_id: "w1:p2".into(),
            tab_id: "w1:t2".into(),
            workspace_id: "w1".into(),
            parent: None,
            cwd: "/w".into(),
            session_id: Some("abc".into()),
            rollout: None,
            started_at: jiff::Timestamp::now().to_string(),
            state: "ready".into(),
            stopped: false,
            last_turn: None,
        }
        .write(&layout)
        .unwrap();
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
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
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        Lane {
            name: "pro".into(),
            pane_id: "w1:p2".into(),
            tab_id: "w1:t2".into(),
            workspace_id: "w1".into(),
            parent: None,
            cwd: "/w".into(),
            session_id: Some("abc".into()),
            rollout: None,
            started_at: jiff::Timestamp::now().to_string(),
            state: "ready".into(),
            stopped: false,
            last_turn: None,
        }
        .write(&layout)
        .unwrap();
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = FakeRunner::new();
        runner.on(
            "pane process-info",
            ok(r#"{"result":{"process_info":{"foreground_processes":[{"pid":1,"name":"codex"}]}}}"#),
        );
        assert!(reconcile(&env, &layout, &runner).unwrap().is_empty());
        assert_eq!(Lane::read(&layout, "pro").unwrap().state, "ready");
    }

    #[test]
    fn start_refuses_an_untrusted_cwd_before_touching_herdr() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = FakeRunner::new();
        runner.on(
            ":17841/healthz",
            ok(r#"{"version":"5.0.8","mode":"browser-only","accepting_turns":true}"#),
        );
        runner.on("codex login status", ok("Logged in\n"));
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        let cwd = dir.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let error = start(
            &env,
            &layout,
            &runner,
            &StartOptions {
                name: "pro".into(),
                parent: None,
                cwd: Some(cwd.display().to_string()),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("not trusted"), "{error}");
        assert!(runner.count("tab create") == 0);
    }

    #[test]
    fn a_blocked_start_reports_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
        std::fs::write(
            dir.path().join(".codex/config.toml"),
            format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
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
                parent: Some("w1:p1".into()),
                cwd: Some(dir.path().display().to_string()),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("WAITING pro-bridge"), "{error}");
        assert!(error.to_string().contains("Sign in"), "{error}");
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
