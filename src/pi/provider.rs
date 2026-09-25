//! The DeepSeek compaction window (SPEC-pi v2 §3.3).
//!
//! pi auto-compacts when `contextTokens > contextWindow - reserveTokens`
//! (docs/compaction.md). `reserveTokens` is global in `settings.json` (pi's
//! default 16384), so the per-model lever is `contextWindow`. The
//! `opencode-go` DeepSeek rows report a 1M window; `herdr-pi setup` lowers it
//! so a DeepSeek lane compacts near 372k tokens (Rolf, 2026-09-19 evening).
//! The merge keeps every other provider, override and key.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value, json};

/// The provider that serves the DeepSeek rows.
pub(crate) const PROVIDER_ID: &str = "opencode-go";

/// The context size a DeepSeek lane should compact at (Rolf, 2026-09-19).
const DEEPSEEK_COMPACT_AT: u64 = 372_000;

/// pi's default `compaction.reserveTokens` (docs/compaction.md).
const PI_RESERVE_TOKENS: u64 = 16_384;

/// The `contextWindow` that makes pi compact at [`DEEPSEEK_COMPACT_AT`].
pub(crate) const DEEPSEEK_CONTEXT_WINDOW: u64 = DEEPSEEK_COMPACT_AT + PI_RESERVE_TOKENS;

/// Merge the DeepSeek `contextWindow` overrides into `models.json`, keeping
/// every other provider, overrides and keys. A missing or unreadable file
/// starts from the empty table. Idempotent.
pub(crate) fn write_overrides(path: &Path, deepseek_models: &[String]) -> Result<()> {
    let mut root: Value = match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text)
            .with_context(|| format!("{} does not parse", path.display()))?,
        _ => json!({"providers": {}}),
    };
    if !root.is_object() {
        root = json!({"providers": {}});
    }
    if root.get("providers").and_then(Value::as_object).is_none() {
        root["providers"] = json!({});
    }
    let provider = &mut root["providers"][PROVIDER_ID];
    if !provider.is_object() {
        *provider = json!({});
    }
    if provider
        .get("modelOverrides")
        .and_then(Value::as_object)
        .is_none()
    {
        provider["modelOverrides"] = json!({});
    }
    for model in deepseek_models {
        let entry = &mut provider["modelOverrides"][model];
        if !entry.is_object() {
            *entry = json!({});
        }
        entry["contextWindow"] = json!(DEEPSEEK_CONTEXT_WINDOW);
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&root).context("could not serialize models.json")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{text}\n"))
        .with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("could not protect {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

/// The DeepSeek recipe models whose `models.json` `contextWindow` is missing
/// or is not [`DEEPSEEK_CONTEXT_WINDOW`]. Doctor prints the names.
pub(crate) fn missing_overrides(path: &Path, deepseek_models: &[String]) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} does not parse", path.display()))?;
    let overrides = value["providers"][PROVIDER_ID]["modelOverrides"].as_object();
    let mut missing = Vec::new();
    for model in deepseek_models {
        let carries = overrides
            .and_then(|map| map.get(model))
            .and_then(|entry| entry.get("contextWindow"))
            .and_then(Value::as_u64)
            == Some(DEEPSEEK_CONTEXT_WINDOW);
        if !carries {
            missing.push(model.clone());
        }
    }
    Ok(missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Vec<String> {
        vec!["deepseek-v4.1-flash".into()]
    }

    #[test]
    fn write_sets_the_override_and_keeps_every_other_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent/models.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{
              "providers": {
                "pro": {"api": "openai-responses", "apiKey": "tok"},
                "opencode-go": {
                  "baseUrl": "https://go",
                  "modelOverrides": {
                    "muse-spark-1.3-contributor": {"contextWindow": 123456},
                    "deepseek-v4.1-flash": {"name": "keep me"}
                  }
                }
              },
              "other": {"key": true}
            }"#,
        )
        .unwrap();
        write_overrides(&path, &models()).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&first).unwrap();
        assert_eq!(
            value["providers"]["opencode-go"]["modelOverrides"]["deepseek-v4.1-flash"]["contextWindow"],
            DEEPSEEK_CONTEXT_WINDOW
        );
        assert_eq!(
            value["providers"]["opencode-go"]["modelOverrides"]["deepseek-v4.1-flash"]["name"],
            "keep me"
        );
        assert_eq!(
            value["providers"]["opencode-go"]["modelOverrides"]["muse-spark-1.3-contributor"]["contextWindow"],
            123456
        );
        assert_eq!(value["providers"]["opencode-go"]["baseUrl"], "https://go");
        assert_eq!(value["providers"]["pro"]["apiKey"], "tok");
        assert_eq!(value["other"]["key"], json!(true));
        write_overrides(&path, &models()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    }

    #[test]
    fn a_missing_file_is_created_with_the_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent/models.json");
        write_overrides(&path, &models()).unwrap();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let overrides = value["providers"]["opencode-go"]["modelOverrides"]
            .as_object()
            .unwrap();
        assert_eq!(overrides.len(), models().len());
    }

    #[test]
    fn missing_overrides_names_a_row_without_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent/models.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{\"providers\":{}}\n").unwrap();
        assert_eq!(missing_overrides(&path, &models()).unwrap(), models());
        write_overrides(&path, &models()).unwrap();
        assert!(missing_overrides(&path, &models()).unwrap().is_empty());
        std::fs::write(
            &path,
            r#"{"providers":{"opencode-go":{"modelOverrides":{"deepseek-v4.1-flash":{"contextWindow":1000}}}}}"#,
        )
        .unwrap();
        assert_eq!(missing_overrides(&path, &models()).unwrap(), models());
    }
}
