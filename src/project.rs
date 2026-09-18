//! Project folders under the root: slugs, settings, status, the per-project
//! lock and the coordinator record.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contracts::{DAY_ONE_ROLES, LaunchRecipe, RoleSpec, RolesTable};

pub const MAX_SLUG: usize = 40;
pub const BODY_WARN_CHARS: usize = 16_000;

/// A slug matches `[a-z0-9][a-z0-9-]*` and is at most 40 characters. Every
/// subcommand validates the slug it is given before building any path from it.
pub fn validate_slug(slug: &str) -> Result<()> {
    let mut chars = slug.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !first_ok || !rest_ok || slug.len() > MAX_SLUG {
        bail!(
            "`{slug}` is not a valid slug (lower-case letters, digits and hyphens, at most {MAX_SLUG} characters)"
        );
    }
    Ok(())
}

/// Lower-cases and turns each run of other characters into one hyphen. Used for
/// project names and for thread titles in branch names.
pub fn slugify(text: &str) -> String {
    let mut slug = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let mut slug: String = slug.chars().take(MAX_SLUG).collect();
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// The slug `new` gives a project name, refusing names that look like paths.
pub fn slug_from_name(name: &str) -> Result<String> {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        bail!("a project name may not contain `/`, `\\` or `..`");
    }
    let slug = slugify(name);
    if slug.is_empty() {
        bail!("`{name}` has no letters or digits to make a slug from");
    }
    validate_slug(&slug)?;
    Ok(slug)
}

/// Words split on `-` and `_`, each with its first letter upper-cased:
/// `herdr-projects` becomes `Herdr Projects`. Plain title case, so `gtm-ai`
/// becomes `Gtm Ai`; a user who wants `GTM AI` sets `name` in PROJECT.md.
pub fn humanize(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The name a project shows: `name` as given unless it is empty or looks like
/// a slug (lower-case letters, digits, `-` and `_` only), else the humanized
/// form of it or of `slug`. A herdr workspace never shows a bare slug, which
/// would read the same as a repository's own workspace.
pub fn display_name(name: &str, slug: &str) -> String {
    let name = name.trim();
    let base = if name.is_empty() { slug } else { name };
    let slug_like = base
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if slug_like {
        humanize(base)
    } else {
        base.to_string()
    }
}

/// Writes through a temporary file in the same directory plus a rename. It never
/// creates parent directories: only `new` creates a project's directories.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    let name = path.file_name().context("path has no file name")?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let result = (|| -> Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("could not write {}", path.display()))
}

pub fn now() -> String {
    jiff::Timestamp::now()
        .round(jiff::Unit::Second)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Repo {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

/// `PROJECT.md` front matter. `repos` is last so the TOML tables follow the
/// plain keys when `new` serializes it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub name: String,
    pub goal: String,
    /// Kept for unowned callers (`coordinator.rs`). New files do not write it
    /// (SPEC-ADE D2). `doctor` refuses a project whose front matter still has
    /// the key.
    #[serde(default = "default_claude", skip_serializing)]
    pub coordinator_agent: String,
    #[serde(default = "default_claude", skip_serializing)]
    pub thread_agent: String,
    pub max_parallel_threads: u32,
    pub auto_resolve_days: u32,
    pub nudge: bool,
    /// Plugin-owned conversation surface (SPEC-ADE D18). Default off.
    #[serde(default)]
    pub talk: bool,
    /// Per-role `kind`/`args` overrides (SPEC-ADE D2). Arrays replace.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub roles: std::collections::BTreeMap<String, RoleOverride>,
    pub repos: Vec<Repo>,
}

fn default_claude() -> String {
    "claude".into()
}

/// Project override of one role. Only `kind` and `args` (SPEC-ADE D2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct RoleOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            name: String::new(),
            goal: String::new(),
            coordinator_agent: default_claude(),
            thread_agent: default_claude(),
            max_parallel_threads: 3,
            auto_resolve_days: 7,
            // Off by default: on herdr 0.9.1 a prompt merges with, and submits,
            // text the user has half-typed (docs/herdr-notes.md, stage 2). With
            // `false` the ticker shows a herdr notification instead.
            nudge: false,
            talk: false,
            roles: std::collections::BTreeMap::new(),
            repos: Vec::new(),
        }
    }
}

/// Splits `+++` TOML front matter from the body.
pub fn parse_project_md(text: &str) -> Result<(Settings, String)> {
    let rest = text
        .strip_prefix("+++\n")
        .context("PROJECT.md must start with a `+++` line")?;
    let (front, body) = match rest.split_once("\n+++\n") {
        Some(parts) => parts,
        None => rest
            .strip_suffix("\n+++")
            .map(|front| (front, ""))
            .context("PROJECT.md front matter has no closing `+++` line")?,
    };
    let settings: Settings =
        toml::from_str(front).context("PROJECT.md front matter does not parse")?;
    Ok((settings, body.trim_start_matches('\n').to_string()))
}

/// Keys D2 removes. Present in the front-matter table, not merely defaulted.
pub fn legacy_agent_keys(front: &str) -> Vec<String> {
    let Ok(value) = toml::from_str::<toml::Value>(front) else {
        return Vec::new();
    };
    let Some(table) = value.as_table() else {
        return Vec::new();
    };
    [
        "coordinator_agent",
        "thread_agent",
        "coordinator_agent_args",
        "thread_agent_args",
    ]
    .into_iter()
    .filter(|key| table.contains_key(*key))
    .map(str::to_string)
    .collect()
}

/// Front matter between the `+++` lines.
pub fn project_md_front(text: &str) -> Result<&str> {
    let rest = text
        .strip_prefix("+++\n")
        .context("PROJECT.md must start with a `+++` line")?;
    match rest.split_once("\n+++\n") {
        Some((front, _)) => Ok(front),
        None => rest
            .strip_suffix("\n+++")
            .context("PROJECT.md front matter has no closing `+++` line"),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Active,
    Paused,
    Archived,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Status::Active => "active",
            Status::Paused => "paused",
            Status::Archived => "archived",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
struct ProjectState {
    status: Status,
}

/// The coordinator's pane and the session the project belongs to.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Coordinator {
    pub socket: String,
    /// Empty when the session was chosen by socket path alone.
    pub session: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub agent_name: String,
    pub cwd: String,
    pub prime_pending: bool,
    pub launch_attempts: u32,
    pub updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Safety {
    pub start_threads: String,
    pub coordinator_agent_args: Vec<String>,
    pub thread_agent_args: Vec<String>,
    pub routine_commands: bool,
}

impl Default for Safety {
    fn default() -> Self {
        Safety {
            start_threads: "propose".into(),
            coordinator_agent_args: Vec::new(),
            thread_agent_args: Vec::new(),
            routine_commands: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub slug: String,
}

/// Held while reading and rewriting anything under `threads/`, `inbox/` or
/// `.state/`. Never held across a herdr, git, gh, ssh or scp call.
pub struct ProjectLock {
    _file: File,
}

impl Project {
    /// An existing project. Validates the slug before building any path.
    pub fn load(root: &Path, slug: &str) -> Result<Project> {
        validate_slug(slug)?;
        let project = Project {
            root: root.to_path_buf(),
            slug: slug.to_string(),
        };
        if !project.project_md().is_file() {
            bail!("no project `{slug}` in {}", root.display());
        }
        Ok(project)
    }

    pub fn dir(&self) -> PathBuf {
        self.root.join(&self.slug)
    }

    pub fn project_md(&self) -> PathBuf {
        self.dir().join("PROJECT.md")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.dir().join(".state")
    }

    /// The canonical folder (symlinks resolved): the key of the project's
    /// `[safety]` table and of its routine approvals.
    pub fn canonical_dir(&self) -> PathBuf {
        std::fs::canonicalize(self.dir()).unwrap_or_else(|_| self.dir())
    }

    /// Takes the per-project lock. The lock file is opened without creating
    /// parent directories, and the project is re-checked afterwards, so a
    /// `delete` that lands mid-operation cannot be resurrected by a writer.
    pub fn lock(&self) -> Result<ProjectLock> {
        let path = self.state_dir().join("lock");
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("project `{}` is gone ({})", self.slug, path.display()))?;
        file.lock()?;
        if !self.project_md().is_file() {
            bail!("project `{}` is gone", self.slug);
        }
        Ok(ProjectLock { _file: file })
    }

    pub fn read_project_md(&self) -> Result<(Settings, String)> {
        let text = std::fs::read_to_string(self.project_md())
            .with_context(|| format!("could not read {}", self.project_md().display()))?;
        parse_project_md(&text)
    }

    pub fn status(&self) -> Status {
        read_json::<ProjectState>(&self.state_dir().join("project.json"))
            .unwrap_or_default()
            .status
    }

    pub fn set_status(&self, status: Status) -> Result<()> {
        let _lock = self.lock()?;
        write_json(
            &self.state_dir().join("project.json"),
            &ProjectState { status },
        )
    }

    pub fn coordinator(&self) -> Option<Coordinator> {
        read_json(&self.state_dir().join("coordinator.json"))
    }

    /// Read-modify-write of `coordinator.json` under the lock: re-reads the
    /// file, lets `change` touch only the fields its step owns, writes.
    pub fn update_coordinator(&self, change: impl FnOnce(&mut Coordinator)) -> Result<Coordinator> {
        let _lock = self.lock()?;
        let mut record = self.coordinator().unwrap_or_default();
        change(&mut record);
        record.updated = now();
        write_json(&self.state_dir().join("coordinator.json"), &record)?;
        Ok(record)
    }

    pub fn safety(&self, config_dir: &Path) -> Result<Safety> {
        load_safety(config_dir, &self.canonical_dir())
    }
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    write_atomic(path, text.as_bytes())
}

/// The effective safety settings: `[safety."<canonical project path>"]` in
/// `<config_dir>/config.toml`, with defaults for an absent table or key.
pub fn load_safety(config_dir: &Path, canonical_project_dir: &Path) -> Result<Safety> {
    #[derive(Deserialize, Default)]
    struct Config {
        #[serde(default)]
        safety: std::collections::BTreeMap<String, Safety>,
    }
    let file = config_dir.join("config.toml");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Ok(Safety::default());
    };
    let mut config: Config =
        toml::from_str(&text).with_context(|| format!("{} does not parse", file.display()))?;
    let safety = config
        .safety
        .remove(&*canonical_project_dir.to_string_lossy())
        .unwrap_or_default();
    if !matches!(safety.start_threads.as_str(), "propose" | "auto") {
        bail!(
            "{}: start_threads must be \"propose\" or \"auto\", not {:?}",
            file.display(),
            safety.start_threads
        );
    }
    Ok(safety)
}

/// Kinds that need a permission flag in `args` (SPEC-ADE D2).
pub fn kind_needs_permission_args(kind: &str) -> bool {
    matches!(kind, "claude" | "cursor" | "agy")
}

/// Plugin default for a day-one role when the safety file has no row.
pub fn default_role_spec(name: &str) -> RoleSpec {
    if name == "pro" {
        RoleSpec {
            kind: "chatgpt".into(),
            ..RoleSpec::default()
        }
    } else {
        RoleSpec {
            kind: "claude".into(),
            ..RoleSpec::default()
        }
    }
}

/// `[roles.<name>]` rows from `config.toml`.
///
/// SPEC-jev-picker v2 `[roles]`/`[recipes]` parsing is lane A4's. This loader
/// keeps only tables that have a D2 `kind` field and skips scalar keys such as
/// `resolver`. When A4 lands, `crate::picker::resolve_launch` runs first:
/// `let spec = crate::picker::resolve_launch(project, role, task, sibling)?;`
pub fn load_roles(config_dir: &Path) -> Result<RolesTable> {
    #[derive(Deserialize)]
    struct Config {
        #[serde(default)]
        roles: Option<toml::Value>,
    }
    let file = config_dir.join("config.toml");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Ok(RolesTable::default());
    };
    let config: Config =
        toml::from_str(&text).with_context(|| format!("{} does not parse", file.display()))?;
    match config.roles {
        Some(value) => parse_roles_value(&value)
            .with_context(|| format!("{} [roles] does not parse", file.display())),
        None => Ok(RolesTable::default()),
    }
}

fn parse_roles_value(value: &toml::Value) -> Result<RolesTable> {
    let Some(table) = value.as_table() else {
        return Ok(RolesTable::default());
    };
    let mut roles = std::collections::BTreeMap::new();
    for (name, inner) in table {
        let Some(row) = inner.as_table() else {
            continue;
        };
        if !row.contains_key("kind") {
            continue;
        }
        let spec: RoleSpec = inner.clone().try_into().with_context(|| {
            format!("[roles.{name}] has an unknown field or a bad type (SPEC-ADE D2)")
        })?;
        roles.insert(name.clone(), spec);
    }
    Ok(RolesTable { roles })
}

fn apply_override(base: RoleSpec, over: &RoleOverride) -> Result<RoleSpec> {
    let mut out = base;
    if let Some(kind) = &over.kind {
        if kind != &out.kind {
            let Some(args) = &over.args else {
                bail!("role_args_missing");
            };
            out.kind = kind.clone();
            out.args = args.clone();
        } else if let Some(args) = &over.args {
            out.args = args.clone();
        }
    } else if let Some(args) = &over.args {
        out.args = args.clone();
    }
    Ok(out)
}

/// Resolve a role: defaults, then config table, then PROJECT.md override.
/// Arrays replace, never merge (SPEC-ADE D2).
pub fn resolve_role(config_dir: &Path, settings: &Settings, role: &str) -> Result<RoleSpec> {
    if role.is_empty() {
        bail!("a role name is required");
    }
    let table = load_roles(config_dir)?;
    let mut spec = table
        .roles
        .get(role)
        .cloned()
        .unwrap_or_else(|| default_role_spec(role));
    if let Some(over) = settings.roles.get(role) {
        spec = apply_override(spec, over)?;
    }
    if spec.ready_timeout_ms == 0 {
        spec.ready_timeout_ms = 20_000;
    }
    Ok(spec)
}

/// SHA-256 of `config.toml` and `RULES.md` (SPEC-ADE D11).
pub fn policy_hash(config_dir: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(std::fs::read(config_dir.join("config.toml")).unwrap_or_default());
    hasher.update(std::fs::read(config_dir.join("RULES.md")).unwrap_or_default());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Launch recipe stored on the thread, never rebuilt from mutable settings.
pub fn launch_recipe(
    spec: &RoleSpec,
    attempt: u32,
    brief_hash: String,
    policy: String,
) -> LaunchRecipe {
    LaunchRecipe {
        kind: spec.kind.clone(),
        args: spec.args.clone(),
        env: spec.env.clone(),
        ready_timeout_ms: if spec.ready_timeout_ms == 0 {
            20_000
        } else {
            spec.ready_timeout_ms
        },
        policy_hash: policy,
        attempt,
        brief_hash,
    }
}

/// `--env` values for `tab create`, including `HERDR_ADE_LAUNCH` (SPEC-ADE D4).
pub fn tab_env(
    slug: &str,
    thread: &str,
    attempt: u32,
    brief_hash: &str,
    spec: &RoleSpec,
) -> Vec<String> {
    let mut env = vec![format!(
        "HERDR_ADE_LAUNCH={slug}/{thread}/{attempt}/{brief_hash}"
    )];
    env.extend(spec.env.iter().cloned());
    if spec.kind == "dsh" {
        if !env.iter().any(|e| e.starts_with("DSH_PERMISSION_MODE=")) {
            env.push("DSH_PERMISSION_MODE=danger-full-access".into());
        }
        if !env.iter().any(|e| e.starts_with("DSH_TUI_LANG=")) {
            env.push("DSH_TUI_LANG=en".into());
        }
    }
    env
}

/// Day-one role names, for doctor.
pub fn day_one_roles() -> &'static [&'static str] {
    &DAY_ONE_ROLES
}

/// Slugs of the projects in `root`: folders that contain `PROJECT.md`. Entries
/// whose names start with a dot are ignored. A missing root has no projects.
pub fn list_slugs(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut slugs: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.') && validate_slug(name).is_ok())
        .filter(|name| root.join(name).join("PROJECT.md").is_file())
        .collect();
    slugs.sort();
    slugs
}

/// `PATH[@MACHINE]` as given to `new --repo`.
pub fn parse_repo_arg(arg: &str) -> Repo {
    if let Some((path, machine)) = arg.rsplit_once('@') {
        let label_like = !machine.is_empty()
            && machine
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if label_like && !path.is_empty() {
            return Repo {
                path: path.to_string(),
                machine: Some(machine.to_string()),
            };
        }
    }
    Repo {
        path: arg.to_string(),
        machine: None,
    }
}

const TASKS_TEMPLATE: &str = "# Tasks\n\n## Backlog\n";

const INSTRUCTIONS_TEMPLATE: &str = "\
# Instructions

Standing instructions for this project. Every thread starts from this text and
from the project's memory. Replace this paragraph with how you want work done:
conventions, what to check before finishing, what never to do.

The settings above, between the `+++` lines, are yours to edit. `nudge = true`
lets the ticker prompt the coordinator when something changed; it is off by
default because a prompt that arrives while you are typing in the coordinator
is merged with, and submits, your half-typed text. With it off you get a herdr
notification instead.
";

/// Creates the folder and skeleton files. The only code path that creates a
/// project's directories. Fails if the slug exists.
pub fn create(root: &Path, name: &str, goal: &str, repos: Vec<Repo>) -> Result<Project> {
    let slug = slug_from_name(name)?;
    let project = Project {
        root: root.to_path_buf(),
        slug: slug.clone(),
    };
    let dir = project.dir();
    if dir.exists() {
        bail!("`{slug}` already exists in {}", root.display());
    }
    let repos = repos
        .into_iter()
        .map(|repo| match repo.machine {
            // A remote path is stored as it is on its own machine.
            Some(_) => repo,
            None => Repo {
                path: std::fs::canonicalize(&repo.path)
                    .or_else(|_| std::path::absolute(&repo.path))
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or(repo.path),
                machine: None,
            },
        })
        .collect();
    let settings = Settings {
        name: display_name(name, &slug),
        goal: goal.to_string(),
        repos,
        ..Settings::default()
    };
    let front = toml::to_string(&settings)?;

    std::fs::create_dir_all(root)?;
    std::fs::create_dir(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    for sub in [
        "memory",
        "scratch",
        "routines",
        "threads",
        "inbox",
        "inbox/done",
        "library",
        ".state",
    ] {
        std::fs::create_dir_all(dir.join(sub))?;
    }
    write_atomic(
        &dir.join("MEMORY.md"),
        b"# Memory\n\nOne line per memory file: `- [title](memory/file.md): what it holds`.\n",
    )?;
    write_atomic(&dir.join("TASKS.md"), TASKS_TEMPLATE.as_bytes())?;
    write_json(
        &project.state_dir().join("project.json"),
        &ProjectState::default(),
    )?;
    // PROJECT.md last: a folder without it is not a project, so a half-made
    // skeleton is never picked up by `list` or the ticker.
    write_atomic(
        &project.project_md(),
        format!("+++\n{front}+++\n\n{INSTRUCTIONS_TEMPLATE}").as_bytes(),
    )?;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_folders_with_project_md_count() {
        let root = tempfile::tempdir().unwrap();
        for name in ["b", "a", ".trash", "empty", "Not_A_Slug"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        for name in ["b", "a", ".trash", "Not_A_Slug"] {
            std::fs::write(root.path().join(name).join("PROJECT.md"), "").unwrap();
        }
        assert_eq!(list_slugs(root.path()), ["a", "b"]);
        assert!(list_slugs(&root.path().join("missing")).is_empty());
    }

    #[test]
    fn slug_validation() {
        for good in ["a", "demo", "demo-2", "0x", &"a".repeat(40)] {
            assert!(validate_slug(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "-a",
            "A",
            "a_b",
            "a/b",
            "../x",
            "a b",
            ".",
            "..",
            &"a".repeat(41),
        ] {
            assert!(validate_slug(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_slug_like_name_is_humanized_and_a_typed_name_is_kept() {
        assert_eq!(humanize("herdr-projects"), "Herdr Projects");
        assert_eq!(humanize("gtm_ai"), "Gtm Ai");
        assert_eq!(humanize("-v2--api-"), "V2 Api");
        assert_eq!(
            display_name("herdr-projects", "herdr-projects"),
            "Herdr Projects"
        );
        assert_eq!(display_name("", "herdr-projects"), "Herdr Projects");
        assert_eq!(display_name("  ", "demo"), "Demo");
        for typed in ["GTM AI", "my project", "Demo", "herdr-Projects"] {
            assert_eq!(display_name(typed, "x"), typed);
        }
    }

    #[test]
    fn create_stores_a_display_name_and_keeps_the_slug() {
        let root = tempfile::tempdir().unwrap();
        let project = create(root.path(), "herdr-projects", "", vec![]).unwrap();
        assert_eq!(project.slug, "herdr-projects");
        assert_eq!(project.read_project_md().unwrap().0.name, "Herdr Projects");
        let project = create(root.path(), "GTM AI", "", vec![]).unwrap();
        assert_eq!(project.slug, "gtm-ai");
        assert_eq!(project.read_project_md().unwrap().0.name, "GTM AI");
    }

    #[test]
    fn slug_derivation_and_name_refusals() {
        assert_eq!(
            slug_from_name("My Demo  Project!").unwrap(),
            "my-demo-project"
        );
        assert_eq!(slug_from_name("  Ünï 42 ").unwrap(), "n-42");
        assert_eq!(slug_from_name(&"x".repeat(60)).unwrap().len(), 40);
        for bad in ["../x", "a/b", "a\\b", "..", "!!!", ""] {
            assert!(slug_from_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn create_writes_the_skeleton_and_refuses_a_second_time() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().join("root");
        let project = create(
            &root,
            "Demo",
            "Ship \"it\"",
            vec![
                parse_repo_arg("/srv/app@box"),
                parse_repo_arg("/no/such/repo"),
            ],
        )
        .unwrap();
        assert_eq!(project.slug, "demo");
        for sub in [
            "memory",
            "scratch",
            "routines",
            "threads",
            "inbox/done",
            "library",
            ".state",
        ] {
            assert!(project.dir().join(sub).is_dir(), "{sub}");
        }
        assert!(project.dir().join("MEMORY.md").is_file());
        assert!(project.dir().join("TASKS.md").is_file());
        let (settings, body) = project.read_project_md().unwrap();
        assert_eq!(settings.name, "Demo");
        assert_eq!(settings.goal, "Ship \"it\"");
        assert_eq!(settings.coordinator_agent, "claude");
        assert_eq!(settings.max_parallel_threads, 3);
        assert_eq!(settings.auto_resolve_days, 7);
        assert!(!settings.nudge);
        assert_eq!(
            settings.repos,
            vec![
                Repo {
                    path: "/srv/app".into(),
                    machine: Some("box".into())
                },
                Repo {
                    path: "/no/such/repo".into(),
                    machine: None
                },
            ]
        );
        assert!(body.starts_with("# Instructions"));
        assert_eq!(project.status(), Status::Active);
        assert!(create(&root, "demo", "", vec![]).is_err());
    }

    #[test]
    fn front_matter_parsing() {
        let (settings, body) =
            parse_project_md("+++\nname = \"X\"\nnudge = true\n+++\n\nBody\n+++\nmore\n").unwrap();
        assert_eq!(settings.name, "X");
        assert!(settings.nudge);
        assert_eq!(settings.thread_agent, "claude");
        assert_eq!(body, "Body\n+++\nmore\n");
        assert!(parse_project_md("no front matter").is_err());
        assert!(parse_project_md("+++\nname = \n+++\n").is_err());
        assert!(parse_project_md("+++\nname = \"X\"\n").is_err());
        let (_, body) = parse_project_md("+++\nname = \"X\"\n+++").unwrap();
        assert_eq!(body, "");
    }

    #[test]
    fn repo_arg_parsing() {
        assert_eq!(parse_repo_arg("/a/b").machine, None);
        assert_eq!(parse_repo_arg("/a/b@m1").machine.as_deref(), Some("m1"));
        assert_eq!(parse_repo_arg("/a/b@m1").path, "/a/b");
        // An `@` inside a path is not a machine.
        assert_eq!(parse_repo_arg("/a@b/c").machine, None);
        assert_eq!(parse_repo_arg("/a@b/c").path, "/a@b/c");
    }

    #[test]
    fn safety_defaults_and_overrides_keyed_by_canonical_path() {
        let config = tempfile::tempdir().unwrap();
        let here = Path::new("/projects/demo");
        assert_eq!(load_safety(config.path(), here).unwrap(), Safety::default());

        std::fs::write(
            config.path().join("config.toml"),
            "root = \"/projects\"\n\n[safety.\"/projects/demo\"]\nstart_threads = \"auto\"\nthread_agent_args = [\"--x\"]\n",
        )
        .unwrap();
        let safety = load_safety(config.path(), here).unwrap();
        assert_eq!(safety.start_threads, "auto");
        assert_eq!(safety.thread_agent_args, ["--x"]);
        assert!(!safety.routine_commands);
        assert!(safety.coordinator_agent_args.is_empty());
        assert_eq!(
            load_safety(config.path(), Path::new("/projects/other")).unwrap(),
            Safety::default()
        );

        std::fs::write(
            config.path().join("config.toml"),
            "[safety.\"/projects/demo\"]\nstart_threads = \"yolo\"\n",
        )
        .unwrap();
        assert!(load_safety(config.path(), here).is_err());
    }

    #[test]
    fn writers_drop_their_write_when_project_md_is_gone() {
        let root = tempfile::tempdir().unwrap();
        let project = create(root.path(), "demo", "", vec![]).unwrap();
        std::fs::remove_file(project.project_md()).unwrap();
        assert!(
            project
                .update_coordinator(|c| c.pane_id = "w1:p1".into())
                .is_err()
        );
        assert!(project.coordinator().is_none());

        // A deleted folder is not recreated by taking the lock.
        std::fs::remove_dir_all(project.dir()).unwrap();
        assert!(project.lock().is_err());
        assert!(!project.dir().exists());
    }

    #[test]
    fn coordinator_updates_keep_other_fields() {
        let root = tempfile::tempdir().unwrap();
        let project = create(root.path(), "demo", "", vec![]).unwrap();
        project
            .update_coordinator(|c| c.socket = "/s".into())
            .unwrap();
        project
            .update_coordinator(|c| c.prime_pending = true)
            .unwrap();
        let record = project.coordinator().unwrap();
        assert_eq!(record.socket, "/s");
        assert!(record.prime_pending);
        assert!(
            std::fs::read_dir(project.state_dir())
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp"))
        );
    }

    #[test]
    fn new_project_does_not_write_removed_agent_keys() {
        let root = tempfile::tempdir().unwrap();
        let project = create(root.path(), "demo", "", vec![]).unwrap();
        let text = std::fs::read_to_string(project.project_md()).unwrap();
        let front = project_md_front(&text).unwrap();
        assert!(legacy_agent_keys(front).is_empty(), "{front}");
        assert!(front.contains("talk = false"));
        let (settings, _) = parse_project_md(&text).unwrap();
        assert_eq!(settings.coordinator_agent, "claude");
        assert!(!settings.talk);
    }

    #[test]
    fn roles_replace_never_merge_and_kind_change_needs_args() {
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            config.path().join("config.toml"),
            "[roles.lane]\nkind = \"claude\"\nargs = [\"--a\", \"--b\"]\nready_timeout_ms = 15000\n",
        )
        .unwrap();
        let mut settings = Settings::default();
        let spec = resolve_role(config.path(), &settings, "lane").unwrap();
        assert_eq!(spec.kind, "claude");
        assert_eq!(spec.args, ["--a", "--b"]);
        assert_eq!(spec.ready_timeout_ms, 15_000);

        settings.roles.insert(
            "lane".into(),
            RoleOverride {
                kind: None,
                args: Some(vec!["--x".into()]),
            },
        );
        let spec = resolve_role(config.path(), &settings, "lane").unwrap();
        assert_eq!(spec.args, ["--x"]);
        assert_eq!(spec.kind, "claude");

        settings.roles.insert(
            "lane".into(),
            RoleOverride {
                kind: Some("cursor".into()),
                args: None,
            },
        );
        let err = resolve_role(config.path(), &settings, "lane")
            .unwrap_err()
            .to_string();
        assert!(err.contains("role_args_missing"), "{err}");

        settings.roles.insert(
            "lane".into(),
            RoleOverride {
                kind: Some("cursor".into()),
                args: Some(vec!["--force".into()]),
            },
        );
        let spec = resolve_role(config.path(), &settings, "lane").unwrap();
        assert_eq!(spec.kind, "cursor");
        assert_eq!(spec.args, ["--force"]);
    }

    #[test]
    fn unknown_role_field_is_refused_and_picker_keys_are_skipped() {
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            config.path().join("config.toml"),
            "[roles]\nresolver = \"off\"\n\n[roles.lane]\nkind = \"claude\"\nbootstrap = true\n",
        )
        .unwrap();
        let err = format!("{:#}", load_roles(config.path()).unwrap_err());
        assert!(
            err.contains("unknown") || err.contains("bootstrap"),
            "{err}"
        );

        std::fs::write(
            config.path().join("config.toml"),
            "[roles]\nresolver = \"off\"\n\n[roles.lane]\nkind = \"claude\"\n",
        )
        .unwrap();
        let table = load_roles(config.path()).unwrap();
        assert_eq!(table.roles["lane"].kind, "claude");
    }

    #[test]
    fn default_timeout_and_dsh_env() {
        let spec = RoleSpec {
            kind: "dsh".into(),
            ..RoleSpec::default()
        };
        let recipe = launch_recipe(&spec, 1, "bh".into(), "ph".into());
        assert_eq!(recipe.ready_timeout_ms, 20_000);
        let env = tab_env("demo", "t-0001", 1, "abcd", &spec);
        assert!(
            env.iter()
                .any(|e| e == "HERDR_ADE_LAUNCH=demo/t-0001/1/abcd")
        );
        assert!(env.iter().any(|e| e.starts_with("DSH_PERMISSION_MODE=")));
    }
}
