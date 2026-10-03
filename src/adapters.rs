//! Agent harness declarations.
//!
//! The engine treats an agent kind as data. Built-in rows describe the
//! harnesses shipped with ADE and `[adapters.<kind>]` rows may replace or add
//! declarations without changing dispatch, hooks, or doctor code.

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

fn hook(shape: &str, path: &str, events: &[&str], prompt: &str) -> HookAdapter {
    HookAdapter {
        shape: shape.into(),
        path: path.into(),
        events: events.iter().map(|value| (*value).into()).collect(),
        prompt_event: prompt.into(),
    }
}

fn native(binary: &str, hook: HookAdapter, doctor: &[&str]) -> Adapter {
    Adapter {
        binary: binary.into(),
        ready_timeout_ms: 30_000,
        coordinator: true,
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
    // Claude can report an interactive startup block immediately and still
    // become ready without intervention. Keep its observation window long
    // enough for that path instead of inheriting the command driver's 30s.
    claude.ready_timeout_ms = 300_000;
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
        hook("claude", ".codex/hooks.json", &["Stop"], ""),
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
    rows.insert("codex".into(), codex);

    let mut cursor = native(
        "cursor-agent",
        hook(
            "cursor",
            ".cursor/hooks.json",
            &["afterAgentResponse", "stop"],
            "",
        ),
        &["{args}", "-p", "Reply only OK."],
    );
    cursor.required_flags.push("--force".into());
    cursor.coordinator = false;
    rows.insert("cursor".into(), cursor);

    let mut agy = native(
        "agy",
        hook(
            "claude",
            ".agy/hooks.json",
            &["Stop", "UserPromptSubmit"],
            "UserPromptSubmit",
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
            blocked_error_resumable: true,
            hook: hook(
                "pi",
                ".pi/herdr-ade-hooks.json",
                &["Stop", "UserPromptSubmit"],
                "UserPromptSubmit",
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

/// Resume spelling belongs to the harness boundary, not lane placement.
pub(crate) fn resume_args(
    launch: &crate::contracts::Launch,
    session: Option<&str>,
) -> Option<Vec<String>> {
    let flag = match launch.kind.as_str() {
        "pi" => "--session",
        "claude" => "--resume",
        "codex" => "resume",
        _ => return None,
    };
    let mut args = launch.args.clone();
    args.extend([flag.to_string(), session?.to_string()]);
    Some(args)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_claude_recipe_disallows_agent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[routing]\ndefault = 'claude_fable_xhigh'\n",
        )
        .unwrap();
        let config = crate::launch::parse_launch_config(dir.path()).unwrap();
        let mut claude_rows = 0;
        for (id, recipe) in &config.recipes {
            if recipe.kind == "claude" {
                claude_rows += 1;
                let args = launch_args(&config.adapters["claude"], recipe);
                assert!(
                    args.windows(2)
                        .any(|pair| pair == ["--disallowedTools", "Agent"]),
                    "{id}: {args:?}"
                );
            }
        }
        assert_eq!(claude_rows, 2);
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
