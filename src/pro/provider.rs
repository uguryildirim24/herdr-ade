//! The `pro` provider pi talks to (SPEC-pro-bridge relay, step 4).
//!
//! `herdr-pro serve` writes this into the shared pi folder's `models.json`
//! every time it starts, so a lane started after a relay restart reads the
//! relay's current port and token. Setup (`herdr-pi setup`) writes the same
//! row when `serve.json` exists. The merge keeps every other provider.
//!
//! The module is self-contained (only `anyhow` and `serde_json`) because it is
//! also included by `src/pi/install.rs` through a `#[path]` attribute, so it
//! carries no `crate::` or `super::` paths.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value, json};

/// The provider id pi resolves `--provider pro` to.
pub(crate) const PROVIDER_ID: &str = "pro";
/// The single model the relay serves.
pub(crate) const MODEL_ID: &str = "pro";

/// The provider table. An answer-only worker: no reasoning effort, text only.
fn provider_config(base_url: &str, token: &str) -> Value {
    json!({
        "name": "Pro",
        "baseUrl": base_url,
        "apiKey": token,
        "authHeader": true,
        "api": "openai-responses",
        "models": [{
            "id": MODEL_ID,
            "name": "Pro",
            "reasoning": false,
            "input": ["text"],
            "contextWindow": 200000,
            "maxTokens": 128000,
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
            "compat": {"supportsReasoningEffort": false}
        }]
    })
}

/// The exact `baseUrl` the relay serves.
pub(crate) fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

/// Merge the `pro` provider into `models.json`, keeping every other provider.
/// A missing or unreadable file starts from the empty table.
pub(crate) fn write_merged(path: &Path, base_url: &str, token: &str) -> Result<()> {
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
    root["providers"][PROVIDER_ID] = provider_config(base_url, token);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_provider_is_a_responses_worker_with_the_token() {
        let config = provider_config("http://127.0.0.1:1234/v1", "secret");
        assert_eq!(config["api"], "openai-responses");
        assert_eq!(config["authHeader"], true);
        assert_eq!(config["apiKey"], "secret");
        assert_eq!(config["baseUrl"], "http://127.0.0.1:1234/v1");
        assert_eq!(config["models"][0]["id"], "pro");
        assert_eq!(config["models"][0]["reasoning"], false);
        assert_eq!(
            config["models"][0]["compat"]["supportsReasoningEffort"],
            false
        );
    }

    #[test]
    fn merge_keeps_the_other_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent/models.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"providers":{"kimi-coding":{"baseUrl":"https://x"}}}"#,
        )
        .unwrap();
        write_merged(&path, &base_url(7), "tok").unwrap();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["providers"]["kimi-coding"]["baseUrl"], "https://x");
        assert_eq!(value["providers"]["pro"]["apiKey"], "tok");
        assert_eq!(
            value["providers"]["pro"]["baseUrl"],
            "http://127.0.0.1:7/v1"
        );
        // A second write replaces only the pro row.
        write_merged(&path, &base_url(9), "tok2").unwrap();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["providers"]["kimi-coding"]["baseUrl"], "https://x");
        assert_eq!(
            value["providers"]["pro"]["baseUrl"],
            "http://127.0.0.1:9/v1"
        );
    }
}
