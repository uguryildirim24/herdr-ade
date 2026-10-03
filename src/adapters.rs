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
    let session = session.filter(|s| !s.trim().is_empty())?;
    let mut args = launch.args.clone();
    args.extend([flag.to_string(), session.to_string()]);
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

/// Evidence emitted by an adapter probe. A reset is an observation, never a
/// polling deadline. Custom adapters can emit this JSON as `ade_dependency`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DependencyEvidence {
    pub(crate) kind: String,
    pub(crate) reset_at: Option<i64>,
}

impl DependencyEvidence {
    fn from_detail(detail: &str) -> Self {
        if let Some(evidence) = detail.lines().find_map(|line| {
            let start = line.find("{\"ade_dependency\":")?;
            serde_json::from_str::<serde_json::Value>(&line[start..])
                .ok()
                .and_then(|value| value.get("ade_dependency").cloned())
                .and_then(|value| serde_json::from_value::<Self>(value).ok())
        }) && ["auth", "quota", "connectivity", "unknown"].contains(&evidence.kind.as_str())
        {
            return Self {
                reset_at: evidence.reset_at.filter(|at| {
                    evidence.kind == "quota" && jiff::Timestamp::from_second(*at).is_ok()
                }),
                kind: evidence.kind,
            };
        }
        let lower = detail.to_ascii_lowercase();
        let kind = if crate::pi::doctor::positive_sign_in_evidence(detail) {
            "auth"
        } else if lower.contains("you've hit your limit")
            || lower.contains("you have hit your limit")
            || lower.contains("usage limit reached")
            || lower.contains("quota exceeded")
            || lower.contains("rate limit exceeded")
        {
            "quota"
        } else if crate::remote::is_unreachable(detail)
            || lower.contains("connection error")
            || lower.contains("service unavailable")
        {
            "connectivity"
        } else {
            "unknown"
        };
        let reset_at = (kind == "quota")
            .then(|| {
                let reset = lower.find("reset")?;
                detail[reset..].split_whitespace().find_map(|token| {
                    token
                        .trim_matches(|c: char| matches!(c, ')' | ',' | '.'))
                        .parse::<jiff::Timestamp>()
                        .ok()
                        .map(|at| at.as_second())
                })
            })
            .flatten();
        Self {
            kind: kind.into(),
            reset_at,
        }
    }
}

/// Only a terminal error, not quoted tool output or old scrollback, can park a
/// live conversational process. Unknown prose is not provider evidence.
pub(crate) fn terminal_dependency(screen: &str) -> Option<(String, DependencyEvidence)> {
    let lines: Vec<_> = screen
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let last = *lines.last()?;
    let line = if last.starts_with(['❯', '›', '>']) && lines.len() > 1 {
        lines[lines.len() - 2]
    } else {
        last
    };
    let lower = line.to_ascii_lowercase();
    if !(lower.starts_with("you've hit your limit")
        || lower.starts_with("you have hit your limit")
        || lower.starts_with("usage limit reached")
        || lower.starts_with("quota exceeded")
        || lower.starts_with("rate limit exceeded")
        || lower.starts_with("authentication failed")
        || lower.starts_with("api error:")
        || lower.starts_with("api error (")
        || lower.starts_with("{\"ade_dependency\":"))
    {
        return None;
    }
    Some((line.into(), DependencyEvidence::from_detail(line)))
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct DependencyWait {
    evidence: DependencyEvidence,
    detail: String,
    checked_at: i64,
    ready: bool,
    ready_for: String,
    auth_notified: bool,
}

/// Models and recipes using the same provider on the same machine share a
/// wait. Native CLIs that hide the provider share their runtime dependency.
pub(crate) fn dependency_key(machine: &str, launch: &crate::contracts::Launch) -> (String, String) {
    let provider = crate::pi::launch::flag_value(&launch.args, "--provider")
        .unwrap_or_else(|| launch.kind.clone());
    (machine.into(), format!("{}:{provider}", launch.kind))
}

fn dependency_path(root: &Path, machine: &str, launch: &crate::contracts::Launch) -> PathBuf {
    let key = dependency_key(machine, launch);
    let hash = crate::thread::sha256_hex(format!("{}\0{}", key.0, key.1).as_bytes());
    root.join(".readiness")
        .join(format!("dependency-{hash}.json"))
}

fn dependency_lock(path: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(path.parent().expect("dependency directory"))?;
    crate::project::lock_file(&path.with_extension("lock"))
}

fn load_dependency(path: &Path) -> Result<DependencyWait> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid dependency wait {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DependencyWait::default()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn dependency_waiting(
    root: &Path,
    machine: &str,
    launch: &crate::contracts::Launch,
) -> bool {
    load_dependency(&dependency_path(root, machine, launch))
        .map(|wait| !wait.ready && !wait.detail.is_empty())
        .unwrap_or(true)
}

pub(crate) fn dependency_failure_class(
    root: &Path,
    machine: &str,
    launch: &crate::contracts::Launch,
) -> crate::contracts::FailureClass {
    match load_dependency(&dependency_path(root, machine, launch))
        .ok()
        .map(|wait| wait.evidence.kind)
        .as_deref()
    {
        Some("auth" | "quota") => crate::contracts::FailureClass::Provider,
        Some("connectivity") => crate::contracts::FailureClass::LostConnection,
        _ => crate::contracts::FailureClass::Unknown,
    }
}

pub(crate) fn reset_pending(root: &Path, machine: &str, launch: &crate::contracts::Launch) -> bool {
    crate::project::read_json::<DependencyWait>(&dependency_path(root, machine, launch))
        .and_then(|wait| wait.evidence.reset_at)
        .is_some_and(|at| at > jiff::Timestamp::now().as_second())
}

/// Serialize recovery probes across lanes, projects and ticker incarnations.
/// The minute is a recheck interval, NOT a claimed provider reset. Success is
/// shared too; no lane needs to rediscover the outage or acquire a new budget.
pub(crate) fn dependency_ready(
    root: &Path,
    machine: &str,
    launch: &crate::contracts::Launch,
    probe: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let path = dependency_path(root, machine, launch);
    let _lock = dependency_lock(&path)?;
    let now = jiff::Timestamp::now().as_second();
    let ready_for = crate::thread::sha256_hex(&serde_json::to_vec(&(
        &launch.kind,
        &launch.args,
        &launch.env,
    ))?);
    let mut wait = load_dependency(&path)?;
    if wait.checked_at != 0
        && (!wait.ready || wait.ready_for == ready_for)
        && (now.saturating_sub(wait.checked_at) < 60
            || wait.evidence.reset_at.is_some_and(|at| at > now))
    {
        if wait.ready {
            return Ok(());
        }
        bail!("{}", wait.detail);
    }
    let result = probe();
    if result.as_ref().err().is_some_and(|error| {
        let detail = format!("{error:#}");
        [
            "disk_low:",
            "version_skew:",
            "protocol_unavailable:",
            "box_repo_",
            "machine_held:",
            "adapter_",
            "pi_args_forbidden:",
        ]
        .iter()
        .any(|local| detail.contains(local))
    }) {
        return result;
    }
    wait.checked_at = jiff::Timestamp::now().as_second();
    wait.ready = result.is_ok();
    match &result {
        Ok(()) => {
            wait.evidence = DependencyEvidence::default();
            wait.ready_for = ready_for;
            wait.detail.clear();
            wait.auth_notified = false;
        }
        Err(error) => {
            wait.detail = format!("{error:#}");
            let mut evidence = DependencyEvidence::from_detail(&wait.detail);
            if evidence.kind == wait.evidence.kind && evidence.reset_at.is_none() {
                evidence.reset_at = wait.evidence.reset_at;
            }
            // A later opaque probe cannot erase a recorded reset or auth fact.
            if evidence.kind != "unknown" || wait.evidence.kind.is_empty() {
                wait.evidence = evidence;
            }
        }
    }
    crate::project::write_json(&path, &wait)?;
    result
}

pub(crate) fn observe_dependency(
    root: &Path,
    machine: &str,
    launch: &crate::contracts::Launch,
    detail: &str,
    evidence: &DependencyEvidence,
) -> Result<()> {
    let path = dependency_path(root, machine, launch);
    let _lock = dependency_lock(&path)?;
    let mut wait = load_dependency(&path)?;
    // Repeated screen observations must not push the recheck into the future.
    if wait.detail.is_empty() || wait.ready {
        wait.checked_at = jiff::Timestamp::now().as_second();
        wait.auth_notified = false;
    }
    wait.ready = false;
    wait.detail = detail.into();
    if evidence.kind != "unknown" || wait.evidence.kind.is_empty() {
        let reset = wait.evidence.reset_at;
        let same_kind = wait.evidence.kind == evidence.kind;
        wait.evidence = evidence.clone();
        if same_kind && wait.evidence.reset_at.is_none() {
            wait.evidence.reset_at = reset;
        }
    }
    crate::project::write_json(&path, &wait)
}

/// A courier receipt proves this machine's link recovered, not its provider.
/// Release only cached machine-unreachable evidence, retaining quota/auth facts.
pub(crate) fn machine_reconnected(
    root: &Path,
    machine: &str,
    launch: &crate::contracts::Launch,
) -> Result<()> {
    let path = dependency_path(root, machine, launch);
    if !path.exists() {
        return Ok(());
    }
    let _lock = dependency_lock(&path)?;
    let mut wait = load_dependency(&path)?;
    if wait.evidence.kind == "connectivity" && crate::remote::is_unreachable(&wait.detail) {
        wait.checked_at = 0;
        crate::project::write_json(&path, &wait)?;
    }
    Ok(())
}

pub(crate) fn notify_auth(
    root: &Path,
    project: &crate::project::Project,
    machine: &str,
    launch: &crate::contracts::Launch,
) -> Result<()> {
    let path = dependency_path(root, machine, launch);
    let _lock = dependency_lock(&path)?;
    let mut wait = load_dependency(&path)?;
    if wait.evidence.kind == "auth" && !wait.auth_notified {
        crate::inbox::write(
            project,
            "dependency-auth",
            &dependency_key(machine, launch).1,
            &format!(
                "Sign in to {} on machine `{machine}`; authentication requires Rolf. {}",
                launch.kind, wait.detail
            ),
            "",
        )?;
        wait.auth_notified = true;
        crate::project::write_json(&path, &wait)?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn expire_dependency_probe(
    root: &Path,
    machine: &str,
    launch: &crate::contracts::Launch,
) {
    let path = dependency_path(root, machine, launch);
    let mut wait = load_dependency(&path).unwrap();
    wait.checked_at = jiff::Timestamp::now().as_second() - 61;
    crate::project::write_json(&path, &wait).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_probes_are_shared_across_recipes_passes_and_concurrent_recovery() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempfile::tempdir().unwrap();
        let launch = crate::contracts::Launch {
            kind: "pi".into(),
            args: vec!["--provider".into(), "same".into()],
            recipe_id: "first-model".into(),
            work_retries: 2,
            same_recipe_retries: 3,
            ..Default::default()
        };
        let probes = AtomicUsize::new(0);
        for recipe in ["first-model", "second-model"] {
            let mut other = launch.clone();
            other.recipe_id = recipe.into();
            assert!(
                dependency_ready(root.path(), "oci", &other, || {
                    probes.fetch_add(1, Ordering::SeqCst);
                    bail!("opaque provider response")
                })
                .is_err()
            );
        }
        assert_eq!(probes.load(Ordering::SeqCst), 1);
        let wait = load_dependency(&dependency_path(root.path(), "oci", &launch)).unwrap();
        assert_eq!(wait.evidence.kind, "unknown");
        assert_eq!(wait.evidence.reset_at, None);
        expire_dependency_probe(root.path(), "oci", &launch);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    dependency_ready(root.path(), "oci", &launch, || {
                        probes.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .unwrap()
                });
            }
        });
        assert_eq!(probes.load(Ordering::SeqCst), 2);
        // A provider wait may be shared, but one model's success cannot prove
        // a different selected model is ready. Equivalent recipes still share.
        let mut other = launch.clone();
        other
            .args
            .extend(["--model".into(), "another-model".into()]);
        dependency_ready(root.path(), "oci", &other, || {
            probes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        other.recipe_id = "equivalent-recipe".into();
        dependency_ready(root.path(), "oci", &other, || {
            bail!("must use the exact cached success")
        })
        .unwrap();
        assert_eq!(probes.load(Ordering::SeqCst), 3);
        assert_eq!((launch.work_retries, launch.same_recipe_retries), (2, 3));
    }

    #[test]
    fn quota_reset_is_evidence_not_a_guessed_polling_deadline() {
        let root = tempfile::tempdir().unwrap();
        let launch = crate::contracts::Launch {
            kind: "custom".into(),
            ..Default::default()
        };
        let detail = r#"probe failed: {"ade_dependency":{"kind":"quota","reset_at":4070908800}}"#;
        assert!(dependency_ready(root.path(), "oci", &launch, || bail!("{detail}")).is_err());
        expire_dependency_probe(root.path(), "oci", &launch);
        assert!(reset_pending(root.path(), "oci", &launch));
        assert!(
            dependency_ready(root.path(), "oci", &launch, || panic!(
                "future reset must park"
            ))
            .is_err()
        );
        observe_dependency(
            root.path(),
            "oci",
            &launch,
            "Usage limit reached",
            &DependencyEvidence {
                kind: "quota".into(),
                reset_at: None,
            },
        )
        .unwrap();
        assert!(reset_pending(root.path(), "oci", &launch));
        let mut wait = load_dependency(&dependency_path(root.path(), "oci", &launch)).unwrap();
        wait.evidence.reset_at = Some(0);
        crate::project::write_json(&dependency_path(root.path(), "oci", &launch), &wait).unwrap();
        dependency_ready(root.path(), "oci", &launch, || Ok(())).unwrap();
        assert_eq!(
            terminal_dependency("Usage limit reached\n❯")
                .unwrap()
                .1
                .reset_at,
            None
        );
        assert_eq!(
            terminal_dependency("Usage limit reached for account 2099-01-01T00:00:00Z")
                .unwrap()
                .1
                .reset_at,
            None
        );
    }

    #[test]
    fn auth_is_shown_once_for_the_exact_machine_without_inventing_a_reset() {
        let world = crate::scenarios::World::new();
        let first = world.project("first", "a.sock");
        let second = world.project("second", "b.sock");
        let launch = crate::contracts::Launch {
            kind: "pi".into(),
            ..Default::default()
        };
        assert!(
            dependency_ready(&world.root, "oci", &launch, || bail!(
                "authentication failed"
            ))
            .is_err()
        );
        for project in [&first, &first, &second] {
            notify_auth(&world.root, project, "oci", &launch).unwrap();
        }
        let notices: Vec<_> = crate::inbox::unhandled(&first)
            .into_iter()
            .chain(crate::inbox::unhandled(&second))
            .filter(|notice| notice.kind == "dependency-auth")
            .collect();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].summary.contains("machine `oci`"));
        assert!(
            load_dependency(&dependency_path(&world.root, "oci", &launch))
                .unwrap()
                .evidence
                .reset_at
                .is_none()
        );
        // A connection receipt is not proof of provider authentication.
        machine_reconnected(&world.root, "oci", &launch).unwrap();
        assert!(
            dependency_ready(&world.root, "oci", &launch, || panic!("auth remains held")).is_err()
        );
    }

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
