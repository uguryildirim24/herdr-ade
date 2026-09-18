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

/// Pi's session flags with their arity, for the fork's
/// `strip_session_picking_args` on r2 (SPEC-pi v2 §3.8, §4 item 2). That
/// function treats `-c`, `--continue`, `-r` and `--resume` as taking a value
/// when the next word has no dash; for pi they are booleans, and eating the
/// next word would drop a message. `true` = the flag takes a value.
pub const R2_STRIP_RULES: [(&str, bool); 7] = [
    ("--session", true),
    ("--fork", true),
    ("--no-session", false),
    ("-c", false),
    ("--continue", false),
    ("-r", false),
    ("--resume", false),
];

/// The session path the herdr extension reported, from a `pane get` answer
/// (`result.pane.agent_session.value`); `None` means the pane has not
/// reported one yet.
pub fn session_from_pane_get(json: &str) -> Result<Option<String>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    Ok(value
        .pointer("/result/pane/agent_session/value")
        .and_then(|v| v.as_str())
        .filter(|path| !path.is_empty())
        .map(str::to_string))
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
    fn pane_get_session_path_is_read_from_the_pane() {
        let pane = r#"{"result":{"pane":{"agent_session":{"source":"herdr:pi","agent":"pi","kind":"path","value":"/s/a.jsonl"}}}}"#;
        assert_eq!(
            session_from_pane_get(pane).unwrap().as_deref(),
            Some("/s/a.jsonl")
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
        for (flag, _) in R2_STRIP_RULES {
            assert!(
                strip.contains(&flag),
                "{flag} is in the rules but not the list"
            );
        }
        let takes_value = |flag: &str| R2_STRIP_RULES.iter().find(|(f, _)| *f == flag).map(|r| r.1);
        assert_eq!(takes_value("--session"), Some(true));
        assert_eq!(takes_value("--fork"), Some(true));
        assert_eq!(takes_value("-c"), Some(false));
        assert_eq!(takes_value("--no-session"), Some(false));
        assert_eq!(takes_value("-s"), None);
    }
}
