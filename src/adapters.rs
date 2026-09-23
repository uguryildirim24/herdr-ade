//! Agent harness declarations.
//!
//! The engine treats an agent kind as data. Built-in rows describe the
//! harnesses shipped with ADE and `[adapters.<kind>]` rows may replace or add
//! declarations without changing dispatch, hooks, talk, or doctor code.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::Recipe;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct HookAdapter {
    /// `claude`, `cursor`, `pi`, or `none`: the on-disk hook JSON grammar.
    pub(crate) shape: String,
    /// Project-relative settings file.
    pub(crate) path: String,
    pub(crate) events: Vec<String>,
    pub(crate) prompt_event: String,
    /// `block`, `followup`, or `none`.
    pub(crate) block: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DoctorAdapter {
    /// `command` runs the declared binary; `pi` uses the provider bridge.
    pub(crate) readiness: String,
    /// Argument template. `{args}` expands to the selected recipe's args.
    pub(crate) args: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Adapter {
    pub(crate) binary: String,
    /// Flags appended to every launch of this kind.
    pub(crate) launch_flags: Vec<String>,
    pub(crate) ready_timeout_ms: u64,
    pub(crate) coordinator: bool,
    pub(crate) talk: bool,
    /// A blocked process with a recorded lane error resumes when prompted.
    pub(crate) blocked_error_resumable: bool,
    pub(crate) capabilities: Vec<String>,
    pub(crate) required_flags: Vec<String>,
    pub(crate) efforts: Vec<String>,
    pub(crate) hook: HookAdapter,
    pub(crate) doctor: DoctorAdapter,
}

pub(crate) fn declarations(config_dir: &Path) -> Result<BTreeMap<String, Adapter>> {
    let document = crate::config::Document::read(config_dir)?;
    declarations_from(&document)
}

pub(crate) fn declarations_from(
    document: &crate::config::Document,
) -> Result<BTreeMap<String, Adapter>> {
    let configured: BTreeMap<String, Adapter> = document.section("adapters")?;
    let mut rows = builtin();
    rows.extend(configured);
    for (kind, row) in &rows {
        validate(kind, row)?;
    }
    Ok(rows)
}

pub(crate) fn declaration(config_dir: &Path, kind: &str) -> Result<Adapter> {
    declarations(config_dir)?
        .remove(kind)
        .with_context(|| format!("adapter_unknown: no adapter declares agent kind `{kind}`"))
}

fn validate(kind: &str, row: &Adapter) -> Result<()> {
    if kind.trim().is_empty() || row.binary.trim().is_empty() {
        bail!("adapter_invalid: `{kind}` needs a binary");
    }
    if row.coordinator && !row.talk {
        bail!("adapter_invalid: coordinator adapter `{kind}` must support talk");
    }
    if row.coordinator
        && (row.hook.prompt_event.is_empty() || !row.hook.events.contains(&row.hook.prompt_event))
    {
        bail!(
            "adapter_invalid: coordinator adapter `{kind}` needs a prompt-submit hook so Rolf's typed messages are recorded"
        );
    }
    if !matches!(row.hook.shape.as_str(), "none" | "claude" | "cursor" | "pi") {
        bail!(
            "adapter_invalid: `{kind}` has unknown hook shape `{}`",
            row.hook.shape
        );
    }
    if row.talk
        && (row.hook.shape == "none" || row.hook.path.is_empty() || row.hook.events.is_empty())
    {
        bail!("adapter_invalid: talk adapter `{kind}` needs a hook path and events");
    }
    if !matches!(row.hook.block.as_str(), "none" | "block" | "followup") {
        bail!(
            "adapter_invalid: `{kind}` has unknown block response `{}`",
            row.hook.block
        );
    }
    if !matches!(row.doctor.readiness.as_str(), "command" | "pi") {
        bail!(
            "adapter_invalid: `{kind}` has unknown readiness driver `{}`",
            row.doctor.readiness
        );
    }
    if row.doctor.readiness == "command" && row.doctor.args.is_empty() {
        bail!("adapter_invalid: command adapter `{kind}` needs doctor arguments");
    }
    Ok(())
}

fn hook(shape: &str, path: &str, events: &[&str], prompt: &str, block: &str) -> HookAdapter {
    HookAdapter {
        shape: shape.into(),
        path: path.into(),
        events: events.iter().map(|value| (*value).into()).collect(),
        prompt_event: prompt.into(),
        block: block.into(),
    }
}

fn native(binary: &str, hook: HookAdapter, doctor: &[&str]) -> Adapter {
    Adapter {
        binary: binary.into(),
        ready_timeout_ms: 30_000,
        coordinator: true,
        talk: true,
        hook,
        doctor: DoctorAdapter {
            readiness: "command".into(),
            args: doctor.iter().map(|value| (*value).into()).collect(),
        },
        ..Adapter::default()
    }
}

fn builtin() -> BTreeMap<String, Adapter> {
    let mut rows = BTreeMap::new();
    let mut claude = native(
        "claude",
        hook(
            "claude",
            ".claude/settings.local.json",
            &["Stop", "UserPromptSubmit"],
            "UserPromptSubmit",
            "block",
        ),
        &[
            "{args}",
            "-p",
            "Reply only OK.",
            "--tools",
            "",
            "--no-session-persistence",
        ],
    );
    claude
        .required_flags
        .push("--dangerously-skip-permissions".into());
    claude.efforts = ["low", "medium", "high", "xhigh", "max"]
        .into_iter()
        .map(str::to_string)
        .collect();
    claude.capabilities.push("native-chat".into());
    rows.insert("claude".into(), claude);

    let mut codex = native(
        "codex",
        hook("claude", ".codex/hooks.json", &["Stop"], "", "block"),
        &[
            "exec",
            "{args}",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "Reply only OK.",
        ],
    );
    // These installed hook grammars expose no prompt-submit event. They may
    // run lanes, but claiming coordinator support would silently lose text
    // typed directly into their panes.
    codex.coordinator = false;
    codex.talk = false;
    rows.insert("codex".into(), codex);

    let mut cursor = native(
        "cursor-agent",
        hook(
            "cursor",
            ".cursor/hooks.json",
            &["afterAgentResponse", "stop"],
            "",
            "followup",
        ),
        &["{args}", "-p", "Reply only OK."],
    );
    cursor.required_flags.push("--force".into());
    cursor.coordinator = false;
    cursor.talk = false;
    rows.insert("cursor".into(), cursor);

    let mut agy = native(
        "agy",
        hook(
            "claude",
            ".agy/hooks.json",
            &["Stop", "UserPromptSubmit"],
            "UserPromptSubmit",
            "block",
        ),
        &["{args}", "-p", "Reply only OK.", "--print-timeout", "60s"],
    );
    agy.required_flags
        .push("--dangerously-skip-permissions".into());
    // Agy stores workspace trust per exact folder. Every lane gets a fresh
    // worktree, so declare it as a new project instead of leaving the agent at
    // the interactive "trust this folder" screen before ADE can send its
    // brief.
    agy.launch_flags.push("--new-project".into());
    agy.efforts = ["low", "medium", "high"]
        .into_iter()
        .map(str::to_string)
        .collect();
    rows.insert("agy".into(), agy);

    rows.insert(
        "pi".into(),
        Adapter {
            binary: "pi".into(),
            ready_timeout_ms: 30_000,
            coordinator: true,
            talk: true,
            blocked_error_resumable: true,
            hook: hook(
                "pi",
                ".pi/herdr-ade-hooks.json",
                &["Stop", "UserPromptSubmit"],
                "UserPromptSubmit",
                "block",
            ),
            doctor: DoctorAdapter {
                readiness: "pi".into(),
                ..DoctorAdapter::default()
            },
            ..Adapter::default()
        },
    );
    rows
}

pub(crate) fn launch_args(adapter: &Adapter, recipe: &Recipe) -> Vec<String> {
    let mut args = recipe.args.clone();
    for flag in &adapter.launch_flags {
        if !args.contains(flag) {
            args.push(flag.clone());
        }
    }
    args
}

pub(crate) fn validate_recipe(adapter: &Adapter, id: &str, recipe: &Recipe) -> Result<()> {
    for flag in &adapter.required_flags {
        if !recipe.args.contains(flag) && !adapter.launch_flags.contains(flag) {
            bail!("recipe_permission_missing: `{id}` has no permission flag `{flag}`");
        }
    }
    for capability in &recipe.capabilities {
        if !adapter.capabilities.contains(capability) {
            bail!(
                "recipe_capability_unknown: `{id}` names `{capability}`, but its adapter does not declare it"
            );
        }
    }
    if !adapter.efforts.is_empty()
        && let Some(index) = recipe.args.iter().position(|arg| arg == "--effort")
        && let Some(effort) = recipe.args.get(index + 1)
        && !adapter.efforts.contains(effort)
    {
        bail!("recipe_effort_unknown: `{id}` names effort {effort:?}");
    }
    // Provider-specific syntax belongs to its declared readiness driver, not
    // to dispatch or to an agent-kind name.
    if adapter.doctor.readiness == "pi" {
        crate::pi::launch::validate_recipe(id, &recipe.provider, &recipe.args, &recipe.env)?;
    }
    Ok(())
}

pub(crate) fn settings_path(project_dir: &Path, adapter: &Adapter) -> Option<PathBuf> {
    (adapter.hook.shape != "none" && !adapter.hook.path.is_empty())
        .then(|| project_dir.join(&adapter.hook.path))
}

pub(crate) fn correction(adapter: &Adapter, reason: &str) -> Option<serde_json::Value> {
    match adapter.hook.block.as_str() {
        "block" => Some(serde_json::json!({ "decision": "block", "reason": reason })),
        "followup" => Some(serde_json::json!({ "followup_message": reason })),
        _ => None,
    }
}

pub(crate) fn capability_label(project: &crate::project::Project, kind: &str) -> &'static str {
    let qualified = project
        .state_dir()
        .join("capabilities")
        .join(format!("{kind}.qualified"))
        .is_file();
    if qualified {
        "surface mediated; native chat checked after display"
    } else {
        "capability: unqualified; chat: shown only through say and ask"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_only_adapter_is_a_complete_declaration() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"[adapters.acme]
binary = "acme-agent"
coordinator = true
talk = true
launch_flags = ["--yes"]
capabilities = ["pictures"]
doctor.readiness = "command"
doctor.args = ["check", "{args}"]
hook.shape = "claude"
hook.path = ".acme/hooks.json"
hook.events = ["Stop", "UserPromptSubmit"]
hook.prompt_event = "UserPromptSubmit"
hook.block = "block"
"#,
        )
        .unwrap();
        let row = declaration(dir.path(), "acme").unwrap();
        assert_eq!(row.binary, "acme-agent");
        assert_eq!(launch_args(&row, &Recipe::default()), ["--yes"]);
    }

    #[test]
    fn agy_starts_each_fresh_lane_as_a_new_project() {
        let row = builtin().remove("agy").unwrap();
        assert_eq!(
            row.doctor.args,
            ["{args}", "-p", "Reply only OK.", "--print-timeout", "60s"]
        );
        assert!(!row.doctor.args.iter().any(|arg| arg == "--max-turns"));
        assert!(!row.doctor.args.iter().any(|arg| arg == "--tools"));

        let args = launch_args(
            &row,
            &Recipe {
                args: vec!["--dangerously-skip-permissions".into()],
                ..Recipe::default()
            },
        );
        assert_eq!(args, ["--dangerously-skip-permissions", "--new-project"]);
    }
}
