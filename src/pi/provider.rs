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
    let original = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    let mut root: Value = match &original {
        Some(text) => serde_json::from_str(text)
            .with_context(|| format!("{} does not parse", path.display()))?,
        None => json!({"providers": {}}),
    };
    let before = root.clone();
    let providers = object_entry(&mut root, "providers")?;
    if deepseek_models.is_empty() {
        // Only this exact declaration came from setup's empty recipe merge.
        // Other provider declarations, including other empty shapes, aren't ours.
        if exactly_empty_provider(&providers[PROVIDER_ID]) {
            providers.as_object_mut().unwrap().remove(PROVIDER_ID);
        }
    } else {
        let provider = object_entry(providers, PROVIDER_ID)?;
        let overrides = object_entry(provider, "modelOverrides")?;
        for model in deepseek_models {
            let entry = object_entry(overrides, model)?;
            entry["contextWindow"] = json!(DEEPSEEK_CONTEXT_WINDOW);
        }
    }
    if original.is_some() && root == before {
        return Ok(());
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
    }
    let text = match original {
        Some(text) => preserve_other_providers(&text, &root)?,
        None => format!("{}\n", serde_json::to_string_pretty(&root)?),
    };
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("could not protect {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

fn exactly_empty_provider(value: &Value) -> bool {
    value == &json!({"modelOverrides": {}})
}

/// Edit only the owned provider, retaining every other declaration's bytes,
/// including its key, whitespace, order and numeric spellings.
fn preserve_other_providers(original: &str, root: &Value) -> Result<String> {
    use serde_json::value::RawValue;
    use std::collections::BTreeMap;

    let fields: BTreeMap<String, &RawValue> = serde_json::from_str(original)?;
    let Some(table) = fields.get("providers") else {
        return Ok(format!("{}\n", serde_json::to_string_pretty(root)?));
    };
    let old: BTreeMap<String, &RawValue> = serde_json::from_str(table.get())?;
    // Borrowed RawValues point into the original input, not reserialized JSON.
    let start = |raw: &RawValue| raw.get().as_ptr() as usize - original.as_ptr() as usize;
    let end = |raw: &RawValue| start(raw) + raw.get().len();
    let mut text = original.to_string();
    match (old.get(PROVIDER_ID), root["providers"].get(PROVIDER_ID)) {
        (Some(raw), Some(value)) => {
            text.replace_range(start(raw)..end(raw), &serde_json::to_string_pretty(value)?);
        }
        (Some(raw), None) => {
            let previous = old
                .values()
                .map(|v| end(v))
                .filter(|e| *e < start(raw))
                .max();
            let from = previous.unwrap_or_else(|| start(table) + 1);
            // Remove the preceding comma, or the following comma for the first
            // member. Whitespace and bytes of the remaining members stay put.
            let to = if previous.is_none() && old.len() > 1 {
                end(raw) + original[end(raw)..end(table)].find(',').unwrap() + 1
            } else {
                end(raw)
            };
            text.replace_range(from..to, "");
        }
        (None, Some(value)) => {
            let at = end(table) - 1;
            text.insert_str(
                at,
                &format!(
                    "{}{}: {}",
                    if old.is_empty() { "" } else { "," },
                    serde_json::to_string(PROVIDER_ID)?,
                    serde_json::to_string_pretty(value)?
                ),
            );
        }
        (None, None) => {}
    }
    Ok(text)
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
    ensure!(
        !exactly_empty_provider(&value["providers"][PROVIDER_ID]),
        "{} contains an exactly-empty `{PROVIDER_ID}` provider declaration rejected by pi; run `herdr-pi setup`",
        path.display()
    );
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
    fn no_models_writes_no_provider_and_only_removes_the_owned_empty_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        write_overrides(&path, &[]).unwrap();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value, json!({"providers": {}}));
        assert!(missing_overrides(&path, &[]).unwrap().is_empty());
        let custom = r#"{ "apiKey" : "keep", "models" : [ ], "sentinel" : 1e3 }"#;
        let custom_member = format!(r#""custom" : {custom}"#);
        let owned = r#""opencode-go" : {"modelOverrides": {}}"#;
        for members in [
            owned.to_string(),
            format!("{owned}, {custom_member}"),
            format!("{custom_member}, {owned}"),
            format!("{custom_member}, {owned}, \"another\" : {{\"keep\":true}}"),
        ] {
            let original = format!(r#"{{"providers":{{{members}}},"sentinel":true}}"#);
            std::fs::write(&path, &original).unwrap();
            assert!(
                missing_overrides(&path, &[])
                    .unwrap_err()
                    .to_string()
                    .contains("herdr-pi setup")
            );
            write_overrides(&path, &[]).unwrap();
            let cleaned = std::fs::read_to_string(&path).unwrap();
            if members.contains("custom") {
                assert!(cleaned.contains(&custom_member), "{cleaned}");
            }
            if members.contains("another") {
                assert!(
                    cleaned.contains(r#""another" : {"keep":true}"#),
                    "{cleaned}"
                );
            }
            let value: Value = serde_json::from_str(&cleaned).unwrap();
            assert!(value["providers"].get(PROVIDER_ID).is_none());
            assert_eq!(value["sentinel"], true);
            assert!(missing_overrides(&path, &[]).unwrap().is_empty());
        }
        for declaration in [
            r#"{}"#,
            r#"{"modelOverrides":{},"baseUrl":"keep"}"#,
            r#"{"modelOverrides":{"deepseek-custom":{"contextWindow":123}}}"#,
            r#"{"models":[]}"#,
        ] {
            let original = format!(
                r#"{{ "providers" : {{"opencode-go" : {declaration}, "custom" : {custom}}} }}"#
            );
            std::fs::write(&path, &original).unwrap();
            write_overrides(&path, &[]).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert!(missing_overrides(&path, &[]).unwrap().is_empty());
        }
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
