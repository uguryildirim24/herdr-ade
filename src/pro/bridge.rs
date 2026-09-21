//! The installed `codex-chatgpt-web` bridge: health, mode and the two control
//! calls (SPEC-pro-bridge v2, "Bridge host" and §4).
//!
//! Local only, never a model turn. The plugin does not start or stop the
//! bridge; `/admin/drain` makes Codex's own retries get a local 503 so they
//! reach no browser (the breaker).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::sh::{Cmd, Runner};
use super::{Env, SEEN_BRIDGE_MAJORS};

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Health {
    pub(crate) version: String,
    pub(crate) major: u64,
    pub(crate) accepting: bool,
    pub(crate) pid: Option<u32>,
    pub(crate) mode: Option<String>,
    pub(crate) raw: Value,
}

impl Health {
    /// An unseen major is refused (spec Design, check 6).
    pub(crate) fn major_seen(&self) -> bool {
        SEEN_BRIDGE_MAJORS.contains(&self.major)
    }
}

fn url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

/// `GET /healthz`, one short local call.
pub(crate) fn health(runner: &dyn Runner, port: u16) -> Result<Health> {
    let output = runner.run(&Cmd::new("curl", HTTP_TIMEOUT).args([
        "-sS",
        "--fail-with-body",
        "--max-time",
        "5",
        &url(port, "/healthz"),
    ]))?;
    if !output.success() {
        bail!(
            "bridge on port {port} did not answer: {}",
            output.error_text()
        );
    }
    let raw: Value = serde_json::from_str(output.stdout.trim())
        .with_context(|| format!("bridge /healthz on port {port} did not answer JSON"))?;
    let version = raw
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let major = version
        .split('.')
        .next()
        .and_then(|m| m.parse::<u64>().ok())
        .unwrap_or(0);
    let accepting = raw
        .get("accepting_turns")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let pid = raw.get("pid").and_then(Value::as_u64).map(|p| p as u32);
    let mode = raw.get("mode").and_then(Value::as_str).map(str::to_string);
    Ok(Health {
        version,
        major,
        accepting,
        pid,
        mode,
        raw,
    })
}

/// The first port that answers: the launcher's 17841, then the terminal
/// fallback's 17941.
pub(crate) fn health_any(runner: &dyn Runner) -> Result<(u16, Health)> {
    let primary = super::BRIDGE_PORT;
    let fallback = super::FALLBACK_PORT;
    if let Ok(health) = health(runner, primary) {
        return Ok((primary, health));
    }
    let health = health(runner, fallback)
        .map_err(|error| anyhow::anyhow!("no bridge on {primary} or {fallback}: {error:#}"))?;
    Ok((fallback, health))
}

/// The bridge's own config: `$CODEX_CHATGPT_WEB_HOME/config.json` else
/// `~/.codex-chatgpt-web/config.json`. Read only; the token is never printed.
fn bridge_config(env: &Env) -> Option<Value> {
    let path = env.bridge_home().join("config.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The control token the two `/admin` calls need.
fn control_token(env: &Env) -> Option<String> {
    bridge_config(env)?
        .get("controlToken")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// The bridge's configured mode when its config is readable.
pub(crate) fn config_mode(env: &Env) -> Option<String> {
    bridge_config(env)?
        .get("mode")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `POST /admin/drain` with the control token.
pub(crate) fn drain(runner: &dyn Runner, env: &Env, port: u16) -> Result<String> {
    admin(runner, env, port, "/admin/drain")
}

/// `POST /admin/resume`; Rolf's clear of the breaker.
pub(crate) fn resume(runner: &dyn Runner, env: &Env, port: u16) -> Result<String> {
    admin(runner, env, port, "/admin/resume")
}

fn admin(runner: &dyn Runner, env: &Env, port: u16, path: &str) -> Result<String> {
    let token = control_token(env).context(
        "no bridge control token; the bridge config is missing, so drain/resume was not sent",
    )?;
    let auth = format!("Authorization: Bearer {token}");
    let output = runner.run(&Cmd::new("curl", HTTP_TIMEOUT).args([
        "-sS",
        "--fail-with-body",
        "--max-time",
        "5",
        "-X",
        "POST",
        "-H",
        &auth,
        &url(port, path),
    ]))?;
    if !output.success() {
        bail!("bridge {path} failed: {}", output.error_text());
    }
    let body: Value = serde_json::from_str(output.stdout.trim())
        .with_context(|| format!("bridge {path} did not answer JSON"))?;
    let expected = path == "/admin/resume";
    if body.get("status").and_then(Value::as_str) != Some("ok")
        || body.get("accepting_turns").and_then(Value::as_bool) != Some(expected)
    {
        bail!("bridge {path} did not reach the requested state");
    }
    Ok(output.stdout.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pro::sh::fake::{FakeRunner, fail, ok};

    #[test]
    fn health_parses_the_launcher_body() {
        let runner = FakeRunner::new();
        runner.on(
            "/healthz",
            ok(r#"{"status":"ok","service":"codex-chatgpt-web","version":"5.0.8","mode":"browser-only","pid":42,"accepting_turns":true}"#),
        );
        let health = health(&runner, 17841).unwrap();
        assert_eq!(health.version, "5.0.8");
        assert_eq!(health.major, 5);
        assert!(health.major_seen());
        assert!(health.accepting);
        assert_eq!(health.pid, Some(42));
        assert_eq!(health.mode.as_deref(), Some("browser-only"));
    }

    #[test]
    fn an_unseen_major_is_refused_and_a_down_bridge_errors() {
        let runner = FakeRunner::new();
        runner.on(
            "/healthz",
            ok(r#"{"version":"6.0.0","accepting_turns":true}"#),
        );
        assert!(!health(&runner, 17841).unwrap().major_seen());
        let runner = FakeRunner::new();
        runner.on("/healthz", fail(7, "connection refused"));
        assert!(health(&runner, 17841).is_err());
    }

    #[test]
    fn health_any_falls_back_to_the_terminal_port() {
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.display().contains(":17841/healthz"),
            |_| Ok(fail(7, "refused")),
        );
        runner.on_fn(
            |cmd| cmd.display().contains(":17941/healthz"),
            |_| Ok(ok(r#"{"version":"5.0.8","accepting_turns":true}"#)),
        );
        let (port, health) = health_any(&runner).unwrap();
        assert_eq!(port, 17941);
        assert_eq!(health.version, "5.0.8");
    }

    #[test]
    fn the_control_token_comes_from_the_bridge_config_and_is_never_echoed() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path(), &[]);
        let home = dir.path().join(".codex-chatgpt-web");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.json"),
            r#"{"mode":"browser-only","controlToken":"secret-token"}"#,
        )
        .unwrap();
        assert_eq!(control_token(&env).as_deref(), Some("secret-token"));
        assert_eq!(config_mode(&env).as_deref(), Some("browser-only"));
        let runner = FakeRunner::new();
        runner.on(
            "/admin/drain",
            ok(r#"{"status":"ok","accepting_turns":false}"#),
        );
        let body = drain(&runner, &env, 17841).unwrap();
        assert!(body.contains("accepting_turns"));
        let call = &runner.calls.borrow()[0];
        assert!(call.env.is_empty());
        assert!(
            call.display()
                .contains("Authorization: Bearer secret-token")
        );
        assert!(!call.args.iter().any(|a| a == "secret-token"));
    }

    #[test]
    fn drain_without_a_config_is_a_named_error() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path(), &[]);
        let runner = FakeRunner::new();
        assert!(drain(&runner, &env, 17841).is_err());
    }
}
