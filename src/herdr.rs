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
    fn cmd(&self, timeout: Duration) -> Cmd {
        let cmd = Cmd::new(&self.bin, timeout)
            .env("HERDR_SOCKET_PATH", self.socket.to_string_lossy())
            .env_remove("HERDR_SESSION");
        match &self.machine {
            Some(machine) => cmd.args(["--machine", machine]),
            None => cmd,
        }
    }

    fn require_session(&self) -> Result<(), HerdrError> {
        // Forwarded calls select the saved machine's session, not a local socket.
        if self.machine.is_none() && self.socket.as_os_str().is_empty() {
            return Err(HerdrError {
                code: "unreachable".into(),
                message: "no coordinator session recorded".into(),
            });
        }
        Ok(())
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

fn transport_error(error: anyhow::Error) -> HerdrError {
    HerdrError {
        code: if error.downcast_ref::<crate::runner::SpawnError>().is_some() {
            "exec_failed"
        } else if crate::remote::is_unreachable(&format!("{error:#}")) {
            "unreachable"
        } else {
            "failed"
        }
        .into(),
        message: format!("herdr transport failed: {error:#}"),
    }
}

pub(crate) const AGENT_START_TIMEOUT: Duration = Duration::from_secs(20);
/// Keep short names readable; long project slugs otherwise exceed the fork's
/// 32-byte agent-name limit. Preserve the role suffix and distinguish slugs.
pub(crate) fn project_agent_name(slug: &str, role: &str) -> String {
    let name = format!("hp-{slug}-{role}");
    if name.len() <= 32 {
        name
    } else {
        let hash = crate::thread::sha256_hex(slug.as_bytes());
        format!("hp-{}-{role}", &hash[..16])
    }
}

pub(crate) const MIN_AGENT_START_TIMEOUT_MS: u64 = 3_001;
const MAX_AGENT_START_TIMEOUT_MS: u64 = 300_000;

enum AgentTimeout {
    Start,
    Wait,
}

/// Fork validation: app/agents.rs requires start >3000 and <=300000 ms.
/// cli/agent.rs and api/wait.rs impose no bounds on wait or prompt-wait u64s.
/// Use the same effective value for herdr and the enclosing process deadline.
fn agent_timeout(timeout_ms: u64, operation: AgentTimeout) -> (String, Duration) {
    let timeout_ms = match operation {
        AgentTimeout::Start => {
            timeout_ms.clamp(MIN_AGENT_START_TIMEOUT_MS, MAX_AGENT_START_TIMEOUT_MS)
        }
        AgentTimeout::Wait => timeout_ms,
    };
    (
        timeout_ms.to_string(),
        Duration::from_millis(timeout_ms) + Duration::from_secs(5),
    )
}

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

#[derive(Debug, Clone, serde::Serialize, Deserialize, PartialEq, Default)]
pub(crate) struct Pane {
    pub(crate) pane_id: String,
    pub(crate) tab_id: String,
    pub(crate) workspace_id: String,
    #[serde(default)]
    pub(crate) cwd: String,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize, PartialEq, Default)]
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

#[derive(Debug, Clone, serde::Serialize, Deserialize, PartialEq, Default)]
pub(crate) struct AgentSession {
    #[serde(default, alias = "value")]
    pub(crate) id: String,
}

impl Agent {
    /// The one "ready for a prompt" predicate: state `idle` or `done`.
    pub(crate) fn ready(&self) -> bool {
        ready_state(&self.agent_status)
    }

    /// Herdr queues prompts during a turn without starting another wake.
    pub(crate) fn promptable(&self) -> bool {
        self.ready() || self.agent_status == "working"
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
    /// Exclusive lookup directory for a box agent. Passed at launch (after
    /// shell startup), not at pane creation where bashrc can override it.
    pub(crate) launch_bin: Option<&'a str>,
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
    #[serde(default)]
    pub(crate) argv: Option<Vec<String>>,
}

impl ProcessInfo {
    /// A foreground shell (or no process) proves the agent has exited. Tools
    /// and unavailable observations remain unknown, not a crash signal.
    pub(crate) fn agent_gone(&self, pane: &str) -> bool {
        self.pane_id == pane
            && self.foreground_processes.iter().all(|p| {
                matches!(
                    p.name.rsplit('/').next().unwrap_or_default(),
                    "sh" | "bash" | "zsh" | "fish"
                )
            })
    }

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
                code: if crate::remote::is_unreachable(error["message"].as_str().unwrap_or("")) {
                    "unreachable".into()
                } else {
                    error["code"].as_str().unwrap_or("failed").to_string()
                },
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
        code: if crate::remote::is_unreachable(&out.error_text()) {
            "unreachable"
        } else {
            "failed"
        }
        .into(),
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
        self.require_session()?;
        let cmd = self.cmd(timeout).args(args.iter().copied());
        let out = self.runner.run(&cmd).map_err(transport_error)?;
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

    /// `pane read` prints a terminal snapshot, not a JSON reply.
    pub(crate) fn pane_read_text(&self, pane: &str, source: &str) -> Result<String, HerdrError> {
        self.pane_read(pane, source, "text")
    }

    pub(crate) fn pane_read_ansi(&self, pane: &str, source: &str) -> Result<String, HerdrError> {
        self.pane_read(pane, source, "ansi")
    }

    fn pane_read(&self, pane: &str, source: &str, format: &str) -> Result<String, HerdrError> {
        self.require_session()?;
        let out = self
            .runner
            .run(
                &self
                    .cmd(CALL_TIMEOUT)
                    .args(["pane", "read", pane, "--source", source, "--format", format]),
            )
            .map_err(transport_error)?;
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
        let (timeout_ms, wait) = agent_timeout(opts.ready_timeout_ms, AgentTimeout::Start);
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
        if let Some(bin) = opts.launch_bin {
            args.push("--env".into());
            args.push(format!("PATH={bin}"));
        }
        if !opts.agent_args.is_empty() {
            args.push("--".into());
            args.extend(opts.agent_args.iter().cloned());
        }
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
        if let Err(error) = self.require_session() {
            return starts.iter().map(|_| Err(error.clone())).collect();
        }
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
                let output = output.map_err(transport_error)?;
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
        let (timeout, wait) = agent_timeout(timeout_ms, AgentTimeout::Wait);
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
            wait,
        )?;
        serde_json::from_value(result["agent"].clone()).map_err(|error| HerdrError {
            code: "failed".into(),
            message: format!("`herdr agent wait` reply changed: {error}"),
        })
    }

    /// Herdr 0.9.1's remote-api-bridge forwards newline-delimited JSON from
    /// stdin to its socket. The prompt CLI has no file/stdin option; putting
    /// arbitrary note text in argv can fail before the client even starts.
    fn input_request(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<(), HerdrError> {
        self.require_session()?;
        let input = format!(
            "{}\n",
            serde_json::json!({
                "id": "ade:input", "method": method, "params": params,
            })
        );
        let output = if let Some(machine) = &self.machine {
            let profile = crate::remote::saved_herdr_profile(self.runner, &self.bin, machine)
                .map_err(|error| HerdrError {
                    code: "failed".into(),
                    message: format!("{error:#}"),
                })?;
            let cmd = crate::remote::ssh_cmd(
                &profile.target,
                &format!(
                    "exec herdr --session {} remote-api-bridge",
                    crate::remote::quote(&profile.session)
                ),
                Some(&input),
                timeout,
            )
            .map_err(transport_error)?;
            self.runner.run(&cmd)
        } else {
            self.runner.run(
                &self
                    .cmd(timeout)
                    .own_group()
                    .arg("remote-api-bridge")
                    .stdin(input),
            )
        }
        .map_err(transport_error)?;
        decode_call(method, output).map(|_| ())
    }

    pub(crate) fn agent_prompt(&self, target: &str, text: &str) -> Result<(), HerdrError> {
        self.input_request(
            "agent.prompt",
            serde_json::json!({"target": target, "text": text}),
            CALL_TIMEOUT,
        )
    }

    /// Submit through the pane surface when an adapter has identified its own
    /// recoverable error screen. `agent prompt` deliberately refuses every
    /// blocked state, including this adapter-owned one.
    pub(crate) fn pane_submit_text(&self, pane: &str, text: &str) -> Result<(), HerdrError> {
        self.input_request(
            "pane.send_text",
            serde_json::json!({"pane_id": pane, "text": text}),
            CALL_TIMEOUT,
        )?;
        self.pane_send_keys(pane, "Enter")
    }

    pub(crate) fn pane_send_keys(&self, pane: &str, key: &str) -> Result<(), HerdrError> {
        self.call(&["pane", "send-keys", pane, key], CALL_TIMEOUT)
            .map(|_| ())
    }

    /// Submits a prompt and requires herdr to observe working or blocked activity.
    pub(crate) fn agent_prompt_wait_started(
        &self,
        target: &str,
        text: &str,
        timeout_ms: u64,
    ) -> Result<(), HerdrError> {
        let (_, wait) = agent_timeout(timeout_ms, AgentTimeout::Wait);
        self.input_request(
            "agent.prompt",
            serde_json::json!({
                "target": target, "text": text,
                "wait": {"until": ["working", "blocked"], "timeout_ms": timeout_ms},
            }),
            wait,
        )
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

    /// Adopt / reconcile the sidebar parent token without a TTL.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_notes_use_stdin_for_local_and_saved_machine_input() {
        use crate::runner::fake::{FakeRunner, ok};
        let runner = FakeRunner::new();
        runner.on("machine list --json", ok(r#"[{"id":"box","label":"box","target":"box","session":"scratch-t-0825","enabled":true}]"#));
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        runner.on("pane send-text", ok(r#"{"result":{}}"#));
        runner.on("pane send-keys", ok(r#"{"result":{}}"#));
        let herdr = Herdr::new("herdr", "/test.sock", &runner);
        let text = "x".repeat(131_072);
        for bound in [herdr.on_machine(""), herdr.on_machine("box")] {
            bound.agent_prompt("w1:p1", &text).unwrap();
            bound
                .agent_prompt_wait_started("w1:p1", &text, 20_000)
                .unwrap();
            bound.pane_submit_text("w1:p1", &text).unwrap();
        }
        let calls = runner.calls.borrow();
        let inputs: Vec<_> = calls
            .iter()
            .filter(|cmd| cmd.args.iter().any(|arg| arg.contains("remote-api-bridge")))
            .collect();
        assert_eq!(inputs.len(), 6);
        for cmd in inputs {
            assert!(cmd.args.iter().all(|arg| arg.len() < 1024));
            let request: serde_json::Value =
                serde_json::from_str(cmd.stdin.as_deref().unwrap()).unwrap();
            assert_eq!(request["params"]["text"], text);
            if cmd.program == "ssh" {
                assert!(
                    cmd.args
                        .iter()
                        .any(|arg| arg.contains("--session") && arg.contains("scratch-t-0825"))
                );
            }
        }
    }

    #[test]
    fn a_large_notes_exec_failure_is_definite_not_server_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("missing-herdr");
        let runner = crate::runner::RealRunner;
        let herdr = Herdr::new(bin.to_string_lossy(), "/test.sock", &runner);
        let error = herdr
            .agent_prompt("w1:p1", &"x".repeat(131_072))
            .unwrap_err();
        assert_eq!(error.code, "exec_failed");
        assert!(error.message.contains("could not execute"), "{error}");
        assert!(!error.message.contains("unreachable"), "{error}");
        assert!(crate::threads::prompt_refused_before_submission(&error));
    }

    #[test]
    fn project_names_fit_the_fork_for_coordinators_and_lanes() {
        use crate::runner::fake::{FakeRunner, ok};
        let runner = FakeRunner::new();
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1"}}}"#),
        );
        let herdr = Herdr::new("herdr", "/test.sock", &runner);
        let long = "a-project-name-long-enough";
        for name in [
            crate::coordinator::agent_name("demo"),
            crate::coordinator::agent_name(long),
            crate::thread::agent_name("demo", "t-0001"),
            crate::thread::agent_name(long, "t-0001"),
        ] {
            herdr
                .agent_start_opts(&AgentStart {
                    name: &name,
                    kind: "pi",
                    pane: "w1:p1",
                    agent_args: &[],
                    launch_bin: None,
                    parent: None,
                    ready_timeout_ms: 30_000,
                })
                .unwrap();
            assert!(name.len() <= 32);
        }
        assert_eq!(
            crate::coordinator::agent_name("demo"),
            "hp-demo-coordinator"
        );
        assert_eq!(
            crate::thread::agent_name("demo", "t-0001"),
            "hp-demo-t-0001"
        );
        assert_ne!(
            project_agent_name(long, "coordinator"),
            project_agent_name(&format!("{long}-other"), "coordinator")
        );
        assert_ne!(
            crate::thread::agent_name(long, "t-0001"),
            crate::thread::agent_name(long, "t-0002")
        );
    }

    #[test]
    fn agent_timeouts_follow_the_forks_operation_specific_bounds() {
        use crate::runner::fake::{FakeRunner, ok};
        let runner = FakeRunner::new();
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1"}}}"#),
        );
        runner.on(
            "agent wait",
            ok(r#"{"result":{"agent":{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1"}}}"#),
        );
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let herdr = Herdr::new("herdr", "/test.sock", &runner);
        for (requested, effective) in [
            (0, 3_001),
            (1_000, 3_001),
            (3_000, 3_001),
            (3_001, 3_001),
            (30_000, 30_000),
            (300_000, 300_000),
            (300_001, 300_000),
            (u64::MAX, 300_000),
        ] {
            herdr
                .agent_start_opts(&AgentStart {
                    name: "coordinator",
                    kind: "claude",
                    pane: "w1:p1",
                    agent_args: &[],
                    launch_bin: None,
                    parent: None,
                    ready_timeout_ms: requested,
                })
                .unwrap();
            let calls = runner.calls.borrow();
            let command = calls.last().unwrap();
            assert!(
                command
                    .args
                    .windows(2)
                    .any(|args| args == ["--timeout", &effective.to_string()])
            );
            assert_eq!(
                command.timeout,
                Duration::from_millis(effective) + Duration::from_secs(5)
            );
        }
        // Waits do not inherit start's settle delay or five-minute maximum.
        for requested in [0, 1_000, 300_001, u64::MAX] {
            herdr.agent_wait_ready("w1:p1", requested).unwrap();
            herdr
                .agent_prompt_wait_started("w1:p1", "hello", requested)
                .unwrap();
            let calls = runner.calls.borrow();
            for command in calls.iter().rev().take(2) {
                if let Some(input) = &command.stdin {
                    let request: serde_json::Value = serde_json::from_str(input).unwrap();
                    assert_eq!(request["params"]["wait"]["timeout_ms"], requested);
                } else {
                    assert!(
                        command
                            .args
                            .windows(2)
                            .any(|args| args == ["--timeout", &requested.to_string()])
                    );
                }
                assert_eq!(
                    command.timeout,
                    Duration::from_millis(requested) + Duration::from_secs(5)
                );
            }
        }
    }

    #[test]
    fn no_recorded_session_refuses_every_local_command_before_running_herdr() {
        let runner = crate::runner::fake::FakeRunner::new();
        let herdr = Herdr::new("herdr", "", &runner);
        for error in [
            herdr.tab_list().unwrap_err(),
            herdr.pane_read_text("w1:p1", "detection").unwrap_err(),
            herdr
                .agent_start_opts(&AgentStart {
                    name: "lane",
                    kind: "pi",
                    pane: "w1:p1",
                    agent_args: &[],
                    launch_bin: None,
                    parent: None,
                    ready_timeout_ms: 1,
                })
                .unwrap_err(),
        ] {
            assert_eq!(error.message, "no coordinator session recorded");
            assert_eq!(error.code, "unreachable");
        }
        assert!(!herdr.reachable());
        assert!(runner.calls.borrow().is_empty());
        // Closed local coordinators still have explicitly routed box work.
        runner.on(
            "--machine box tab list",
            crate::runner::fake::ok(r#"{"result":{"tabs":[]}}"#),
        );
        assert!(herdr.on_machine("box").tab_list().unwrap().is_empty());
        assert_eq!(runner.count("--machine box tab list"), 1);
    }

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
}
