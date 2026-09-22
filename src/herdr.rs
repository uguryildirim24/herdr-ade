//! Typed calls to the herdr CLI. Differences between herdr versions stay here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::runner::{Cmd, Runner};

pub(crate) const MIN_VERSION: Version = Version(0, 9, 1);
pub(crate) const CALL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version(pub u64, pub u64, pub u64);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Parses `herdr 0.9.1` and `herdr 0.9.2-preview.3`; a pre-release suffix is ignored.
fn parse_version(text: &str) -> Option<Version> {
    let token = text
        .split_whitespace()
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let core = token.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some(Version(parts.next()??, parts.next()??, parts.next()??))
}

/// A herdr command that does not talk to a server (`--version`, `session list`).
fn bare(bin: &str) -> Cmd {
    Cmd::new(bin, CALL_TIMEOUT)
}

pub(crate) fn version(bin: &str, runner: &dyn Runner) -> Result<Version> {
    let out = runner.run(&bare(bin).arg("--version"))?;
    if !out.success() {
        bail!("`{bin} --version` failed: {}", out.error_text());
    }
    parse_version(&out.stdout)
        .with_context(|| format!("could not read a version from `{}`", out.stdout.trim()))
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct SessionInfo {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) default: bool,
    #[serde(default)]
    pub(crate) running: bool,
    pub(crate) socket_path: PathBuf,
}

pub(crate) fn session_list(bin: &str, runner: &dyn Runner) -> Result<Vec<SessionInfo>> {
    #[derive(Deserialize)]
    struct Reply {
        sessions: Vec<SessionInfo>,
    }
    let out = runner.run(&bare(bin).args(["session", "list", "--json"]))?;
    if !out.success() {
        bail!("`{bin} session list` failed: {}", out.error_text());
    }
    let reply: Reply =
        serde_json::from_str(&out.stdout).context("`herdr session list --json` output changed")?;
    Ok(reply.sessions)
}

/// herdr bound to one session's socket. Every call a project makes goes through
/// this, so a project always talks to the session it was opened in.
pub(crate) struct Herdr<'a> {
    bin: String,
    socket: PathBuf,
    /// A saved SSH machine: every call is forwarded as `herdr --machine M ...`.
    machine: Option<String>,
    runner: &'a dyn Runner,
}

impl<'a> Herdr<'a> {
    pub(crate) fn new(
        bin: impl Into<String>,
        socket: impl Into<PathBuf>,
        runner: &'a dyn Runner,
    ) -> Self {
        Herdr {
            bin: bin.into(),
            socket: socket.into(),
            machine: None,
            runner,
        }
    }

    /// The same session, with calls forwarded to a saved machine.
    pub(crate) fn on_machine(&self, machine: &str) -> Herdr<'a> {
        Herdr {
            bin: self.bin.clone(),
            socket: self.socket.clone(),
            machine: (!machine.is_empty()).then(|| machine.to_string()),
            runner: self.runner,
        }
    }

    /// `HERDR_SESSION` is removed so an inherited value can never compete with
    /// the socket this project recorded.
    pub(crate) fn cmd(&self, timeout: Duration) -> Cmd {
        let cmd = Cmd::new(&self.bin, timeout)
            .exit_meaning(crate::runner::ExitMeaning::Structured)
            .env("HERDR_SOCKET_PATH", self.socket.to_string_lossy())
            .env_remove("HERDR_SESSION");
        match &self.machine {
            Some(machine) => cmd.args(["--machine", machine]),
            None => cmd,
        }
    }

    /// True when the socket file exists and the server answers.
    pub(crate) fn reachable(&self) -> bool {
        if !self.socket.exists() {
            return false;
        }
        self.runner
            .run(&self.cmd(CALL_TIMEOUT).args(["status", "server"]))
            .map(|out| out.success())
            .unwrap_or(false)
    }
}

/// A herdr call that failed. `code` is herdr's own error code (for example
/// `agent_blocked` or `pane_not_found`), or `timeout` / `unreachable` / `failed`
/// when herdr never answered with one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HerdrError {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl std::fmt::Display for HerdrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "herdr: {} ({})", self.message, self.code)
    }
}

impl std::error::Error for HerdrError {}

pub(crate) const AGENT_START_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub(crate) struct Workspace {
    pub(crate) workspace_id: String,
    #[serde(default)]
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) tab_count: usize,
    #[serde(default)]
    pub(crate) pane_count: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub(crate) struct Tab {
    pub(crate) tab_id: String,
    pub(crate) workspace_id: String,
    #[serde(default)]
    pub(crate) label: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub(crate) struct Pane {
    pub(crate) pane_id: String,
    pub(crate) tab_id: String,
    pub(crate) workspace_id: String,
    #[serde(default)]
    pub(crate) cwd: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub(crate) struct Agent {
    pub(crate) pane_id: String,
    pub(crate) tab_id: String,
    pub(crate) workspace_id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) agent: String,
    #[serde(default)]
    pub(crate) agent_status: String,
    #[serde(default)]
    pub(crate) cwd: String,
    /// Pane tokens from `agent list` (SPEC-ADE D3).
    #[serde(default)]
    pub(crate) tokens: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) agent_session: Option<AgentSession>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub(crate) struct AgentSession {
    #[serde(default)]
    pub(crate) id: String,
}

impl Agent {
    /// The one "ready for a prompt" predicate: state `idle` or `done`.
    pub(crate) fn ready(&self) -> bool {
        ready_state(&self.agent_status)
    }

    pub(crate) fn parent(&self) -> Option<&str> {
        self.tokens.get("parent").map(String::as_str)
    }
}

/// Arguments for `agent start` including `--parent` and `ready_timeout_ms`.
#[derive(Debug, Clone)]
pub(crate) struct AgentStart<'a> {
    pub(crate) name: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) pane: &'a str,
    pub(crate) agent_args: &'a [String],
    pub(crate) parent: Option<&'a str>,
    pub(crate) ready_timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub(crate) struct ProcessInfo {
    #[serde(default)]
    pub(crate) pane_id: String,
    #[serde(default)]
    pub(crate) foreground_processes: Vec<ForegroundProcess>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub(crate) struct ForegroundProcess {
    pub(crate) pid: u32,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) argv0: Option<String>,
}

impl ProcessInfo {
    /// First foreground process, used as identity evidence (SPEC-ADE D3).
    /// Every foreground process: while the agent runs a tool, the tool is
    /// in the foreground beside it.
    pub(crate) fn identities(&self) -> Vec<crate::contracts::ProcessIdentity> {
        self.foreground_processes
            .iter()
            .map(|proc| crate::contracts::ProcessIdentity {
                pid: proc.pid,
                argv0: proc
                    .argv0
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| proc.name.clone()),
            })
            .collect()
    }

    /// The process to bind for an agent of `kind`: the one whose program is
    /// named for the kind, else the first (A1 review M2).
    pub(crate) fn identity(&self, kind: &str) -> Option<crate::contracts::ProcessIdentity> {
        let all = self.identities();
        all.iter()
            .find(|p| {
                !kind.is_empty() && p.argv0.rsplit('/').next().is_some_and(|b| b.contains(kind))
            })
            .cloned()
            .or_else(|| all.into_iter().next())
    }
}

pub(crate) fn ready_state(state: &str) -> bool {
    matches!(state, "idle" | "done")
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Created {
    pub(crate) workspace_id: String,
    pub(crate) tab_id: String,
    pub(crate) pane_id: String,
}

fn decode_call(label: &str, out: crate::runner::Output) -> Result<serde_json::Value, HerdrError> {
    if out.timed_out {
        return Err(HerdrError {
            code: "timeout".into(),
            message: format!("`herdr {label}` timed out"),
        });
    }
    let reply = [&out.stdout, &out.stderr]
        .into_iter()
        .find_map(|text| serde_json::from_str::<serde_json::Value>(text.trim()).ok());
    if let Some(reply) = reply {
        if let Some(error) = reply.get("error") {
            return Err(HerdrError {
                code: error["code"].as_str().unwrap_or("failed").to_string(),
                message: error["message"].as_str().unwrap_or("").to_string(),
            });
        }
        if out.success() {
            return Ok(reply
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null));
        }
    }
    if out.success() && out.stdout.trim().is_empty() && out.stderr.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    Err(HerdrError {
        code: "failed".into(),
        message: format!("`herdr {label}`: {}", out.error_text()),
    })
}

impl<'a> Herdr<'a> {
    /// Runs one herdr command and returns the `result` object of its JSON reply.
    pub(crate) fn call(
        &self,
        args: &[&str],
        timeout: Duration,
    ) -> Result<serde_json::Value, HerdrError> {
        let cmd = self.cmd(timeout).args(args.iter().copied());
        let out = self.runner.run(&cmd).map_err(|e| HerdrError {
            code: "unreachable".into(),
            message: format!("{e:#}"),
        })?;
        decode_call(&args.join(" "), out)
    }

    fn call_as<T: serde::de::DeserializeOwned>(
        &self,
        args: &[&str],
        field: &str,
    ) -> Result<T, HerdrError> {
        let result = self.call(args, CALL_TIMEOUT)?;
        serde_json::from_value(result[field].clone()).map_err(|e| HerdrError {
            code: "failed".into(),
            message: format!("`herdr {}` reply changed: {e}", args.join(" ")),
        })
    }

    pub(crate) fn workspace_list(&self) -> Result<Vec<Workspace>, HerdrError> {
        self.call_as(&["workspace", "list"], "workspaces")
    }

    pub(crate) fn tab_list(&self) -> Result<Vec<Tab>, HerdrError> {
        self.call_as(&["tab", "list"], "tabs")
    }

    pub(crate) fn pane_list(&self) -> Result<Vec<Pane>, HerdrError> {
        self.call_as(&["pane", "list"], "panes")
    }

    pub(crate) fn agent_list(&self) -> Result<Vec<Agent>, HerdrError> {
        self.call_as(&["agent", "list"], "agents")
    }

    fn created(result: &serde_json::Value) -> Result<Created, HerdrError> {
        let pane = &result["root_pane"];
        match (
            pane["workspace_id"].as_str(),
            pane["tab_id"].as_str(),
            pane["pane_id"].as_str(),
        ) {
            (Some(w), Some(t), Some(p)) => Ok(Created {
                workspace_id: w.into(),
                tab_id: t.into(),
                pane_id: p.into(),
            }),
            _ => Err(HerdrError {
                code: "failed".into(),
                message: "herdr's create reply has no root_pane ids".into(),
            }),
        }
    }

    /// `workspace create` with `--env KEY=VALUE` for its first pane (the
    /// coordinator's `HERDR_ADE_LAUNCH`, SPEC-ADE D14).
    pub(crate) fn workspace_create_env(
        &self,
        cwd: &Path,
        label: &str,
        focus: bool,
        env: &[String],
    ) -> Result<Created, HerdrError> {
        let cwd = cwd.to_string_lossy();
        let focus = if focus { "--focus" } else { "--no-focus" };
        let mut args = vec![
            "workspace".to_string(),
            "create".to_string(),
            "--cwd".to_string(),
            cwd.into_owned(),
            "--label".to_string(),
            label.to_string(),
            focus.to_string(),
        ];
        for pair in env {
            args.push("--env".into());
            args.push(pair.clone());
        }
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = self.call(&borrowed, CALL_TIMEOUT)?;
        Self::created(&result)
    }

    /// `tab create` with `--env KEY=VALUE` (SPEC-ADE D4).
    pub(crate) fn tab_create_env(
        &self,
        workspace: &str,
        cwd: &Path,
        label: &str,
        focus: bool,
        env: &[String],
    ) -> Result<Created, HerdrError> {
        let cwd = cwd.to_string_lossy();
        let focus = if focus { "--focus" } else { "--no-focus" };
        let mut args = vec![
            "tab".to_string(),
            "create".to_string(),
            "--workspace".to_string(),
            workspace.to_string(),
            "--cwd".to_string(),
            cwd.into_owned(),
            "--label".to_string(),
            label.to_string(),
            focus.to_string(),
        ];
        for pair in env {
            args.push("--env".into());
            args.push(pair.clone());
        }
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = self.call(&borrowed, CALL_TIMEOUT)?;
        Self::created(&result)
    }

    pub(crate) fn tab_rename(&self, tab: &str, label: &str) -> Result<(), HerdrError> {
        self.call(&["tab", "rename", tab, label], CALL_TIMEOUT)
            .map(|_| ())
    }

    pub(crate) fn tab_close(&self, tab: &str) -> Result<(), HerdrError> {
        self.call(&["tab", "close", tab], CALL_TIMEOUT).map(|_| ())
    }

    /// A workspace's label, as the sidebar shows it.
    pub(crate) fn workspace_label(&self, workspace: &str) -> Result<String, HerdrError> {
        let result = self.call(&["workspace", "get", workspace], CALL_TIMEOUT)?;
        Ok(result["workspace"]["label"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    pub(crate) fn workspace_rename(&self, workspace: &str, label: &str) -> Result<(), HerdrError> {
        self.call(&["workspace", "rename", workspace, label], CALL_TIMEOUT)
            .map(|_| ())
    }

    /// The working directory herdr reports for a new tab's pane.
    pub(crate) fn pane_cwd(&self, pane: &str) -> Result<String, HerdrError> {
        Ok(self.pane_get(pane)?.cwd)
    }

    pub(crate) fn pane_get(&self, pane: &str) -> Result<Pane, HerdrError> {
        let result = self.call(&["pane", "get", pane], CALL_TIMEOUT)?;
        serde_json::from_value(result["pane"].clone()).map_err(|e| HerdrError {
            code: "failed".into(),
            message: format!("`herdr pane get` reply changed: {e}"),
        })
    }

    pub(crate) fn pane_process_info(&self, pane: &str) -> Result<ProcessInfo, HerdrError> {
        let result = self.call(&["pane", "process-info", "--pane", pane], CALL_TIMEOUT)?;
        serde_json::from_value(result["process_info"].clone()).map_err(|e| HerdrError {
            code: "failed".into(),
            message: format!("`herdr pane process-info` reply changed: {e}"),
        })
    }

    /// Types one command line into a pane's own shell (SPEC-remote §3.3).
    pub(crate) fn pane_run(&self, pane: &str, command: &str) -> Result<(), HerdrError> {
        self.call(&["pane", "run", pane, command], CALL_TIMEOUT)
            .map(|_| ())
    }

    /// `pane read` prints text, not a JSON reply, so this returns it verbatim.
    pub(crate) fn pane_read_text(&self, pane: &str, source: &str) -> Result<String, HerdrError> {
        let out = self
            .runner
            .run(
                &self
                    .cmd(CALL_TIMEOUT)
                    .args(["pane", "read", pane, "--source", source, "--format", "text"]),
            )
            .map_err(|e| HerdrError {
                code: "unreachable".into(),
                message: format!("{e:#}"),
            })?;
        if !out.success() {
            return Err(HerdrError {
                code: "failed".into(),
                message: format!("`herdr pane read`: {}", out.error_text()),
            });
        }
        Ok(out.stdout)
    }

    /// Closes a whole workspace, used to clean up a probe pane.
    pub(crate) fn workspace_close(&self, workspace: &str) -> Result<(), HerdrError> {
        self.call(&["workspace", "close", workspace], CALL_TIMEOUT)
            .map(|_| ())
    }

    fn agent_start_command(&self, opts: &AgentStart<'_>) -> (Vec<String>, Cmd) {
        let timeout_ms = opts.ready_timeout_ms.to_string();
        let mut args = vec![
            "agent".to_string(),
            "start".to_string(),
            opts.name.to_string(),
            "--kind".to_string(),
            opts.kind.to_string(),
            "--pane".to_string(),
            opts.pane.to_string(),
            "--timeout".to_string(),
            timeout_ms,
        ];
        if let Some(parent) = opts.parent.filter(|p| !p.is_empty()) {
            args.push("--parent".into());
            args.push(parent.to_string());
        }
        if !opts.agent_args.is_empty() {
            args.push("--".into());
            args.extend(opts.agent_args.iter().cloned());
        }
        let wait = Duration::from_millis(opts.ready_timeout_ms) + Duration::from_secs(5);
        let command = self.cmd(wait).args(args.iter().cloned());
        (args, command)
    }

    /// `agent start` with `--parent` and `ready_timeout_ms` (SPEC-ADE D2, D3).
    pub(crate) fn agent_start_opts(&self, opts: &AgentStart<'_>) -> Result<Agent, HerdrError> {
        self.agent_start_many(std::slice::from_ref(opts))
            .pop()
            .expect("one start has one result")
    }

    /// Starts independent agents concurrently. Results keep input order, so
    /// each durable thread record receives only its own launch outcome.
    pub(crate) fn agent_start_many(
        &self,
        starts: &[AgentStart<'_>],
    ) -> Vec<Result<Agent, HerdrError>> {
        let prepared: Vec<_> = starts
            .iter()
            .map(|opts| self.agent_start_command(opts))
            .collect();
        let commands: Vec<Cmd> = prepared
            .iter()
            .map(|(_, command)| command.clone())
            .collect();
        self.runner
            .run_parallel(&commands)
            .into_iter()
            .zip(prepared)
            .map(|(output, (args, _))| {
                let output = output.map_err(|error| HerdrError {
                    code: "unreachable".into(),
                    message: format!("{error:#}"),
                })?;
                let result = decode_call(&args.join(" "), output)?;
                serde_json::from_value(result["agent"].clone()).map_err(|error| HerdrError {
                    code: "failed".into(),
                    message: format!("`herdr agent start` reply changed: {error}"),
                })
            })
            .collect()
    }

    /// Waits for the same ready-for-input states used after `agent start`.
    pub(crate) fn agent_wait_ready(
        &self,
        target: &str,
        timeout_ms: u64,
    ) -> Result<Agent, HerdrError> {
        let timeout = timeout_ms.to_string();
        let result = self.call(
            &[
                "agent",
                "wait",
                target,
                "--until",
                "idle",
                "--until",
                "done",
                "--timeout",
                &timeout,
            ],
            Duration::from_millis(timeout_ms) + Duration::from_secs(5),
        )?;
        serde_json::from_value(result["agent"].clone()).map_err(|error| HerdrError {
            code: "failed".into(),
            message: format!("`herdr agent wait` reply changed: {error}"),
        })
    }

    /// Submits a prompt. herdr's parser takes positionals first and options
    /// after them, and has no `--` separator here; text in the second
    /// position is accepted even when it starts with a dash (checked on 0.9.1).
    pub(crate) fn agent_prompt(&self, target: &str, text: &str) -> Result<(), HerdrError> {
        self.call(&["agent", "prompt", target, text], CALL_TIMEOUT)
            .map(|_| ())
    }

    /// Submits a first prompt and requires herdr to observe that it started.
    pub(crate) fn agent_prompt_wait_started(
        &self,
        target: &str,
        text: &str,
        timeout_ms: u64,
    ) -> Result<(), HerdrError> {
        let timeout = timeout_ms.to_string();
        self.call(
            &[
                "agent",
                "prompt",
                target,
                text,
                "--wait",
                "--until",
                "working",
                "--until",
                "blocked",
                "--timeout",
                &timeout,
            ],
            Duration::from_millis(timeout_ms) + Duration::from_secs(5),
        )
        .map(|_| ())
    }

    pub(crate) fn agent_focus(&self, target: &str) -> Result<(), HerdrError> {
        self.call(&["agent", "focus", target], CALL_TIMEOUT)
            .map(|_| ())
    }

    /// Puts a name back on an agent that is already running in `target`.
    /// `agent start` drops the name when interactive readiness times out, and
    /// a live server handoff drops it on respawn; the process keeps running.
    pub(crate) fn agent_rename(&self, target: &str, name: &str) -> Result<(), HerdrError> {
        self.call(&["agent", "rename", target, name], CALL_TIMEOUT)
            .map(|_| ())
    }

    pub(crate) fn notification_show(&self, title: &str, body: &str) -> Result<(), HerdrError> {
        self.call(
            &["notification", "show", title, "--body", body],
            CALL_TIMEOUT,
        )
        .map(|_| ())
    }

    /// Display tokens on a pane row, always with a TTL so they fade if the
    /// ticker stops.
    pub(crate) fn pane_report_tokens(
        &self,
        pane: &str,
        tokens: &[(&str, &str)],
        ttl: Duration,
    ) -> Result<(), HerdrError> {
        let ttl = ttl.as_millis().to_string();
        let pairs: Vec<String> = tokens.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let mut args = vec![
            "pane",
            "report-metadata",
            pane,
            "--source",
            SOURCE,
            "--ttl-ms",
            &ttl,
        ];
        for pair in &pairs {
            args.push("--token");
            args.push(pair);
        }
        self.call(&args, CALL_TIMEOUT).map(|_| ())
    }

    pub(crate) fn pane_clear_tokens(&self, pane: &str, names: &[&str]) -> Result<(), HerdrError> {
        let mut args = vec!["pane", "report-metadata", pane, "--source", SOURCE];
        for name in names {
            args.push("--clear-token");
            args.push(name);
        }
        self.call(&args, CALL_TIMEOUT).map(|_| ())
    }

    /// Adopt / reconcile parent token: pane path, no TTL (SPEC-ADE D3).
    pub(crate) fn pane_set_parent(&self, pane: &str, parent: &str) -> Result<(), HerdrError> {
        let token = format!("parent={parent}");
        self.call(
            &[
                "pane",
                "report-metadata",
                pane,
                "--source",
                SOURCE,
                "--token",
                &token,
            ],
            CALL_TIMEOUT,
        )
        .map(|_| ())
    }
}

/// True when `herdr agent start --help` names `--parent` (the r2 fork).
pub(crate) fn parent_on_start_supported(bin: &str, runner: &dyn Runner) -> bool {
    runner
        .run(&bare(bin).args(["agent", "start", "--help"]))
        .ok()
        .map(|out| {
            let text = format!("{}{}", out.stdout, out.stderr);
            text.contains("--parent")
        })
        .unwrap_or(false)
}

pub(crate) const SOURCE: &str = "herdr-ade";

impl<'a> Herdr<'a> {
    fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, HerdrError> {
        let line = serde_json::json!({ "id": "herdr-ade", "method": method, "params": params })
            .to_string();
        let reply = self
            .runner
            .socket_request(&self.socket, &line, CALL_TIMEOUT)
            .map_err(|e| HerdrError {
                code: "unreachable".into(),
                message: format!("{e:#}"),
            })?;
        let reply: serde_json::Value =
            serde_json::from_str(reply.trim()).map_err(|e| HerdrError {
                code: "failed".into(),
                message: format!("herdr's reply to {method} did not parse: {e}"),
            })?;
        match reply.get("error") {
            Some(error) => Err(HerdrError {
                code: error["code"].as_str().unwrap_or("failed").to_string(),
                message: error["message"].as_str().unwrap_or("").to_string(),
            }),
            None => Ok(reply["result"].clone()),
        }
    }

    /// Filters the sidebar's agents to one project and sorts them by attention:
    /// the coordinator (rank 0) first, then by the group's display-order digit.
    /// herdr holds one transient view, so this replaces any other tool's view.
    pub(crate) fn agent_view_set_project(&self, slug: &str) -> Result<(), HerdrError> {
        self.request(
            "agent.view.set",
            serde_json::json!({
                "source": SOURCE,
                "label": format!("project: {slug}"),
                "filter": { "op": "eq", "field": { "token": "project" }, "value": slug },
                "sort": [{ "field": { "token": "rank" }, "order": "asc" }],
            }),
        )
        .map(|_| ())
    }

    pub(crate) fn agent_view_clear(&self) -> Result<(), HerdrError> {
        self.request("agent.view.clear", serde_json::json!({}))
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_reply_with_exit_zero_is_success_and_an_error_reply_is_not() {
        use crate::runner::fake::{FakeRunner, fail, ok};
        let runner = FakeRunner::new();
        runner.on("pane run w1:p1", ok(""));
        runner.on(
            "agent send-keys",
            fail(
                1,
                r#"{"error":{"code":"agent_not_found","message":"agent target w1:p1 not found"}}"#,
            ),
        );
        runner.on("pane send-keys", fail(1, ""));
        let h = Herdr::new("herdr", "/nonexistent.sock", &runner);
        let t = Duration::from_secs(1);
        assert_eq!(
            h.call(&["pane", "run", "w1:p1", "echo"], t).unwrap(),
            serde_json::Value::Null
        );
        let e = h
            .call(&["agent", "send-keys", "w1:p1", "esc"], t)
            .unwrap_err();
        assert_eq!(e.code, "agent_not_found");
        let e = h
            .call(&["pane", "send-keys", "w1:p1", "esc"], t)
            .unwrap_err();
        assert_eq!(e.code, "failed");
    }

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("herdr 0.9.0\n"), Some(Version(0, 9, 0)));
        assert_eq!(
            parse_version("herdr 0.9.2-preview.3"),
            Some(Version(0, 9, 2))
        );
        assert_eq!(parse_version("0.10.0"), Some(Version(0, 10, 0)));
        assert_eq!(parse_version("herdr"), None);
        assert!(Version(0, 9, 0) < MIN_VERSION);
        assert!(Version(0, 10, 0) > MIN_VERSION);
    }

    #[test]
    fn agent_start_opts_passes_parent_and_timeout() {
        use crate::runner::fake::{FakeRunner, ok};
        let runner = FakeRunner::new();
        runner.on(
            "--parent",
            ok(r#"{"result":{"agent":{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","name":"lane","tokens":{"parent":"w1:p1"}}}}"#),
        );
        let herdr = Herdr::new("herdr", "/tmp/x.sock", &runner);
        let args = vec!["--force".to_string()];
        let agent = herdr
            .agent_start_opts(&AgentStart {
                name: "lane",
                kind: "cursor",
                pane: "w2:p1",
                agent_args: &args,
                parent: Some("w1:p1"),
                ready_timeout_ms: 30_000,
            })
            .unwrap();
        assert_eq!(agent.parent(), Some("w1:p1"));
        let call = runner.calls.borrow();
        let line = call[0].display();
        assert!(line.contains("--parent w1:p1"), "{line}");
        assert!(line.contains("--timeout 30000"), "{line}");
        assert!(line.contains("--force"), "{line}");
    }

    #[test]
    fn tab_create_env_passes_launch_variable() {
        use crate::runner::fake::{FakeRunner, ok};
        let runner = FakeRunner::new();
        runner.on(
            "HERDR_ADE_LAUNCH",
            ok(r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","cwd":"/wt"}}}"#),
        );
        let herdr = Herdr::new("herdr", "/tmp/x.sock", &runner);
        let created = herdr
            .tab_create_env(
                "w1",
                Path::new("/wt"),
                "t-0001",
                false,
                &["HERDR_ADE_LAUNCH=demo/t-0001/1/abcd".into()],
            )
            .unwrap();
        assert_eq!(created.pane_id, "w1:p2");
        let line = runner.calls.borrow()[0].display();
        assert!(
            line.contains("--env HERDR_ADE_LAUNCH=demo/t-0001/1/abcd"),
            "{line}"
        );
        assert!(line.contains("--no-focus"), "{line}");
    }

    #[test]
    fn process_info_identity_uses_argv0() {
        let info = ProcessInfo {
            pane_id: "w2:p1".into(),
            foreground_processes: vec![ForegroundProcess {
                pid: 9,
                name: "cursor-agent".into(),
                argv0: Some("/bin/cursor-agent".into()),
            }],
        };
        let id = info.identity("cursor").unwrap();
        assert_eq!(id.pid, 9);
        assert_eq!(id.argv0, "/bin/cursor-agent");
    }
}
