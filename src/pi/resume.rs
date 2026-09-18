//! Session resume after a herdr restart (SPEC-pi v2 §3.8).
//!
//! Native restore types exactly `pi --session <path>` into a fresh login
//! shell. The wrapper supplies the shared folder, the herdr extension
//! reloads, and the session file restores provider, model and thinking level
//! (`model_change`, `thinking_level_change`). `--approve` is never replayed.
//!
//! ADE's own thread restart is a new attempt: it reads the reported session
//! path from `pane get` and appends `--session <path>`, recording it as
//! `launch.resume_session` — never inside the recipe's `args`.

use std::path::Path;

use anyhow::{Result, bail};

/// The exact line native restore types (fork-owned; recorded here so doctor
/// and the skill can assert it).
pub fn restore_line(session: &Path) -> String {
    format!("pi --session {}", session.display())
}

/// The argv ADE appends for a new attempt on the same session.
pub fn append_resume_session(session: &Path, recipe_args: &[String]) -> Result<Vec<String>> {
    for arg in recipe_args {
        let flag = arg.split('=').next().unwrap_or(arg);
        if SESSION_PICKING.contains(&flag) {
            bail!(
                "pi_args_forbidden: `{flag}` must stay out of the recipe and live on launch.resume_session"
            );
        }
    }
    Ok(vec!["--session".to_string(), session.display().to_string()])
}

/// Session-picking flags a recipe may not carry; the r2 replay strip list
/// must also drop them for pi (`-c` is a boolean, pi has no `-s`).
pub const SESSION_PICKING: [&str; 9] = [
    "--session",
    "--fork",
    "--no-session",
    "-c",
    "--continue",
    "-r",
    "--resume",
    "--session-id",
    "--fork-session",
];

/// What the reviewer's r2 strip list needs to drop for pi, as exact tokens.
pub fn r2_strip_list() -> Vec<&'static str> {
    SESSION_PICKING.to_vec()
}

/// Parse the session path the herdr extension reported from a `pane get`
/// answer. Accepts the pane shape and the agent shape, tolerantly; `None`
/// means the pane has not reported one yet.
pub fn session_from_pane_get(json: &str) -> Result<Option<String>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    for root in ["pane", "agent"] {
        if let Some(session) = value
            .get("result")
            .and_then(|r| r.get(root))
            .and_then(|a| a.get("agent_session"))
        {
            if let Some(path) = session.get("value").and_then(|v| v.as_str()) {
                if !path.is_empty() {
                    return Ok(Some(path.to_string()));
                }
            }
            if let Some(path) = session.get("agent_session_path").and_then(|v| v.as_str()) {
                if !path.is_empty() {
                    return Ok(Some(path.to_string()));
                }
            }
        }
        if let Some(session) = value
            .get("result")
            .and_then(|r| r.get(root))
            .and_then(|a| a.get("agent_session_path"))
            .and_then(|v| v.as_str())
        {
            if !session.is_empty() {
                return Ok(Some(session.to_string()));
            }
        }
    }
    Ok(None)
}

/// A fresh login shell has no plugin env; the wrapper is on the login `PATH`
/// (`~/.local/bin`), so a bare `pi` finds the shared folder again.
pub fn login_path_is_the_way_back() -> &'static str {
    "the wrapper at ~/.local/bin/pi is what the login PATH must resolve; no per-lane env survives"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_appends_exactly_one_session() {
        let recipe = vec!["--provider".to_string(), "deepseek".to_string()];
        let extra = append_resume_session(Path::new("/s/lane.jsonl"), &recipe).unwrap();
        assert_eq!(extra, vec!["--session", "/s/lane.jsonl"]);
        let bad = vec!["--session".to_string(), "/old".to_string()];
        assert!(
            append_resume_session(Path::new("/s/lane.jsonl"), &bad)
                .unwrap_err()
                .to_string()
                .contains("pi_args_forbidden")
        );
        let continues = vec!["-c".to_string()];
        assert!(append_resume_session(Path::new("/s/lane.jsonl"), &continues).is_err());
    }

    #[test]
    fn restore_line_is_the_bare_pi_session() {
        assert_eq!(
            restore_line(Path::new("/state/pi/agent/sessions/--/x.jsonl")),
            "pi --session /state/pi/agent/sessions/--/x.jsonl"
        );
    }

    #[test]
    fn pane_get_session_path_is_read_from_both_shapes() {
        let pane = r#"{"result":{"pane":{"agent_session":{"source":"herdr:pi","agent":"pi","kind":"path","value":"/s/a.jsonl"}}}}"#;
        assert_eq!(
            session_from_pane_get(pane).unwrap().as_deref(),
            Some("/s/a.jsonl")
        );
        let agent = r#"{"result":{"agent":{"agent_session":{"value":"/s/b.jsonl"}}}}"#;
        assert_eq!(
            session_from_pane_get(agent).unwrap().as_deref(),
            Some("/s/b.jsonl")
        );
        let none = r#"{"result":{"pane":{"pane_id":"w1:p1"}}}"#;
        assert_eq!(session_from_pane_get(none).unwrap(), None);
        assert!(session_from_pane_get("not json").is_err());
    }

    #[test]
    fn r2_strip_list_covers_every_pi_session_flag() {
        let strip = r2_strip_list();
        for flag in ["--session", "--fork", "--no-session", "-c"] {
            assert!(strip.contains(&flag), "missing {flag}");
        }
        assert!(!strip.contains(&"-s"), "pi has no -s");
    }
}
