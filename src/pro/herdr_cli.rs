//! Typed herdr calls for the Pro bridge (SPEC-pro-bridge v2, "Lane start").
//!
//! The `herdr-pro` binary cannot use `crate::herdr` (that module borrows the
//! ADE contracts and runner), so this is the small seam it needs, over the pi
//! module's `sh` runner. Session selection stays with the coordinator's
//! environment (`HERDR_SOCKET_PATH`), which the CLI reads itself.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use super::sh::{Cmd, Runner};

const CALL_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Pane {
    #[serde(default)]
    pub pane_id: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub cwd: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Agent {
    #[serde(default)]
    pub pane_id: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub agent_status: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub agent_session: Option<AgentSession>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct AgentSession {
    #[serde(default)]
    pub id: String,
}

impl Agent {
    /// The one "ready for a prompt" predicate: `idle` or `done`.
    pub fn ready(&self) -> bool {
        matches!(self.agent_status.as_str(), "idle" | "done")
    }

    pub fn blocked(&self) -> bool {
        self.agent_status == "blocked"
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ProcessInfo {
    #[serde(default)]
    pub foreground_processes: Vec<ForegroundProcess>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ForegroundProcess {
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub argv0: Option<String>,
}

impl ProcessInfo {
    /// True when a process named for the kind is still foreground.
    pub fn runs(&self, kind: &str) -> bool {
        self.foreground_processes.iter().any(|proc| {
            let program = proc
                .argv0
                .as_deref()
                .filter(|a| !a.is_empty())
                .unwrap_or(&proc.name);
            program.rsplit('/').next().is_some_and(|b| b == kind)
        })
    }
}

/// Runs one herdr command and returns the `result` object.
fn call(runner: &dyn Runner, bin: &str, args: &[&str], timeout: Duration) -> Result<Value> {
    let output = runner.run(&Cmd::new(bin, timeout).args(args.iter().copied()))?;
    if output.timed_out {
        anyhow::bail!("`herdr {}` timed out", args.join(" "));
    }
    let reply: Option<Value> = [&output.stdout, &output.stderr]
        .into_iter()
        .find_map(|text| serde_json::from_str(text.trim()).ok());
    if let Some(reply) = reply {
        if let Some(error) = reply.get("error") {
            anyhow::bail!(
                "herdr: {} ({})",
                error["message"].as_str().unwrap_or(""),
                error["code"].as_str().unwrap_or("failed")
            );
        }
        if output.success() {
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
    }
    if output.success() {
        return Ok(Value::Null);
    }
    anyhow::bail!("`herdr {}`: {}", args.join(" "), output.error_text())
}

fn call_as<T: serde::de::DeserializeOwned>(
    runner: &dyn Runner,
    bin: &str,
    args: &[&str],
    field: &str,
) -> Result<T> {
    let result = call(runner, bin, args, CALL_TIMEOUT)?;
    serde_json::from_value(result[field].clone())
        .with_context(|| format!("herdr's reply to `{}` changed", args.join(" ")))
}

pub fn agent_list(runner: &dyn Runner, bin: &str) -> Result<Vec<Agent>> {
    call_as(runner, bin, &["agent", "list"], "agents")
}

pub fn agent_find(runner: &dyn Runner, bin: &str, name: &str) -> Result<Option<Agent>> {
    Ok(agent_list(runner, bin)?
        .into_iter()
        .find(|agent| agent.name == name))
}

pub fn pane_get(runner: &dyn Runner, bin: &str, pane: &str) -> Result<Pane> {
    call_as(runner, bin, &["pane", "get", pane], "pane")
}

pub fn process_info(runner: &dyn Runner, bin: &str, pane: &str) -> Result<ProcessInfo> {
    call_as(
        runner,
        bin,
        &["pane", "process-info", "--pane", pane],
        "process_info",
    )
}

pub fn tab_close(runner: &dyn Runner, bin: &str, tab: &str) -> Result<()> {
    call(runner, bin, &["tab", "close", tab], CALL_TIMEOUT).map(|_| ())
}

/// The pane's visible text. `herdr pane read` prints plain text, not JSON.
pub fn pane_read(runner: &dyn Runner, bin: &str, pane: &str) -> Result<String> {
    let output = runner.run(&Cmd::new(bin, CALL_TIMEOUT).args([
        "pane",
        "read",
        pane,
        "--source",
        "recent-unwrapped",
    ]))?;
    if !output.success() {
        anyhow::bail!("`herdr pane read {pane}`: {}", output.error_text());
    }
    let text = if output.stdout.trim().is_empty() {
        output.stderr.clone()
    } else {
        output.stdout.clone()
    };
    Ok(text.trim().to_string())
}

/// `herdr tab create`; returns the root pane the lane will live in. `env` is
/// the v2 Pro-home pair (`CODEX_HOME=...`); empty in v1.
pub fn tab_create(
    runner: &dyn Runner,
    bin: &str,
    workspace: &str,
    cwd: &str,
    label: &str,
    env: &[String],
) -> Result<Pane> {
    let mut args = vec!["tab", "create"];
    if !workspace.is_empty() {
        args.extend(["--workspace", workspace]);
    }
    args.extend(["--cwd", cwd, "--label", label, "--no-focus"]);
    let mut owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    for pair in env {
        owned.push("--env".into());
        owned.push(pair.clone());
    }
    let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
    let result = call(runner, bin, &borrowed, CALL_TIMEOUT)?;
    serde_json::from_value(result["root_pane"].clone())
        .context("herdr's tab create reply has no root_pane")
}

/// Arguments for `herdr agent start` including `--parent` and the ready
/// timeout.
pub struct StartSpec<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub pane: &'a str,
    pub parent: Option<&'a str>,
    pub extra: &'a [String],
    pub ready_timeout_ms: u64,
}

/// `herdr agent start`; `extra` is passed after `--`.
pub fn agent_start(runner: &dyn Runner, bin: &str, spec: &StartSpec<'_>) -> Result<Agent> {
    let timeout = spec.ready_timeout_ms.to_string();
    let mut args = vec![
        "agent".to_string(),
        "start".to_string(),
        spec.name.to_string(),
        "--kind".to_string(),
        spec.kind.to_string(),
        "--pane".to_string(),
        spec.pane.to_string(),
        "--timeout".to_string(),
        timeout,
    ];
    if let Some(parent) = spec.parent.filter(|p| !p.is_empty()) {
        args.push("--parent".into());
        args.push(parent.to_string());
    }
    if !spec.extra.is_empty() {
        args.push("--".into());
        args.extend(spec.extra.iter().cloned());
    }
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let wait = Duration::from_millis(spec.ready_timeout_ms) + Duration::from_secs(5);
    let result = call(runner, bin, &borrowed, wait)?;
    serde_json::from_value(result["agent"].clone()).context("herdr's agent start reply changed")
}

/// Submit a prompt. Text in the second position is accepted even when it
/// starts with a dash (checked on 0.9.1).
pub fn agent_prompt(runner: &dyn Runner, bin: &str, target: &str, text: &str) -> Result<()> {
    call(
        runner,
        bin,
        &["agent", "prompt", target, text],
        CALL_TIMEOUT,
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pro::sh::fake::{FakeRunner, ok};

    #[test]
    fn agent_start_passes_parent_timeout_and_args() {
        let runner = FakeRunner::new();
        runner.on(
            "--parent",
            ok(r#"{"result":{"agent":{"pane_id":"w1:p2","name":"pro","agent":"codex","agent_status":"idle"}}}"#),
        );
        let args = vec!["-c".to_string(), "model=chatgpt-web/pro".to_string()];
        let agent = agent_start(
            &runner,
            "herdr",
            &StartSpec {
                name: "pro",
                kind: "codex",
                pane: "w1:p2",
                parent: Some("w1:p1"),
                extra: &args,
                ready_timeout_ms: 120_000,
            },
        )
        .unwrap();
        assert_eq!(agent.name, "pro");
        assert!(agent.ready());
        let line = runner.calls.borrow()[0].display();
        assert!(line.contains("--parent w1:p1"), "{line}");
        assert!(line.contains("--timeout 120000"), "{line}");
        assert!(line.contains("-c model=chatgpt-web/pro"), "{line}");
    }

    #[test]
    fn agent_find_reads_the_list() {
        let runner = FakeRunner::new();
        runner.on(
            "agent list",
            ok(r#"{"result":{"agents":[{"name":"a","agent_status":"idle"},{"name":"pro","agent_status":"blocked"}]}}"#),
        );
        let found = agent_find(&runner, "herdr", "pro").unwrap().unwrap();
        assert!(found.blocked());
        assert!(agent_find(&runner, "herdr", "nope").unwrap().is_none());
    }

    #[test]
    fn tab_create_uses_the_workspace_and_no_focus() {
        let runner = FakeRunner::new();
        runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"pane_id":"w1:p9","tab_id":"w1:t9","workspace_id":"w1","cwd":"/trusted"}}}"#),
        );
        let pane = tab_create(&runner, "herdr", "w1", "/trusted", "pro", &[]).unwrap();
        assert_eq!(pane.pane_id, "w1:p9");
        let line = runner.calls.borrow()[0].display();
        assert!(line.contains("--workspace w1"), "{line}");
        assert!(line.contains("--cwd /trusted"), "{line}");
        assert!(line.contains("--no-focus"), "{line}");
    }

    #[test]
    fn tab_create_passes_a_pro_home_env() {
        let runner = FakeRunner::new();
        runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"pane_id":"w1:p9","tab_id":"w1:t9","workspace_id":"w1","cwd":"/trusted"}}}"#),
        );
        let env = vec!["CODEX_HOME=/pro/codex".to_string()];
        tab_create(&runner, "herdr", "w1", "/trusted", "pro", &env).unwrap();
        let line = runner.calls.borrow()[0].display();
        assert!(line.contains("--env CODEX_HOME=/pro/codex"), "{line}");
    }

    #[test]
    fn process_info_reports_whether_codex_still_runs() {
        let info = ProcessInfo {
            foreground_processes: vec![ForegroundProcess {
                pid: 3,
                name: "codex".into(),
                argv0: Some("/opt/homebrew/bin/codex".into()),
            }],
        };
        assert!(info.runs("codex"));
        assert!(!info.runs("pi"));
    }

    #[test]
    fn an_error_reply_is_reported() {
        let runner = FakeRunner::new();
        runner.on(
            "agent prompt",
            ok(r#"{"error":{"code":"agent_not_found","message":"no agent"}}"#),
        );
        let error = agent_prompt(&runner, "herdr", "pro", "hi").unwrap_err();
        assert!(error.to_string().contains("agent_not_found"), "{error}");
    }
}
