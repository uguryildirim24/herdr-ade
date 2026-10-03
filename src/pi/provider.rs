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

use anyhow::{Context, Result, ensure};
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
/// every other provider, overrides and keys. Only a missing file starts
/// from the empty table. Read or shape errors leave the file untouched.
pub(crate) fn write_overrides(path: &Path, deepseek_models: &[String]) -> Result<()> {
    let mut root: Value = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} does not parse", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({"providers": {}}),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    let providers = object_entry(&mut root, "providers")?;
    let provider = object_entry(providers, PROVIDER_ID)?;
    let overrides = object_entry(provider, "modelOverrides")?;
    for model in deepseek_models {
        let entry = object_entry(overrides, model)?;
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

/// Create absent objects, but never replace a configuration value of another shape.
fn object_entry<'a>(parent: &'a mut Value, key: &str) -> Result<&'a mut Value> {
    let object = parent
        .as_object_mut()
        .context("models.json configuration must be an object")?;
    let value = object.entry(key).or_insert_with(|| json!({}));
    ensure!(value.is_object(), "models.json `{key}` must be an object");
    Ok(value)
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
    fn read_and_shape_errors_leave_models_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        // Mode 000 reproduces the original read failure without relying on /dev.
        let original = r#"{"providers":{"custom":{"apiKey":"keep"}},"sentinel":true}"#;
        std::fs::write(&path, original).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(write_overrides(&path, &models()).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        for invalid in [
            "",
            "[]",
            r#"{"providers":null}"#,
            r#"{"providers":{"opencode-go":[]},"sentinel":true}"#,
            r#"{"providers":{"opencode-go":{"modelOverrides":false}}}"#,
            r#"{"providers":{"opencode-go":{"modelOverrides":{"deepseek-v4.1-flash":null}}}}"#,
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(write_overrides(&path, &models()).is_err(), "{invalid}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
        }
        std::fs::remove_file(&path).unwrap();
        write_overrides(&path, &models()).unwrap();
        assert!(missing_overrides(&path, &models()).unwrap().is_empty());
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
}
