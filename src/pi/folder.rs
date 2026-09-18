//! The one shared pi folder: `settings.json`, `models.json`, `trust.json`,
//! `extensions/`, `sessions/` (SPEC-pi v2 §3.3).
//!
//! Nothing here is per-lane. Lanes are separated by their worktrees, so pi
//! already writes each session under its own `sessions/--<cwd>--/` folder.
//! The file holds no secrets: `auth.json` is written only by pi's `/login`.

use anyhow::{Context, Result};
use serde_json::Value;

use super::{GUARD_FILE, HERDR_EXTENSION_FILE, Layout};

/// Exactly SPEC-pi v2 §3.3: no trust dialog, no personal skills, one retry.
pub const SETTINGS_JSON: &str = r#"{
  "defaultProjectTrust": "never",
  "enableInstallTelemetry": false,
  "quietStartup": true,
  "skills": { "enabled": false },
  "retry": {
    "enabled": true,
    "maxRetries": 1,
    "provider": { "maxRetries": 0, "maxRetryDelayMs": 60000 }
  }
}
"#;

/// No custom providers this round: Pro stays the pro-bridge Codex worker and
/// Cursor stays outside pi (decisions 18:05, 18:30).
pub const MODELS_JSON: &str = "{\n  \"providers\": {}\n}\n";

/// What setup created; printed by `herdr-pi setup`.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderReport {
    pub created_dirs: Vec<std::path::PathBuf>,
    pub wrote_settings: bool,
    pub wrote_models: bool,
    pub wrote_trust: bool,
}

/// Create the shared folder and write the plugin-owned files. Idempotent:
/// an existing `settings.json` is never clobbered (settings drift is
/// cooperative, and doctor reports it), and `auth.json` is never written.
pub fn ensure(layout: &Layout) -> Result<FolderReport> {
    let mut report = FolderReport {
        created_dirs: Vec::new(),
        wrote_settings: false,
        wrote_models: false,
        wrote_trust: false,
    };
    for dir in [
        layout.root.clone(),
        layout.npm(),
        layout.root.join("bin"),
        layout.agent(),
        layout.extensions(),
        layout.sessions(),
        layout.lanes(),
    ] {
        if !dir.is_dir() {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("could not create {}", dir.display()))?;
            report.created_dirs.push(dir);
        }
    }

    if !layout.settings().exists() {
        std::fs::write(layout.settings(), SETTINGS_JSON)
            .with_context(|| format!("could not write {}", layout.settings().display()))?;
        report.wrote_settings = true;
    }
    if !layout.models().exists() {
        std::fs::write(layout.models(), MODELS_JSON)
            .with_context(|| format!("could not write {}", layout.models().display()))?;
        report.wrote_models = true;
    }
    // `trust.json` stays empty; doctor fails on any `true` entry.
    if !layout.trust().exists() {
        std::fs::write(layout.trust(), "{}\n")
            .with_context(|| format!("could not write {}", layout.trust().display()))?;
        report.wrote_trust = true;
    }
    Ok(report)
}

/// What doctor needs from `settings.json` (SPEC-pi v2 §3.3, §3.9).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsState {
    pub trust_never: bool,
    pub skills_disabled: bool,
    pub telemetry_off: bool,
    pub retries_capped: bool,
}

pub fn read_settings(layout: &Layout) -> Result<SettingsState> {
    let text = std::fs::read_to_string(layout.settings())
        .with_context(|| format!("could not read {}", layout.settings().display()))?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} does not parse", layout.settings().display()))?;
    Ok(SettingsState {
        trust_never: value.get("defaultProjectTrust").and_then(Value::as_str) == Some("never"),
        skills_disabled: value
            .get("skills")
            .and_then(|s| s.get("enabled"))
            .and_then(Value::as_bool)
            == Some(false),
        telemetry_off: value.get("enableInstallTelemetry").and_then(Value::as_bool) == Some(false),
        retries_capped: value
            .get("retry")
            .and_then(|r| r.get("maxRetries"))
            .and_then(Value::as_u64)
            == Some(1)
            && value
                .get("retry")
                .and_then(|r| r.get("provider"))
                .and_then(|p| p.get("maxRetries"))
                .and_then(Value::as_u64)
                == Some(0),
    })
}

/// True when `trust.json` holds a `true` value anywhere. Doctor fails then
/// (SPEC-pi v2 §3.9): a trusted folder executes repository pi code.
pub fn trust_has_true(layout: &Layout) -> Result<bool> {
    let path = layout.trust();
    if !path.exists() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(false);
    }
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} does not parse", path.display()))?;
    Ok(any_true(&value))
}

fn any_true(value: &Value) -> bool {
    match value {
        Value::Bool(found) => *found,
        Value::Array(items) => items.iter().any(any_true),
        Value::Object(map) => map.values().any(any_true),
        _ => false,
    }
}

/// The guard and the herdr state hook, both in the shared folder
/// (SPEC-pi v2 §3.3, §3.7).
pub fn extension_state(layout: &Layout) -> (bool, bool) {
    let guard = std::fs::read_to_string(layout.guard())
        .map(|text| text.contains(super::GUARD_MARKER))
        .unwrap_or(false);
    let herdr = layout.herdr_extension().exists();
    (guard, herdr)
}

/// True when the file names one of the two plugin-managed extensions.
pub fn is_managed_extension(name: &str) -> bool {
    name == GUARD_FILE || name == HERDR_EXTENSION_FILE
}

/// The per-lane session dir for a shared cwd (SPEC-pi v2 §3.3): a static
/// `--session-dir` in the recipe, never computed at launch.
pub fn lane_session_dir(layout: &Layout, thread_id: &str) -> std::path::PathBuf {
    layout.lanes().join(thread_id)
}

/// Sanity used by setup and doctor: the folder exists and has a settings file.
pub fn exists(layout: &Layout) -> bool {
    layout.agent().is_dir() && layout.settings().is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_follow_the_spec_contract() {
        let value: Value = serde_json::from_str(SETTINGS_JSON).unwrap();
        assert_eq!(value["defaultProjectTrust"], "never");
        assert_eq!(value["skills"]["enabled"], false);
        assert_eq!(value["enableInstallTelemetry"], false);
        assert_eq!(value["retry"]["maxRetries"], 1);
        assert_eq!(value["retry"]["provider"]["maxRetries"], 0);
        assert_eq!(value["retry"]["provider"]["maxRetryDelayMs"], 60000);
    }

    #[test]
    fn ensure_writes_the_folder_once_and_never_clobbers_settings() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pi"));
        let first = ensure(&layout).unwrap();
        assert!(first.wrote_settings && first.wrote_models && first.wrote_trust);
        assert!(!layout.auth().exists(), "setup never writes auth.json");
        assert!(exists(&layout));

        std::fs::write(layout.settings(), "{\"defaultProjectTrust\":\"ask\"}").unwrap();
        let second = ensure(&layout).unwrap();
        assert!(!second.wrote_settings);
        let state = read_settings(&layout).unwrap();
        assert!(!state.trust_never);
    }

    #[test]
    fn trust_json_with_any_true_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pi"));
        ensure(&layout).unwrap();
        assert!(!trust_has_true(&layout).unwrap());
        std::fs::write(layout.trust(), "{\"/repo\":true}\n").unwrap();
        assert!(trust_has_true(&layout).unwrap());
        std::fs::write(layout.trust(), "{\"/repo\":{\"/sub\":false}}\n").unwrap();
        assert!(!trust_has_true(&layout).unwrap());
    }

    #[test]
    fn settings_state_reads_every_cap() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pi"));
        std::fs::create_dir_all(layout.agent()).unwrap();
        std::fs::write(layout.settings(), SETTINGS_JSON).unwrap();
        let state = read_settings(&layout).unwrap();
        assert_eq!(
            state,
            SettingsState {
                trust_never: true,
                skills_disabled: true,
                telemetry_off: true,
                retries_capped: true,
            }
        );
    }

    #[test]
    fn lane_session_dir_is_per_thread() {
        let layout = Layout::for_test("/p/pi");
        assert_eq!(
            lane_session_dir(&layout, "t-0007"),
            std::path::PathBuf::from("/p/pi/lanes/t-0007")
        );
    }
}
