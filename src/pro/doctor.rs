//! `herdr-pro doctor` (SPEC-pro-bridge v2, "The plugin's doctor checks").
//!
//! Fail closed, local only, never a model turn. One page; every failing row
//! names what to do.

use anyhow::Result;
use serde_json::Value;

use super::herdr_cli;
use super::sh::{Cmd, Runner, SETUP};
use super::state;
use super::{Env, Layout, bridge};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub level: Level,
    pub label: String,
    pub detail: String,
}

impl Row {
    pub fn ok(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            level: Level::Ok,
            label: label.into(),
            detail: detail.into(),
        }
    }

    pub fn warn(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            level: Level::Warn,
            label: label.into(),
            detail: detail.into(),
        }
    }

    pub fn fail(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            level: Level::Fail,
            label: label.into(),
            detail: detail.into(),
        }
    }

    /// `[ok  ] label: detail`, the plugin's doctor line shape.
    pub fn line(&self) -> String {
        let mark = match self.level {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        format!("[{mark}] {}: {}", self.label, self.detail)
    }
}

pub fn healthy(rows: &[Row]) -> bool {
    !rows.iter().any(|row| row.level == Level::Fail)
}

pub fn json(rows: &[Row]) -> Value {
    let checks: Vec<Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "check": row.label,
                "ok": row.level != Level::Fail,
                "level": match row.level {
                    Level::Ok => "ok",
                    Level::Warn => "warn",
                    Level::Fail => "fail",
                },
                "detail": row.detail,
            })
        })
        .collect();
    serde_json::json!({ "ok": healthy(rows), "checks": checks })
}

/// Every row, from the process environment.
pub fn doctor_rows(env: &Env, layout: &Layout, runner: &dyn Runner) -> Vec<Row> {
    let mut rows = Vec::new();

    // 1, 2, 6: the bridge answers, is browser-only, and is a seen major.
    match bridge::health_any(runner) {
        Ok((port, health)) => {
            let accepting = if health.accepting {
                "accepting turns"
            } else {
                "draining"
            };
            if health.accepting {
                rows.push(Row::ok(
                    "bridge",
                    format!("{accepting} on port {port}, version {}", health.version),
                ));
            } else {
                rows.push(Row::fail(
                    "bridge",
                    format!("{accepting} on port {port}; resume it before a turn"),
                ));
            }
            if health.major_seen() {
                rows.push(Row::ok(
                    "bridge version",
                    format!("major {} is known", health.major),
                ));
            } else {
                rows.push(Row::fail(
                    "bridge version",
                    format!(
                        "major {} was never seen; update the plugin before a turn",
                        health.major
                    ),
                ));
            }
            match health.mode.clone().or_else(|| bridge::config_mode(env)) {
                Some(mode) if mode == "browser-only" => {
                    rows.push(Row::ok("bridge mode", "browser-only"));
                }
                Some(mode) => rows.push(Row::fail(
                    "bridge mode",
                    format!("{mode}; Pro runs in browser-only (never Full or Zero Risk)"),
                )),
                None => rows.push(Row::fail(
                    "bridge mode",
                    "the bridge did not report its mode and the config was not readable; expected browser-only",
                )),
            }
        }
        Err(error) => {
            rows.push(Row::fail("bridge", format!("{error:#}")));
        }
    }

    // 3: Rolf's daily Codex must not be routed through the bridge.
    rows.push(codex_route_row(env));

    // 4: the Pro home is signed in.
    rows.push(codex_login_row(layout, runner));

    // 5: the breaker.
    let now = jiff::Timestamp::now();
    match state::cooldown_until(layout) {
        Some(until) if until > now => rows.push(Row::warn(
            "cooldown",
            format!("turns refused until {until}; clear with `herdr-pro resume-bridge`"),
        )),
        Some(_) => rows.push(Row::ok("cooldown", "expired")),
        None => rows.push(Row::ok("cooldown", "none")),
    }

    // herdr itself: the same floor the plugin needs for `--parent` on start.
    rows.push(herdr_row(env, runner));

    rows
}

fn codex_route_row(env: &Env) -> Row {
    // This is always Rolf's everyday Codex home, even when Pro uses its own
    // home. The invariant is that daily Codex never inherits the bridge.
    let path = env.codex_home().join("config.toml");
    if !path.exists() {
        return Row::ok("~/.codex route", "no config.toml");
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Row::fail(
            "~/.codex route",
            format!(
                "{} is unreadable, so the route cannot be checked",
                path.display()
            ),
        );
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return Row::fail(
            "~/.codex route",
            format!(
                "{} does not parse, so the route cannot be checked",
                path.display()
            ),
        );
    };
    match table.get("openai_base_url").and_then(|v| v.as_str()) {
        Some(url) if local_bridge_url(url, super::BRIDGE_PORT) => Row::fail(
            "~/.codex route",
            format!(
                "openai_base_url points at the bridge ({url}); run `codex-chatgpt-web route disconnect`"
            ),
        ),
        Some(url) if local_bridge_url(url, super::FALLBACK_PORT) => Row::fail(
            "~/.codex route",
            format!("openai_base_url points at the bridge fallback ({url}); remove it"),
        ),
        Some(url) => Row::warn(
            "~/.codex route",
            format!("openai_base_url is set to {url}; the bridge expects it unset"),
        ),
        None => Row::ok("~/.codex route", "no openai_base_url override"),
    }
}

fn local_bridge_url(url: &str, port: u16) -> bool {
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|host| url.contains(&format!("{host}:{port}")))
}

fn codex_login_row(layout: &Layout, runner: &dyn Runner) -> Row {
    let home = layout.codex_home();
    match runner.run(
        &Cmd::new("codex", SETUP)
            .args(["login", "status"])
            .env("CODEX_HOME", home.display().to_string()),
    ) {
        Ok(output) if output.success() => {
            let text = output
                .stdout
                .trim()
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                Row::ok("codex login", format!("ok in {}", home.display()))
            } else {
                Row::ok("codex login", format!("{text} ({})", home.display()))
            }
        }
        Ok(output) => Row::fail(
            "codex login",
            format!(
                "sign_in_required: the Pro home {} is not signed in ({}); run `CODEX_HOME={} codex login`",
                home.display(),
                output.error_text(),
                home.display()
            ),
        ),
        Err(error) => Row::fail("codex login", format!("{error:#}")),
    }
}

fn herdr_row(env: &Env, runner: &dyn Runner) -> Row {
    let bin = env.herdr_bin();
    match herdr_cli::agent_list(runner, &bin) {
        Ok(_) => Row::ok("herdr", format!("{bin} answers `agent list`")),
        Err(error) => Row::fail("herdr", format!("{error:#}")),
    }
}

/// `doctor` and `start` share one gate: any failing row stops a start.
pub fn gate(env: &Env, layout: &Layout, runner: &dyn Runner) -> Result<()> {
    let rows = doctor_rows(env, layout, runner);
    if healthy(&rows) {
        return Ok(());
    }
    let reasons: Vec<String> = rows
        .iter()
        .filter(|row| row.level == Level::Fail)
        .map(|row| format!("{}: {}", row.label, row.detail))
        .collect();
    anyhow::bail!("doctor failed; {}", reasons.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pro::sh::fake::{FakeRunner, fail, ok};

    fn env_with(dir: &std::path::Path) -> Env {
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        Env::for_test(dir, &[("HERDR_BIN_PATH", "/h/herdr")])
    }

    fn scripted(dir: &std::path::Path) -> FakeRunner {
        let runner = FakeRunner::new();
        runner.on(
            ":17841/healthz",
            ok(r#"{"version":"5.0.8","mode":"browser-only","pid":7,"accepting_turns":true}"#),
        );
        runner.on("codex login status", ok("Logged in\n"));
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        std::fs::create_dir_all(dir.join(".codex-chatgpt-web")).unwrap();
        std::fs::write(
            dir.join(".codex-chatgpt-web/config.json"),
            r#"{"mode":"browser-only","controlToken":"t"}"#,
        )
        .unwrap();
        runner
    }

    #[test]
    fn a_healthy_setup_has_no_failures() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with(dir.path());
        let layout = Layout::for_test(dir.path().join("pro"));
        let rows = doctor_rows(&env, &layout, &scripted(dir.path()));
        let text: Vec<String> = rows.iter().map(Row::line).collect();
        assert!(!text.iter().any(|l| l.contains("[FAIL]")), "{text:?}");
        assert!(healthy(&rows));
    }

    #[test]
    fn a_down_bridge_fails_and_the_route_and_login_rows_still_run() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with(dir.path());
        std::fs::write(
            dir.path().join(".codex/config.toml"),
            "openai_base_url = \"http://127.0.0.1:17841/v1\"\n",
        )
        .unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        let runner = FakeRunner::new();
        runner.on("/healthz", fail(7, "connection refused"));
        runner.on("codex login status", fail(1, "not logged in"));
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        let rows = doctor_rows(&env, &layout, &runner);
        let text: Vec<String> = rows.iter().map(Row::line).collect();
        assert!(text.iter().any(|l| l.contains("[FAIL] bridge")), "{text:?}");
        assert!(
            text.iter().any(|l| l.contains("[FAIL] ~/.codex route")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("[FAIL] codex login")),
            "{text:?}"
        );
        assert!(gate(&env, &layout, &runner).is_err());
    }

    #[test]
    fn a_wrong_mode_is_a_failure_and_a_cooldown_is_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with(dir.path());
        std::fs::create_dir_all(dir.path().join(".codex-chatgpt-web")).unwrap();
        std::fs::write(
            dir.path().join(".codex-chatgpt-web/config.json"),
            r#"{"mode":"full","controlToken":"t"}"#,
        )
        .unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        state::set_cooldown(
            &layout,
            jiff::Timestamp::now() + jiff::SignedDuration::from_secs(600),
        )
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            ":17841/healthz",
            ok(r#"{"version":"5.0.8","pid":7,"accepting_turns":true}"#),
        );
        runner.on("codex login status", ok("Logged in\n"));
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        let rows = doctor_rows(&env, &layout, &runner);
        let text: Vec<String> = rows.iter().map(Row::line).collect();
        assert!(
            text.iter().any(|l| l.contains("[FAIL] bridge mode")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("[warn] cooldown")),
            "{text:?}"
        );
        assert!(!healthy(&rows));
    }
}
