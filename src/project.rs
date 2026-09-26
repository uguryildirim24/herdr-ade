//! Project folders under the root: slugs, settings, status, the per-project
//! lock and the coordinator record.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contracts::{Launch, RoleSpec};

const MAX_SLUG: usize = 40;

/// A slug matches `[a-z0-9][a-z0-9-]*` and is at most 40 characters. Every
/// subcommand validates the slug it is given before building any path from it.
pub(crate) fn validate_slug(slug: &str) -> Result<()> {
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
pub(crate) fn slugify(text: &str) -> String {
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
pub(crate) fn slug_from_name(name: &str) -> Result<String> {
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
fn humanize(slug: &str) -> String {
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
pub(crate) fn display_name(name: &str, slug: &str) -> String {
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
/// creates parent directories; each optional store creates its folder on first use.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
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

pub(crate) fn now() -> String {
    jiff::Timestamp::now()
        .round(jiff::Unit::Second)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// One repository gate, including the exact environment needed to run it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct Gate {
    pub(crate) command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub(crate) struct Repo {
    pub(crate) path: String,
    /// The integration branch. When absent, the repository's checked-out
    /// branch is used at round open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    /// The only remote to which a completed integration branch may be pushed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) push_remote: Option<String>,
    /// `None` means gates are not configured; `Some([])` explicitly makes the
    /// repository gate-free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) gates: Option<Vec<Gate>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) machine: Option<String>,
    /// The box clone path for this repository (SPEC-remote §4.1). When
    /// present, the project's own row wins over the committed default map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) box_path: Option<String>,
    /// The URL-matched remote the lane branch publishes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) publish_url: Option<String>,
    /// Rebuildable ignored paths specific to this repository. These are added
    /// to the global worktree list and never apply to another repository.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) disposable: Vec<String>,
    /// Task milestones this repository requires. An empty list inherits the
    /// project's list; repositories without an install step simply omit it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) task_states: Vec<String>,
}

/// `PROJECT.md` front matter. `repos` is last so the TOML tables follow the
/// plain keys when `new` serializes it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct Settings {
    pub(crate) name: String,
    pub(crate) goal: String,
    /// Ordered task milestones used when a repository has no override.
    #[serde(default = "default_task_states")]
    pub(crate) task_states: Vec<String>,
    pub(crate) repos: Vec<Repo>,
}

fn default_task_states() -> Vec<String> {
    ["finished", "reviewed", "merged"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            name: String::new(),
            goal: String::new(),
            task_states: default_task_states(),
            repos: Vec::new(),
        }
    }
}

/// Splits `+++` TOML front matter from the body.
fn parse_project_md(text: &str) -> Result<(Settings, String)> {
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
    let value: toml::Value =
        toml::from_str(front).context("PROJECT.md front matter does not parse")?;
    if let Some(table) = value.as_table() {
        if table.contains_key("roles") {
            bail!(
                "roles_removed: remove roles from PROJECT.md; recipes and routing live in config.toml"
            );
        }
        if table.contains_key("gates") {
            bail!("gates_removed: move gates into each repository row in PROJECT.md");
        }
    }
    let settings: Settings = value
        .try_into()
        .context("PROJECT.md front matter does not parse")?;
    Ok((settings, body.trim_start_matches('\n').to_string()))
}

/// Removed keys that `doctor` refuses when they remain in front matter.
pub(crate) fn removed_project_keys(front: &str) -> Vec<String> {
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
        "max_parallel_threads",
        "gates",
    ]
    .into_iter()
    .filter(|key| table.contains_key(*key))
    .map(str::to_string)
    .collect()
}

/// Front matter between the `+++` lines.
pub(crate) fn project_md_front(text: &str) -> Result<&str> {
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
pub(crate) enum Status {
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
pub(crate) struct Coordinator {
    pub(crate) socket: String,
    /// Empty when the session was chosen by socket path alone.
    pub(crate) session: String,
    pub(crate) workspace_id: String,
    pub(crate) tab_id: String,
    pub(crate) pane_id: String,
    pub(crate) agent_name: String,
    pub(crate) cwd: String,
    /// Set once when the bound pane disappears on a still-running server.
    pub(crate) closed_by_rolf_at: String,
    /// A new message asks the ticker to reopen a previously closed pane.
    pub(crate) reopen_requested: bool,
    /// Socket inode of the server that hosted this pane. A changed inode is
    /// evidence of a server restart, not a manually closed pane.
    pub(crate) server_socket_inode: u64,
    /// Evidence that an agent ran here, before reporting this pane's process dead.
    pub(crate) last_agent_seen_at: String,
    pub(crate) prime_pending: bool,
    pub(crate) launch_attempts: u32,
    pub(crate) updated: String,
    /// The coordinator's launch recipe from the `coordinator` role (SPEC-ADE
    /// D2), stored at `open` and reused by the ticker's relaunch. `attempt`
    /// counts coordinator tabs; `brief_hash` is the SHA-256 of `PROJECT.md`
    /// at `open`. Both reach the pane in `HERDR_ADE_LAUNCH` (D14).
    pub(crate) launch: Launch,
    /// The priming line was submitted for this binding. Transport is not the
    /// receipt: `prime_pending` clears only on the `ha context` receipt, and
    /// the ticker never re-sends a submitted line on its own (D14).
    pub(crate) prime_sent: bool,
    /// `"acknowledged"` after the matching bootstrap call (D14).
    pub(crate) bootstrap: String,
    /// Counts coordinator agent starts for this project and never resets:
    /// `open` and the ticker's relaunch each start a new incarnation, and a
    /// sealed event is bound to the one it was sealed for (D5 X5).
    pub(crate) generation: u32,
    /// How many times `open` or the ticker put the recorded name back on the
    /// agent running in the bound pane after herdr dropped it. A durable record
    /// of a repair, so `doctor` can say it happened.
    pub(crate) name_restored: u32,
}

impl Coordinator {
    /// The recipient attempt events and receipts bind (D5 `coordinator_attempt`).
    pub(crate) fn attempt(&self) -> u32 {
        self.generation.max(1)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Project {
    pub(crate) root: PathBuf,
    pub(crate) slug: String,
}

/// Held while reading and rewriting anything under `threads/`, `inbox/` or
/// `.state/`. Never held across a herdr, git, gh, ssh or scp call.
pub(crate) struct ProjectLock {
    _file: File,
}

/// Held while a box start fetches and creates its worktree, keyed by the
/// stable profile id and the box repository so starts for one box repository
/// serialize (SPEC-remote §4.2 step 3).
pub(crate) struct BoxLock {
    _file: File,
}

/// Held while one project finds or creates its shared workspace on a machine.
/// Repository provisioning has a different lock because one project may span
/// repositories while still owning exactly one remote workspace.
pub(crate) struct RemoteWorkspaceLock {
    _file: File,
}

pub(crate) fn remote_workspace_lock(
    root: &Path,
    slug: &str,
    machine_id: &str,
) -> Result<RemoteWorkspaceLock> {
    let dir = root.join(".locks");
    std::fs::create_dir_all(&dir)?;
    let mut hasher = Sha256::new();
    hasher.update(b"workspace\n");
    hasher.update(slug.as_bytes());
    hasher.update(b"\n");
    hasher.update(machine_id.as_bytes());
    let key: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let path = dir.join(format!("{key}.lock"));
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open remote workspace lock {}", path.display()))?;
    file.lock()?;
    Ok(RemoteWorkspaceLock { _file: file })
}

pub(crate) fn box_lock(root: &Path, machine_id: &str, box_repo: &str) -> Result<BoxLock> {
    let dir = root.join(".locks");
    std::fs::create_dir_all(&dir)?;
    let mut hasher = Sha256::new();
    hasher.update(machine_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(box_repo.as_bytes());
    let key: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let path = dir.join(format!("{key}.lock"));
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open box lock {}", path.display()))?;
    file.lock()?;
    Ok(BoxLock { _file: file })
}

impl Project {
    /// An existing project. Validates the slug before building any path.
    pub(crate) fn load(root: &Path, slug: &str) -> Result<Project> {
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

    pub(crate) fn dir(&self) -> PathBuf {
        self.root.join(&self.slug)
    }

    pub(crate) fn project_md(&self) -> PathBuf {
        self.dir().join("PROJECT.md")
    }

    pub(crate) fn state_dir(&self) -> PathBuf {
        self.dir().join(".state")
    }

    pub(crate) fn record_dir(&self, kind: &str) -> PathBuf {
        self.state_dir().join(kind)
    }

    pub(crate) fn record_dir_for_write(&self, kind: &str) -> Result<PathBuf> {
        let dir = self.record_dir(kind);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        Ok(dir)
    }

    pub(crate) fn record_file(&self, name: &str) -> PathBuf {
        self.state_dir().join(name)
    }

    pub(crate) fn record_file_for_write(&self, name: &str) -> Result<PathBuf> {
        Ok(self.record_file(name))
    }

    /// The canonical folder (symlinks resolved): the key of the project's
    pub(crate) fn canonical_dir(&self) -> PathBuf {
        std::fs::canonicalize(self.dir()).unwrap_or_else(|_| self.dir())
    }

    /// Takes the per-project lock. The lock file is opened without creating
    /// parent directories, and the project is re-checked afterwards, so a
    /// `delete` that lands mid-operation cannot be resurrected by a writer.
    pub(crate) fn lock(&self) -> Result<ProjectLock> {
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

    pub(crate) fn read_project_md(&self) -> Result<(Settings, String)> {
        let text = std::fs::read_to_string(self.project_md())
            .with_context(|| format!("could not read {}", self.project_md().display()))?;
        parse_project_md(&text)
    }

    pub(crate) fn status(&self) -> Status {
        read_json::<ProjectState>(&self.state_dir().join("project.json"))
            .unwrap_or_default()
            .status
    }

    pub(crate) fn set_status(&self, status: Status) -> Result<()> {
        let _lock = self.lock()?;
        let path = self.state_dir().join("project.json");
        let mut state = read_json::<ProjectState>(&path).unwrap_or_default();
        state.status = status;
        write_json(&path, &state)
    }

    pub(crate) fn coordinator(&self) -> Option<Coordinator> {
        read_json(&self.state_dir().join("coordinator.json"))
    }

    /// Read-modify-write of `coordinator.json` under the lock: re-reads the
    /// file, lets `change` touch only the fields its step owns, writes.
    pub(crate) fn update_coordinator(
        &self,
        change: impl FnOnce(&mut Coordinator),
    ) -> Result<Coordinator> {
        let _lock = self.lock()?;
        let mut record = self.coordinator().unwrap_or_default();
        change(&mut record);
        record.updated = now();
        write_json(&self.state_dir().join("coordinator.json"), &record)?;
        Ok(record)
    }
}

/// `ha machine hold <machine>`: new box starts are held until released
/// (SPEC-remote §2.4). The marker lives under the ADE root, not on the box.
pub(crate) fn machine_hold(root: &Path, machine: &str) -> Result<PathBuf> {
    let path = machine_hold_path(root, machine)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, b"")?;
    Ok(path)
}

/// Removes the hold. Returns whether one was present.
pub(crate) fn machine_release(root: &Path, machine: &str) -> Result<bool> {
    let path = machine_hold_path(root, machine)?;
    Ok(std::fs::remove_file(&path).is_ok())
}

pub(crate) fn machine_held(root: &Path, machine: &str) -> bool {
    machine_hold_path(root, machine).is_ok_and(|path| path.exists())
}

fn machine_hold_path(root: &Path, machine: &str) -> Result<PathBuf> {
    if machine.is_empty()
        || machine
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        bail!("`{machine}` is not a machine name");
    }
    Ok(root.join(".machines").join(format!("{machine}.hold")))
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    write_atomic(path, text.as_bytes())
}

/// SHA-256 of executable settings and standing rules.
pub(crate) fn policy_hash(config_dir: &Path) -> String {
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
pub(crate) fn launch_recipe(
    spec: &RoleSpec,
    attempt: u32,
    brief_hash: String,
    policy: String,
    role: &str,
) -> Launch {
    Launch {
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
        skill_hash: crate::thread::sha256_hex(crate::lane::skill_text(role).as_bytes()),
        ..Launch::default()
    }
}

/// `--env` values for `tab create`, including `HERDR_ADE_LAUNCH` (SPEC-ADE D4).
/// A box lane also gets the box PATH and its own `CARGO_TARGET_DIR`
/// (SPEC-remote §§3.3, 4.2).
pub(crate) fn tab_env(
    slug: &str,
    thread: &str,
    attempt: u32,
    brief_hash: &str,
    machine: Option<&crate::remote::MachineDeclaration>,
    spec: &RoleSpec,
) -> Vec<String> {
    let mut env = vec![format!(
        "HERDR_ADE_LAUNCH={slug}/{thread}/{attempt}/{brief_hash}"
    )];
    if let Some(machine) = machine {
        // These three values are the box binding, wrapper path and isolated
        // build folder. A recipe cannot replace them with Mac-side values.
        env.extend(
            spec.env
                .iter()
                .filter(|value| {
                    !["HERDR_ADE_LAUNCH", "PATH", "CARGO_TARGET_DIR"]
                        .iter()
                        .any(|key| value.starts_with(&format!("{key}=")))
                })
                .cloned(),
        );
        env.push(format!("PATH={}", machine.path));
        env.push(format!(
            "CARGO_TARGET_DIR={}/{slug}-{thread}",
            machine.build
        ));
    } else {
        // Keep the established local-lane argv unchanged.
        env.extend(spec.env.iter().cloned());
    }
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

/// `HERDR_ADE_LAUNCH=<project>/<thread>/<attempt>/<brief hash>` (SPEC-ADE
/// D4, D14), parsed. The thread is `coordinator` for a coordinator pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchEnv {
    pub(crate) project: String,
    pub(crate) thread: String,
    pub(crate) attempt: u32,
    pub(crate) brief_hash: String,
}

impl LaunchEnv {
    pub(crate) fn parse(value: &str) -> Option<LaunchEnv> {
        let mut parts = value.trim().split('/');
        let project = parts.next()?.to_string();
        let thread = parts.next()?.to_string();
        let attempt = parts.next()?.parse().ok()?;
        let brief_hash = parts.next()?.to_string();
        if parts.next().is_some()
            || project.is_empty()
            || thread.is_empty()
            || brief_hash.is_empty()
        {
            return None;
        }
        Some(LaunchEnv {
            project,
            thread,
            attempt,
            brief_hash,
        })
    }

    /// The pane's own value, `None` when unset or malformed.
    pub(crate) fn from_process() -> Option<LaunchEnv> {
        std::env::var("HERDR_ADE_LAUNCH")
            .ok()
            .and_then(|value| LaunchEnv::parse(&value))
    }
}

/// Slugs of the projects in `root`: folders that contain `PROJECT.md`. Entries
/// whose names start with a dot are ignored. A missing root has no projects.
pub(crate) fn list_slugs(root: &Path) -> Vec<String> {
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
pub(crate) fn parse_repo_arg(arg: &str) -> Repo {
    if let Some((path, machine)) = arg.rsplit_once('@') {
        let label_like = !machine.is_empty()
            && machine
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if label_like && !path.is_empty() {
            return Repo {
                path: path.to_string(),
                machine: Some(machine.to_string()),
                ..Repo::default()
            };
        }
    }
    Repo {
        path: arg.to_string(),
        machine: None,
        ..Repo::default()
    }
}

fn project_md_prefix(bytes: &[u8]) -> Result<&[u8]> {
    if !bytes.starts_with(b"+++\n") {
        bail!("PROJECT.md must start with a `+++` line");
    }
    let end = bytes[4..]
        .windows(5)
        .position(|window| window == b"\n+++\n")
        .map(|index| index + 4 + 5)
        .or_else(|| bytes.ends_with(b"\n+++").then_some(bytes.len()))
        .context("PROJECT.md front matter has no closing `+++` line")?;
    Ok(&bytes[..end])
}

fn markdown_item(id: &str, provenance: &str, text: &str) -> String {
    let text = text.trim().replace('\n', "\n  ");
    format!("- `{id}` ({provenance}): {text}\n")
}

fn note_provenance(row: &crate::note::Row) -> String {
    let authority = row
        .request
        .as_ref()
        .map(|request| format!("request:{request}"))
        .unwrap_or_else(|| "historical".into());
    if row.tasks.is_empty() {
        authority
    } else {
        format!("{authority}; {}", row.tasks.join(", "))
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn latest_history(project: &Project) -> Option<String> {
    let mut names: Vec<String> = std::fs::read_dir(project.state_dir().join("history"))
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names.pop().map(|name| format!(".state/history/{name}/"))
}

pub(crate) fn running_stage(
    thread: &crate::thread::Thread,
    events: &[crate::contracts::Event],
) -> String {
    use crate::thread::Status;
    // A sealed completion describes the attempt, not the current lifecycle.
    // In particular a resolved lane must not keep saying it awaits a round.
    if thread.status == Status::Resolved {
        return "resolved".into();
    }
    let completion = crate::round::latest_event(events, &thread.id, thread.attempt.max(1));
    if thread.status == Status::Failed {
        let reason = if thread.error.trim().is_empty() {
            completion
                .and_then(|event| event.payload.failed.as_ref())
                .map(|failed| failed.text.as_str())
                .unwrap_or("reason unknown")
        } else {
            &thread.error
        };
        return format!(
            "failed: {}",
            one_line(reason).chars().take(120).collect::<String>()
        );
    }
    if let Some(event) = completion {
        if let Some(failed) = &event.payload.failed {
            return format!(
                "failed: {}",
                one_line(&failed.text).chars().take(120).collect::<String>()
            );
        }
        if let Some(waiting) = &event.payload.waiting
            && event.id != thread.answered_waiting_event
        {
            return format!(
                "waiting: {}",
                one_line(&waiting.text)
                    .chars()
                    .take(120)
                    .collect::<String>()
            );
        }
        if event.payload.done.is_some() {
            return "done, waiting for a round".into();
        }
    }
    match thread.status {
        Status::Starting => "starting".into(),
        Status::Open if thread.prompt_pending => "starting".into(),
        Status::Open if thread.last_state == "blocked" || thread.last_group == "idle" => {
            "waiting at prompt".into()
        }
        Status::Open if thread.last_group == "working" || thread.last_state == "working" => {
            "working".into()
        }
        Status::Open => "state unknown".into(),
        Status::Failed | Status::Resolved => unreachable!(),
    }
}

fn page_body(project: &Project, settings: &Settings) -> String {
    let evidence = crate::task::EvidenceSnapshot::load(project);
    let mut out = String::from("# Project\n\n");
    out.push_str("## Goal and what Rolf gets\n\n");
    if settings.goal.trim().is_empty() {
        out.push_str("Goal: not written down.\n");
    } else {
        out.push_str(&format!("Goal: {}\n", settings.goal.trim()));
    }
    let mut plan = crate::plan::load(project).ok().flatten();
    if let Some(plan) = &mut plan {
        crate::plan::project_states_with_evidence(project, plan, &evidence);
        if plan.what_you_get.is_empty() && plan.does.is_empty() {
            out.push_str("What Rolf gets: not written down.\n");
        } else {
            out.push_str(&format!(
                "What Rolf gets: {} {}\n",
                plan.what_you_get.trim(),
                plan.does.trim()
            ));
        }
    } else {
        out.push_str("What Rolf gets: not written down.\n");
    }

    out.push_str("\n## Waiting on Rolf\n\n");
    let asks = crate::ask::open_asks(project);
    let events = evidence.events();
    let mut waiting = Vec::new();
    for ask in asks {
        waiting.push(format!(
            "- `{}` {} Choices: {}\n",
            ask.id,
            ask.question.trim(),
            ask.choices.join(" / ")
        ));
    }
    if waiting.is_empty() {
        out.push_str("None.\n");
    } else {
        out.extend(waiting);
    }

    if crate::talk::long_input_hold(project) {
        out.push_str("\nAutomated prompts have waited over 30 minutes for text in the coordinator's input line. They remain pending.\n");
    }
    out.push_str("\n## Running now\n\n");
    let threads: Vec<_> = crate::thread::list(project)
        .into_iter()
        .filter(|thread| thread.status != crate::thread::Status::Resolved)
        .collect();
    let rounds: Vec<_> = crate::round::list(project)
        .into_iter()
        .filter(|round| !round.phase.closed())
        .collect();
    if threads.is_empty() && rounds.is_empty() {
        out.push_str("None.\n");
    }
    for thread in threads {
        out.push_str(&format!(
            "- Thread `{}`: {} ({}){}\n",
            thread.id,
            thread.title.trim(),
            running_stage(&thread, events),
            if thread.follow_ups.iter().any(|follow_up| follow_up.state
                == crate::thread::FollowUpState::Queued
                && follow_up.attempt == thread.attempt.max(1))
            {
                " — follow-up queued"
            } else {
                ""
            }
        ));
        if let Some(event) = crate::round::latest_event(events, &thread.id, thread.attempt.max(1))
            && event.id != thread.answered_waiting_event
            && let Some(waiting) = &event.payload.waiting
        {
            out.push_str(&format!(
                "  waiting on coordinator: {}\n",
                one_line(&waiting.text)
            ));
        }
    }
    for round in rounds {
        out.push_str(&format!(
            "- Round `{}`: {} ({})\n",
            round.round,
            round.plain.trim(),
            format!("{:?}", round.phase).to_lowercase()
        ));
    }

    out.push_str("\n## Plan\n\n");
    match &plan {
        Some(plan) if !plan.steps.is_empty() => {
            for step in &plan.steps {
                out.push_str(&format!(
                    "- `{}` [{}] {}\n",
                    step.id,
                    step.state.word(),
                    step.text.trim()
                ));
                for sub in &step.subtasks {
                    out.push_str(&format!(
                        "  - `{}` [{}] {}\n",
                        sub.id,
                        sub.state.word(),
                        sub.text.trim()
                    ));
                }
            }
        }
        _ => out.push_str("No steps are written down.\n"),
    }

    let (views, errors) = crate::task::views_with_evidence(project, &evidence);
    out.push_str("\n## Open tasks\n\n");
    let open: Vec<_> = views
        .iter()
        .filter(|view| !view.terminal_with_evidence(project, &evidence))
        .collect();
    if open.is_empty() && errors.is_empty() {
        out.push_str("None.\n");
    }
    for view in &open {
        out.push_str(&format!(
            "- `{}` [{}] {} — next: {}\n",
            view.record.id,
            view.state.word(),
            one_line(&view.record.title),
            one_line(&view.next)
        ));
        if let Some(wait) = crate::task::active_wait(project, &view.record) {
            out.push_str(&format!(
                "  waits on {}: {}\n",
                wait.kind,
                one_line(&wait.target)
            ));
        }
    }
    for error in errors {
        out.push_str(&format!("- Unreadable task: {error:#}\n"));
    }

    let mut notes = crate::note::active_rows(project);
    crate::note::sort_newest_first(&mut notes);
    let applies = |row: &&crate::note::Row| {
        row.tasks.is_empty()
            || row
                .tasks
                .iter()
                .any(|task| open.iter().any(|view| view.record.id == *task))
    };

    out.push_str("\n## Task notes in force\n\n");
    let task_notes: Vec<_> = notes
        .iter()
        .filter(|row| row.kind == "task note")
        .filter(applies)
        .collect();
    if task_notes.is_empty() {
        out.push_str("None.\n");
    }
    for row in task_notes {
        out.push_str(&markdown_item(&row.id, &note_provenance(row), &row.text));
    }

    out.push_str("\n## Standing instructions in force\n\n");
    let instructions: Vec<_> = notes
        .iter()
        .filter(|row| row.kind == "standing instruction")
        .filter(applies)
        .collect();
    if instructions.is_empty() {
        out.push_str("None.\n");
    }
    for row in instructions {
        out.push_str(&markdown_item(&row.id, &note_provenance(row), &row.text));
    }

    out.push_str("\n## Facts in force\n\n");
    let facts: Vec<_> = notes
        .iter()
        .filter(|row| row.kind == "memory")
        .filter(applies)
        .collect();
    if facts.is_empty() {
        out.push_str("None.\n");
    }
    for row in facts {
        out.push_str(&markdown_item(&row.id, &note_provenance(row), &row.text));
    }

    out.push_str("\n## Recently finished or dropped tasks\n\n");
    let mut finished: Vec<_> = views
        .iter()
        .filter(|view| view.terminal_with_evidence(project, &evidence))
        .collect();
    finished.sort_by(|a, b| b.record.created.cmp(&a.record.created));
    if finished.is_empty() {
        out.push_str("None.\n");
    }
    for view in finished {
        out.push_str(&format!(
            "- `{}` [{}] {}",
            view.record.id,
            view.state.word(),
            view.record.title.trim()
        ));
        if view.state == crate::task::State::Dropped
            && let Some(evidence) = view.record.dropped.last()
        {
            out.push_str(&format!(" — dropped: {}", evidence.reason.trim()));
        }
        out.push('\n');
        for attempt in &view.record.attempts {
            if let Ok(thread) = crate::thread::load(project, attempt)
                && let Some(report) = crate::thread::report_reference(project, &thread)
            {
                let label = if crate::thread::sealed_report_path(project, &thread).is_some() {
                    "Final report"
                } else {
                    "Historical report (not completion)"
                };
                out.push_str(&format!("  {label} (`{attempt}`): `{report}`\n"));
            }
        }
    }

    let history = latest_history(project).unwrap_or_else(|| "the hidden .state folder".into());
    out.push_str(&format!(
        "\n---\nView rebuilt at {}; history is kept in {}.\n",
        now(),
        history
    ));
    out
}

struct PageLock {
    _file: File,
}

fn page_lock(project: &Project) -> Result<PageLock> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(project.state_dir().join("page.lock"))?;
    file.lock()?;
    Ok(PageLock { _file: file })
}

/// Rebuilds only the binary-owned body. The coordinator-owned front matter is
/// compared immediately before the atomic replacement, so an edit is never
/// overwritten with an older copy.
pub(crate) fn refresh_page(project: &Project) -> Result<()> {
    let _lock = page_lock(project)?;
    let before = std::fs::read(project.project_md())?;
    let prefix = project_md_prefix(&before)?.to_vec();
    let (settings, _) = parse_project_md(std::str::from_utf8(&before)?)?;
    let body = page_body(project, &settings);
    let current = std::fs::read(project.project_md())?;
    if project_md_prefix(&current)? != prefix {
        bail!("project_page_changed: PROJECT.md front matter changed while its page was rebuilt");
    }
    let mut page = prefix;
    page.push(b'\n');
    page.extend_from_slice(body.as_bytes());
    write_atomic(&project.project_md(), &page)
}

/// Creates the folder and skeleton files. The only code path that creates a
/// project's directories. Fails if the slug exists.
pub(crate) fn create(root: &Path, name: &str, goal: &str, repos: Vec<Repo>) -> Result<Project> {
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
            None => {
                let path = std::fs::canonicalize(&repo.path)
                    .or_else(|_| std::path::absolute(&repo.path))
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| repo.path.clone());
                Repo { path, ..repo }
            }
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
    // `.state` is part of the project itself: its project record and lock are
    // needed immediately. Every content folder is created by its first writer.
    std::fs::create_dir(dir.join(".state"))?;
    write_json(
        &project.state_dir().join("project.json"),
        &ProjectState::default(),
    )?;
    // PROJECT.md last: a folder without it is not a project, so a half-made
    // skeleton is never picked up by `list` or the ticker.
    write_atomic(
        &project.project_md(),
        format!("+++\n# Repository gates: {{ command = \"...\", paths = [\"src/**\"] }}. Omit paths to always run.\n# Paths are repository-relative: * and ? match within a segment; ** is a whole directory segment.\n{front}+++\n").as_bytes(),
    )?;
    refresh_page(&project)?;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_lane_stage_uses_recorded_observation_and_sealed_events() {
        let mut lane = crate::thread::Thread {
            id: "t-0001".into(),
            status: crate::thread::Status::Starting,
            ..Default::default()
        };
        assert_eq!(running_stage(&lane, &[]), "starting");
        lane.status = crate::thread::Status::Open;
        lane.last_state = "working".into();
        assert_eq!(running_stage(&lane, &[]), "working");
        lane.last_state = "blocked".into();
        assert_eq!(running_stage(&lane, &[]), "waiting at prompt");
        let mut event = crate::contracts::Event {
            id: "e1".into(),
            op: "op".into(),
            thread: lane.id.clone(),
            attempt: 1,
            round: None,
            recipient: Default::default(),
            created: "2026-01-01T00:00:00Z".into(),
            payload: Default::default(),
        };
        event.payload.done = Some(Default::default());
        assert_eq!(
            running_stage(&lane, &[event.clone()]),
            "done, waiting for a round"
        );
        event.payload.done = None;
        event.payload.failed = Some(crate::contracts::WaitingPayload {
            text: "Provider stopped".into(),
            ..Default::default()
        });
        assert_eq!(
            running_stage(&lane, &[event.clone()]),
            "failed: Provider stopped"
        );
        lane.status = crate::thread::Status::Resolved;
        assert_eq!(running_stage(&lane, &[event.clone()]), "resolved");
        lane.status = crate::thread::Status::Failed;
        assert_eq!(running_stage(&lane, &[event]), "failed: Provider stopped");
    }

    #[test]
    fn finished_history_is_not_cut_off_before_context_full_can_show_it() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let dir = project.record_dir_for_write("tasks").unwrap();
        for index in 0..11 {
            let task = crate::task::Task {
                id: format!("job-{index:04}"),
                title: format!("Finished {index}"),
                authority: vec!["request:q-1".into()],
                acceptance: vec!["Done".into()],
                created: format!("2020-01-{:02}T00:00:00Z", index + 1),
                dropped: vec![crate::task::DropEvidence {
                    at: "2020-02-01T00:00:00Z".into(),
                    reason: "Complete".into(),
                }],
                ..Default::default()
            };
            std::fs::write(
                dir.join(format!("{}.toml", task.id)),
                toml::to_string(&task).unwrap(),
            )
            .unwrap();
        }
        refresh_page(&project).unwrap();
        let (_, page) = project.read_project_md().unwrap();
        assert!(page.contains("`job-0000` [dropped]"));
        assert!(page.contains("`job-0010` [dropped]"));
    }

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
    fn create_writes_only_the_required_skeleton_and_refuses_a_second_time() {
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
        assert!(project.state_dir().is_dir());
        for optional in ["tasks", "scratch", "threads", "inbox", "library"] {
            assert!(!project.dir().join(optional).exists(), "{optional}");
        }
        for old in ["MEMORY.md", "memory", "TASKS.md"] {
            assert!(!project.dir().join(old).exists(), "{old}");
        }
        let (settings, body) = project.read_project_md().unwrap();
        assert_eq!(settings.name, "Demo");
        assert_eq!(settings.goal, "Ship \"it\"");
        assert_eq!(
            settings.repos,
            vec![
                Repo {
                    path: "/srv/app".into(),
                    machine: Some("box".into()),
                    ..Repo::default()
                },
                Repo {
                    path: "/no/such/repo".into(),
                    machine: None,
                    ..Repo::default()
                },
            ]
        );
        assert!(body.starts_with("# Project"));
        assert_eq!(project.status(), Status::Active);
        assert!(create(&root, "demo", "", vec![]).is_err());
    }

    #[test]
    fn lane_waits_belong_to_the_lane_not_waiting_on_rolf() {
        let root = tempfile::tempdir().unwrap();
        let project = create(root.path(), "demo", "", vec![]).unwrap();
        let lane = crate::thread::allocate(&project, |thread| {
            thread.status = crate::thread::Status::Open;
            thread.attempt = 1;
            thread.title = "Check deployment".into();
        })
        .unwrap();
        crate::events::seal_create_if_absent(
            &project,
            &crate::contracts::Event {
                id: "wait-1".into(),
                op: "wait-1".into(),
                thread: lane.id.clone(),
                attempt: 1,
                round: None,
                recipient: crate::contracts::Recipient::default(),
                created: now(),
                payload: crate::contracts::EventPayload {
                    waiting: Some(crate::contracts::WaitingPayload {
                        text: "Coordinator, choose a build".into(),
                        ..crate::contracts::WaitingPayload::default()
                    }),
                    ..crate::contracts::EventPayload::default()
                },
            },
        )
        .unwrap();
        let page = page_body(&project, &Settings::default());
        let rolf = page
            .split_once("## Waiting on Rolf\n\n")
            .unwrap()
            .1
            .split_once("\n## Running now")
            .unwrap()
            .0;
        assert_eq!(rolf, "None.\n");
        assert!(page.contains("waiting on coordinator: Coordinator, choose a build"));
    }

    #[test]
    fn project_page_keeps_each_open_task_to_one_line() {
        let root = tempfile::tempdir().unwrap();
        let project = create(root.path(), "demo", "", vec![]).unwrap();
        crate::talk::append(
            &project,
            None,
            crate::talk::Entry::Rolf {
                request: "q-1".into(),
                text: "Keep the project page short.".into(),
                answer: None,
            },
        )
        .unwrap();
        let task = crate::task::add(
            &project,
            "Keep each open task on one line.",
            vec!["request:q-1".into()],
            vec!["The detailed acceptance condition stays in the task record.".into()],
            None,
            None,
        )
        .unwrap();
        crate::note::add(
            &project,
            crate::note::Kind::Instruction,
            "Keep this task-specific instruction on the current page.",
            "q-1",
            None,
            vec![task.id.clone()],
        )
        .unwrap();

        let page = std::fs::read_to_string(project.project_md()).unwrap();
        let open = page
            .split_once("## Open tasks\n\n")
            .unwrap()
            .1
            .split_once("\n## Task notes in force")
            .unwrap()
            .0;
        assert_eq!(
            open.lines().filter(|line| line.contains(&task.id)).count(),
            1
        );
        assert!(open.contains(" — next: verify 1 acceptance condition(s)"));
        assert!(!open.contains("detailed acceptance condition"));
        assert!(page.contains("Keep this task-specific instruction on the current page."));
    }

    #[test]
    fn page_reads_each_event_once_even_with_plan_and_multiple_tasks() {
        let root = tempfile::tempdir().unwrap();
        let project = create(root.path(), "demo", "", vec![]).unwrap();
        let tasks = project.record_dir_for_write("tasks").unwrap();
        let events = project.record_dir_for_write("events").unwrap();
        let mut ids = Vec::new();
        for index in 1..=3 {
            let thread = crate::thread::allocate(&project, |thread| {
                thread.repo = "/repo".into();
                thread.base = "base".into();
            })
            .unwrap();
            let id = format!("job-{index:04}");
            ids.push(id.clone());
            let task = crate::task::Task {
                id: id.clone(),
                title: format!("Task {index}"),
                authority: vec!["request:q-1".into()],
                acceptance: vec!["Done".into()],
                repo: Some("/repo".into()),
                attempts: vec![thread.id.clone()],
                ..crate::task::Task::default()
            };
            std::fs::write(
                tasks.join(format!("{id}.toml")),
                toml::to_string(&task).unwrap(),
            )
            .unwrap();
            let event = crate::contracts::Event {
                id: format!("{}-1-1", thread.id),
                op: format!("{}-1-1", thread.id),
                thread: thread.id,
                attempt: 1,
                round: None,
                recipient: crate::contracts::Recipient::default(),
                created: now(),
                payload: crate::contracts::EventPayload::default(),
            };
            std::fs::write(
                events.join(format!("{}.toml", event.id)),
                toml::to_string(&event).unwrap(),
            )
            .unwrap();
        }
        let plan = crate::contracts::Plan {
            steps: vec![crate::contracts::PlanStep {
                id: "s-1".into(),
                tasks: ids,
                ..crate::contracts::PlanStep::default()
            }],
            ..crate::contracts::Plan::default()
        };
        std::fs::write(
            crate::plan::plan_path(&project),
            toml::to_string(&plan).unwrap(),
        )
        .unwrap();
        let reads = crate::events::count_event_reads(|| refresh_page(&project).unwrap());
        assert_eq!(reads, 3, "page must load each of the three events once");
    }

    #[test]
    fn page_lists_subtasks_beneath_their_step() {
        let root = tempfile::tempdir().unwrap();
        let project = create(root.path(), "demo", "", vec![]).unwrap();
        let plan = crate::contracts::Plan {
            steps: vec![crate::contracts::PlanStep {
                id: "s-1".into(),
                text: "Build the screen".into(),
                subtasks: vec![crate::contracts::PlanStep {
                    id: "s-2".into(),
                    text: "Draw the list".into(),
                    ..crate::contracts::PlanStep::default()
                }],
                ..crate::contracts::PlanStep::default()
            }],
            ..crate::contracts::Plan::default()
        };
        std::fs::write(
            crate::plan::plan_path(&project),
            toml::to_string(&plan).unwrap(),
        )
        .unwrap();
        refresh_page(&project).unwrap();
        let (_, body) = project.read_project_md().unwrap();
        assert!(
            body.contains("## Plan\n\n- `s-1` [left] Build the screen\n  - `s-2` [left] Draw the list\n"),
            "{body}"
        );
    }

    #[test]
    fn page_rewrite_keeps_front_matter_bytes_and_never_reads_its_body_as_a_note() {
        let root = tempfile::tempdir().unwrap();
        let project = create(root.path(), "demo", "First goal.", vec![]).unwrap();
        let before = std::fs::read(project.project_md()).unwrap();
        let prefix = project_md_prefix(&before).unwrap();
        let edited = String::from_utf8(prefix.to_vec()).unwrap().replace(
            "goal = \"First goal.\"",
            "# kept exactly\ngoal = \"Edited goal.\"",
        );
        std::fs::write(
            project.project_md(),
            format!("{edited}\n# Hand-written body must disappear.\n"),
        )
        .unwrap();

        refresh_page(&project).unwrap();

        let after = std::fs::read(project.project_md()).unwrap();
        assert_eq!(project_md_prefix(&after).unwrap(), edited.as_bytes());
        assert!(
            !String::from_utf8(after)
                .unwrap()
                .contains("Hand-written body")
        );
        assert!(crate::note::active_rows(&project).is_empty());
    }

    #[test]
    fn front_matter_parsing() {
        let (settings, body) =
            parse_project_md("+++\nname = \"X\"\n+++\n\nBody\n+++\nmore\n").unwrap();
        assert_eq!(settings.name, "X");
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
    fn new_project_does_not_write_removed_settings() {
        let root = tempfile::tempdir().unwrap();
        let project = create(root.path(), "demo", "", vec![]).unwrap();
        let text = std::fs::read_to_string(project.project_md()).unwrap();
        let front = project_md_front(&text).unwrap();
        assert!(removed_project_keys(front).is_empty(), "{front}");
        assert!(!front.contains("max_parallel_threads"), "{front}");
        assert!(!front.contains("talk"), "{front}");
        let (settings, _) = parse_project_md(&text).unwrap();
        assert_eq!(settings.name, "Demo");
    }

    #[test]
    fn project_model_overrides_are_refused() {
        let front = "[roles.lane]\nkind = \"claude\"";
        assert!(
            parse_project_md(&format!("+++\n{front}\n+++\n"))
                .unwrap_err()
                .to_string()
                .contains("roles_removed")
        );
    }

    #[test]
    fn default_timeout_and_dsh_env() {
        let spec = RoleSpec {
            kind: "dsh".into(),
            ..RoleSpec::default()
        };
        let recipe = launch_recipe(&spec, 1, "bh".into(), "ph".into(), "lane");
        assert_eq!(recipe.ready_timeout_ms, 20_000);
        let env = tab_env("demo", "t-0001", 1, "abcd", None, &spec);
        assert!(
            env.iter()
                .any(|e| e == "HERDR_ADE_LAUNCH=demo/t-0001/1/abcd")
        );
        assert!(env.iter().any(|e| e.starts_with("DSH_PERMISSION_MODE=")));
    }
}
