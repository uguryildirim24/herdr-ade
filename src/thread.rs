//! Thread records, ids, briefs, groups and the copy home.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::herdr::{Agent, Pane, ready_state};
use crate::project::{self, Project, slugify, write_atomic};
use crate::runner::{Cmd, Runner};

pub(crate) const STARTING_TIMEOUT_SECS: i64 = 300;
pub(crate) const BLOCKED_DEBOUNCE_SECS: i64 = 30;
const NOT_READY_SECS: i64 = 60;
/// The brief's memory-note budget. `compose_brief` stops adding dated notes
/// past this, and `ha doctor` / `ha context` warn at it: the warning fires at
/// the point where a brief starts dropping notes, and 32k is a small enough
/// share of a lane's context to prune before it costs real tokens.
pub(crate) const MEMORY_CAP_CHARS: usize = 32_000;
const LIBRARY_CAP_KB: u64 = 50 * 1024;
pub(crate) const MAX_LAUNCH_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    #[default]
    Starting,
    Open,
    Failed,
    Resolved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FollowUpState {
    #[default]
    Queued,
    Uncertain,
    Superseded,
    Cancelled,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub(crate) struct FollowUp {
    pub(crate) attempt: u32,
    pub(crate) text: String,
    pub(crate) state: FollowUpState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    #[default]
    Worktree,
    Tab,
    Adopted,
}

/// `threads/<id>.toml`. An empty string means "not set". Paths are stored as
/// they are on the thread's own machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub(crate) struct Thread {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) status: Status,
    pub(crate) error: String,
    pub(crate) prompt_pending: bool,
    /// Attempt-bound follow-ups accepted while the first brief is pending.
    /// Terminal dispositions remain visible instead of crossing attempts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) follow_ups: Vec<FollowUp>,
    pub(crate) launch_attempts: u32,
    pub(crate) failure_event: String,
    pub(crate) last_failure: String,
    /// Classification of the current failure evidence. Old records load as
    /// unknown rather than guessing from prose.
    #[serde(default)]
    pub(crate) failure_class: crate::contracts::FailureClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider_failure_kind: Option<String>,
    pub(crate) escalation_pending: bool,
    pub(crate) kind: Kind,
    pub(crate) repo: String,
    pub(crate) origin: String,
    pub(crate) branch: String,
    pub(crate) base: String,
    pub(crate) machine: String,
    /// The stable saved-profile id the lane resolved to (SPEC-remote §4.1).
    /// Empty on a local lane; a renamed label does not change it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) machine_id: String,
    pub(crate) worktree_path: String,
    pub(crate) thread_dir: String,
    pub(crate) workspace_id: String,
    pub(crate) tab_id: String,
    pub(crate) pane_id: String,
    pub(crate) agent: String,
    pub(crate) agent_name: String,
    pub(crate) cwd: String,
    pub(crate) created: String,
    pub(crate) updated: String,
    pub(crate) last_state: String,
    pub(crate) last_state_change: String,
    /// When and where the latest successful remote observation was made.
    /// Empty on historical and local records.
    pub(crate) last_observed: String,
    pub(crate) observation_source: String,
    /// Latest attempted remote check, successful or not.
    pub(crate) observation_attempted: String,
    /// A failed check never overwrites the last successful observation.
    pub(crate) observation_error: String,
    pub(crate) last_group: String,
    pub(crate) report_hash: String,
    pub(crate) last_report_change: String,
    /// Incomplete report/library copy, kept with the report it describes.
    pub(crate) copy_notes: Vec<String>,
    /// Current pull request validation problem (not a second inbox record).
    pub(crate) pr_note: String,
    pub(crate) lineage_mismatch: bool,
    pub(crate) acked_report_hash: String,
    pub(crate) pr: String,
    pub(crate) pr_state: String,
    pub(crate) pr_review: String,
    pub(crate) pr_summary: Option<crate::pr::Summary>,
    pub(crate) resolved_reason: String,
    /// Why recovery deliberately stopped this thread. Empty on historical and
    /// normally resolved records.
    pub(crate) cancellation_reason: String,
    /// Cleanup still owes the same final-copy, pane/tab, and folder work that
    /// `thread resolve` performs. The ticker retries it instead of making a
    /// landed round wait on an external session.
    pub(crate) cleanup_pending: bool,
    /// The resolved reason to record after automatic cleanup succeeds.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) cleanup_reason: String,
    /// ADE role name (SPEC-ADE D2). Empty on a pre-ADE thread.
    pub(crate) role: String,
    /// Round this reviewer was started for. Empty on lanes and historical
    /// reviewer records created before this identity was stored.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) review_round: String,
    pub(crate) launch: crate::contracts::Launch,
    pub(crate) attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) partial: Option<String>,
    pub(crate) bootstrap: String,
    pub(crate) plain: String,
    pub(crate) identity: crate::contracts::IdentityBinding,
    #[serde(default)]
    pub(crate) passive: bool,
}

impl Thread {
    pub(crate) fn is_remote(&self) -> bool {
        !self.machine.is_empty()
    }

    /// The stable saved-profile id used for machine routing. The label is
    /// display-only and may be renamed without changing an attempt's identity.
    pub(crate) fn machine_route(&self) -> &str {
        if self.machine_id.is_empty() {
            &self.machine
        } else {
            &self.machine_id
        }
    }

    pub(crate) fn report_path(&self) -> String {
        format!("{}/report.md", self.thread_dir)
    }

    fn library_path(&self) -> String {
        format!("{}/library", self.thread_dir)
    }
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
    let digits = id.strip_prefix("t-").unwrap_or("");
    if digits.len() < 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
        bail!("`{id}` is not a thread id (expected the form t-0001)");
    }
    Ok(())
}

pub(crate) fn threads_dir(project: &Project) -> PathBuf {
    project.record_dir("threads")
}

pub(crate) fn threads_dir_for_write(project: &Project) -> Result<PathBuf> {
    project.record_dir_for_write("threads")
}

fn record_path(project: &Project, id: &str) -> PathBuf {
    threads_dir(project).join(format!("{id}.toml"))
}

pub(crate) fn task_path(project: &Project, id: &str) -> PathBuf {
    threads_dir(project).join(format!("{id}.task.md"))
}

pub(crate) fn task_path_for_write(project: &Project, id: &str) -> Result<PathBuf> {
    Ok(threads_dir_for_write(project)?.join(format!("{id}.task.md")))
}

pub(crate) fn home_report_path(project: &Project, id: &str) -> PathBuf {
    threads_dir(project).join(format!("{id}.md"))
}

fn regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// The one durable final report for this attempt, when its sealed
/// content-addressed artifact is present and valid.
pub(crate) fn sealed_report_path(project: &Project, thread: &Thread) -> Option<PathBuf> {
    let attempt = thread.attempt.max(1);
    let (path, hash) = crate::events::list(project)
        .into_iter()
        .filter(|event| event.thread == thread.id && event.attempt == attempt)
        .filter_map(|event| {
            let done = event.payload.done?;
            Some((event.created, event.id, done.artifact))
        })
        .max_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)))
        .map(|(_, _, hash)| (crate::events::artifact_path(project, &hash), hash))?;
    (regular_file(&path) && std::fs::read(&path).is_ok_and(|bytes| sha256_hex(&bytes) == hash))
        .then_some(path)
}

/// The sealed artifact, or an unmatched historical home copy kept readable
/// without treating it as completion evidence.
pub(crate) fn final_report_path(project: &Project, thread: &Thread) -> Option<PathBuf> {
    sealed_report_path(project, thread).or_else(|| {
        let historical = home_report_path(project, &thread.id);
        regular_file(&historical).then_some(historical)
    })
}

fn path_reference(project: &Project, path: PathBuf) -> String {
    path.strip_prefix(project.dir())
        .unwrap_or(&path)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn sealed_report_reference(project: &Project, thread: &Thread) -> Option<String> {
    sealed_report_path(project, thread).map(|path| path_reference(project, path))
}

pub(crate) fn report_reference(project: &Project, thread: &Thread) -> Option<String> {
    let path = final_report_path(project, thread)?;
    Some(path_reference(project, path))
}

pub(crate) fn load(project: &Project, id: &str) -> Result<Thread> {
    validate_id(id)?;
    let path = record_path(project, id);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("no thread `{id}` in `{}`", project.slug))?;
    toml::from_str(&text).with_context(|| format!("{} does not parse", path.display()))
}

pub(crate) fn list_with_errors(project: &Project) -> (Vec<Thread>, Vec<anyhow::Error>) {
    let entries = match std::fs::read_dir(threads_dir(project)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), Vec::new());
        }
        Err(error) => return (Vec::new(), vec![error.into()]),
    };
    let mut threads = Vec::new();
    let mut errors = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(error.into());
                continue;
            }
        };
        let Ok(name) = entry.file_name().into_string() else {
            errors.push(anyhow::anyhow!("a thread record name is not UTF-8"));
            continue;
        };
        let Some(id) = name.strip_suffix(".toml") else {
            continue;
        };
        match load(project, id) {
            Ok(thread) => threads.push(thread),
            Err(error) => errors.push(error),
        }
    }
    threads.sort_by(|a, b| a.id.cmp(&b.id));
    (threads, errors)
}

pub(crate) fn list(project: &Project) -> Vec<Thread> {
    list_with_errors(project).0
}

fn write_record(project: &Project, thread: &Thread) -> Result<()> {
    let dir = threads_dir_for_write(project)?;
    write_atomic(
        &dir.join(format!("{}.toml", thread.id)),
        toml::to_string(thread)?.as_bytes(),
    )
}

/// Read-modify-write under the project lock: re-reads the record, lets `change`
/// touch only the fields its step owns, writes.
pub(crate) fn update(
    project: &Project,
    id: &str,
    change: impl FnOnce(&mut Thread),
) -> Result<Thread> {
    update_checked(project, id, |thread| {
        change(thread);
        Ok(())
    })
}

/// Compare-and-change for transitions which awaited external work.
pub(crate) fn update_checked(
    project: &Project,
    id: &str,
    change: impl FnOnce(&mut Thread) -> Result<()>,
) -> Result<Thread> {
    let _lock = project.lock()?;
    let mut thread = load(project, id)?;
    let before = thread.clone();
    change(&mut thread)?;
    if thread.attempt != before.attempt {
        // A receipt proves one exact attempt; a replacement must earn its own.
        thread.bootstrap.clear();
        for follow_up in &mut thread.follow_ups {
            if follow_up.attempt == before.attempt.max(1)
                && follow_up.state == FollowUpState::Queued
            {
                follow_up.state = FollowUpState::Superseded;
            }
        }
    }
    if thread.status == Status::Resolved && before.status != Status::Resolved {
        let disposition = if thread.resolved_reason == "cancelled" {
            FollowUpState::Cancelled
        } else {
            FollowUpState::Closed
        };
        for follow_up in &mut thread.follow_ups {
            if follow_up.state == FollowUpState::Queued {
                follow_up.state = disposition;
            }
        }
    }
    thread.updated = project::now();
    write_record(project, &thread)?;
    observe_transition(project, &before, &thread);
    Ok(thread)
}

/// Record transitions, not polls: an unchanged blocked/error state is one
/// occurrence even if the ticker sees it a hundred times.
fn observe_transition(project: &Project, before: &Thread, after: &Thread) {
    // The failure ledger is for harness defects, not provider outages, gone
    // processes, failed work, or ordinary retries. Unknown startup breakage is
    // the only thread transition that supplies evidence of a harness failure.
    if after.status == Status::Failed
        && before.status != Status::Failed
        && after.failure_class == crate::contracts::FailureClass::Unknown
    {
        crate::ledger::observe(project, "thread-error", &after.id, &after.error);
        if after.launch_attempts == 0 && !after.launch.kind.is_empty() {
            crate::ledger::observe(
                project,
                "launch-not-attempted",
                &after.id,
                &format!("launch_attempts = 0: {}", after.error),
            );
        }
    }
}

/// Allocates the next id under the project lock and writes the first record.
pub(crate) fn allocate(project: &Project, fill: impl FnOnce(&mut Thread)) -> Result<Thread> {
    let _lock = project.lock()?;
    let next = list(project)
        .iter()
        .filter_map(|t| t.id.strip_prefix("t-")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let mut thread = Thread {
        id: format!("t-{next:04}"),
        status: Status::Starting,
        created: project::now(),
        ..Thread::default()
    };
    fill(&mut thread);
    thread.updated = thread.created.clone();
    let path = record_path(project, &thread.id);
    if path.exists() {
        bail!("thread id {} is already taken", thread.id);
    }
    write_record(project, &thread)?;
    Ok(thread)
}

pub(crate) fn branch_name(slug: &str, id: &str, title: &str) -> String {
    let title = slugify(title);
    if title.is_empty() {
        format!("hp/{slug}/{id}")
    } else {
        format!("hp/{slug}/{id}-{title}")
    }
}

pub(crate) fn agent_name(slug: &str, id: &str) -> String {
    format!("hp-{slug}-{id}")
}

/// `<agent working directory>/.herdr-project/<slug>-<id>`, for every kind.
pub(crate) fn thread_dir(cwd: &str, slug: &str, id: &str) -> String {
    format!("{}/.herdr-project/{slug}-{id}", cwd.trim_end_matches('/'))
}

/// The one line the agent is prompted with. Nothing from outside is ever
/// placed in a prompt. A launched lane's first instruction is its role skill,
/// which is also the bootstrap receipt. Code lanes read the frozen brief from
/// their ignored `.herdr-project` folder; it is a content-addressed project
/// artifact rather than a commit in the product repository. Project-owned
/// lanes keep `brief.md` in their own git folder.
pub(crate) fn launch_prompt(prefix: &str, slug: &str, t: &Thread) -> String {
    let id = &t.id;
    let role = if t.role.is_empty() { "lane" } else { &t.role };
    let continuation = if t.last_failure.is_empty() {
        String::new()
    } else {
        format!(
            " Continue the preserved worktree; do not reset or discard changes. The previous attempt reported this failure: {}.",
            serde_json::to_string(&t.last_failure).unwrap_or_default()
        )
    };
    if t.is_remote() {
        return format!(
            "Run {prefix} skill {role}, then read .herdr-project/{slug}-{id}/brief.md and do what it says. You run on the cloud box named `{}`; finish with `ha done`, never with a parent prompt.{continuation}",
            t.machine
        );
    }
    let prompt = match t.kind {
        Kind::Worktree if !t.is_remote() => format!(
            "Run {prefix} skill {role}, then read .herdr-project/{slug}-{id}/brief.md and do what it says."
        ),
        Kind::Tab => {
            format!("Run {prefix} skill {role}, then read brief.md and do what it says.")
        }
        Kind::Adopted if t.repo.is_empty() && !t.thread_dir.is_empty() => format!(
            "Read {}/brief.md and do what it says. Work and commit in {}.",
            t.thread_dir, t.worktree_path
        ),
        _ => format!("Read .herdr-project/{slug}-{id}/brief.md and do what it says."),
    };
    format!("{prompt}{continuation}")
}

// ---------------------------------------------------------------- briefs

/// The line that opens every thread skill: the skills write `hp`, and this
/// names the prefix `hp` stands for.
/// Stores immutable prose alongside the project's machine records. The hash
/// is both its filename and the receipt carried by the thread record.
pub(crate) fn store_artifact(project: &Project, bytes: &[u8]) -> Result<String> {
    let hash = sha256_hex(bytes);
    let dir = project.state_dir().join("artifacts");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(&hash);
    match std::fs::read(&path) {
        Ok(existing) if existing == bytes => {}
        Ok(_) => bail!("artifact_conflict: {} has different bytes", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::project::write_atomic(&path, bytes)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(hash)
}

pub(crate) fn artifact(project: &Project, hash: &str) -> Result<Vec<u8>> {
    let path = project.state_dir().join("artifacts").join(hash);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("brief_artifact_missing: {}", path.display()))?;
    if sha256_hex(&bytes) != hash {
        bail!("brief_artifact_mismatch: {}", path.display());
    }
    Ok(bytes)
}

pub(crate) fn commands_line(prefix: &str) -> String {
    format!("Commands: `{prefix}`. Every `hp` command below means that prefix.\n\n")
}

/// A brief read without `hp skill` (adopted, remote) carries the lane skill.
pub(crate) fn with_lane_skill(prefix: &str, brief: &str) -> String {
    format!(
        "{}{}\n\n{brief}",
        commands_line(prefix),
        include_str!("../skill/LANE.md").trim_end()
    )
}

/// The largest dated-note payload carried by any current task's brief.
/// `total_chars` includes notes the cap would drop, so the warning describes
/// the attempted payload rather than only what fits.
#[derive(Debug, Clone, Default)]
pub(crate) struct MemoryUse {
    notes: Vec<(String, String)>,
    total_chars: usize,
}

impl MemoryUse {
    fn from_rows<'a>(rows: impl Iterator<Item = &'a crate::note::Row>) -> Self {
        let notes: Vec<_> = rows
            .filter(|row| brief_carries_memory(row))
            .map(|row| (row.id.clone(), render_brief_row(row)))
            .collect();
        let total_chars = notes
            .iter()
            .map(|(id, text)| memory_block(id, text).chars().count())
            .sum();
        Self { notes, total_chars }
    }

    fn over_budget(&self) -> bool {
        self.total_chars > MEMORY_CAP_CHARS
    }

    /// The `ha doctor` / `ha context` warning, or `None` when every brief fits.
    pub(crate) fn warning(&self) -> Option<String> {
        if !self.over_budget() {
            return None;
        }
        let parts = self
            .notes
            .iter()
            .map(|(id, text)| format!("{id} {}", memory_block(id, text).chars().count()))
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "memory over budget: {} of {} characters ({parts}); replace stale dated notes",
            self.total_chars, MEMORY_CAP_CHARS
        ))
    }
}

fn render_brief_row(row: &crate::note::Row) -> String {
    let provenance = match (&row.at, &row.request) {
        (Some(at), Some(request)) => {
            format!("{} · request:{}", &at[..at.len().min(10)], request)
        }
        _ => "undated".into(),
    };
    format!("<!-- {} · {} -->\n{}", row.id, provenance, row.text.trim())
}

fn brief_carries_instruction(row: &crate::note::Row) -> bool {
    row.kind == "standing instruction" && row.at.is_some() && row.request.is_some()
}

fn brief_carries_memory(row: &crate::note::Row) -> bool {
    row.kind == "memory" && row.at.is_some() && row.request.is_some()
}

fn memory_block(id: &str, text: &str) -> String {
    format!("\n## {id}\n\n{}\n", text.trim())
}

/// Measures the largest note payload a current brief would attempt to carry.
/// Task scopes are measured separately because no brief receives notes for a
/// different task. Historical task notes remain scoped to their task.
pub(crate) fn memory_use(project: &Project) -> MemoryUse {
    let rows = crate::note::active_rows(project);
    let mut scopes = std::collections::BTreeSet::from([None]);
    for task in rows.iter().flat_map(|row| &row.tasks) {
        scopes.insert(Some(task.as_str()));
    }
    scopes
        .into_iter()
        .map(|task| {
            let mut applicable: Vec<_> = rows
                .iter()
                .filter(|row| {
                    row.tasks.is_empty()
                        || task.is_some_and(|id| row.tasks.iter().any(|item| item == id))
                })
                .collect();
            applicable.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
            MemoryUse::from_rows(applicable.into_iter())
        })
        .max_by_key(|usage| usage.total_chars)
        .unwrap_or_default()
}

pub(crate) struct BriefInput<'a> {
    pub(crate) task: &'a str,
    /// The lead's complete supplied brief when this lane is linked to a
    /// stable task. Unlinked lanes already carry this text as `task`.
    pub(crate) supplied_task: Option<&'a str>,
    pub(crate) instructions: &'a str,
    /// (note id, rendered note), in the order they should be included.
    pub(crate) facts: &'a [(String, String)],
    pub(crate) repository: &'a str,
    pub(crate) machine: &'a str,
    pub(crate) gates: Option<&'a [crate::project::Gate]>,
    pub(crate) restart: bool,
    pub(crate) report_path: &'a str,
    pub(crate) library_path: &'a str,
}

fn compose_brief(input: &BriefInput) -> String {
    let mut brief = String::new();
    if input.restart {
        brief.push_str(
            "**A previous attempt at this task exists on this branch.** Read its report at the report path below first, look at what is already on the branch, and continue from there.\n\n",
        );
    }
    brief.push_str("# Task\n\n");
    if let Some(supplied) = input.supplied_task {
        brief.push_str("## Lead brief\n\n");
        brief.push_str(supplied);
        if !supplied.ends_with('\n') {
            brief.push('\n');
        }
        brief.push('\n');
    }
    brief.push_str(input.task.trim());
    brief.push_str("\n\n# Instructions in force\n\n");
    if input.instructions.trim().is_empty() {
        brief.push_str("None.\n");
    } else {
        brief.push_str(input.instructions.trim());
        brief.push('\n');
    }
    brief.push_str("\n# Facts in force\n");

    let mut used = 0;
    let mut left_out = Vec::new();
    for (id, text) in input.facts {
        let block = memory_block(id, text);
        let size = block.chars().count();
        if used + size <= MEMORY_CAP_CHARS {
            used += size;
            brief.push_str(&block);
        } else {
            left_out.push(id.as_str());
        }
    }
    if input.facts.is_empty() {
        brief.push_str("\nNone.\n");
    }
    if !left_out.is_empty() {
        brief.push_str(&format!(
            "\nNot included because dated facts are over {MEMORY_CAP_CHARS} characters: {}.\n",
            left_out.join(", ")
        ));
    }

    brief.push_str("\n# Repository, machine and pinned gates\n\n");
    if input.repository.is_empty() {
        brief.push_str("- Repository: none.\n");
    } else {
        brief.push_str(&format!("- Repository: `{}`.\n", input.repository));
    }
    brief.push_str(&format!("- Machine: {}.\n", input.machine));
    match input.gates {
        None if input.repository.is_empty() => brief.push_str("- Gates: not applicable.\n"),
        None => brief.push_str("- Gates: not configured for this repository.\n"),
        Some([]) => brief.push_str("- Gates: this repository is explicitly gate-free.\n"),
        Some(gates) => {
            brief.push_str("- Gates:\n");
            for gate in gates {
                brief.push_str(&format!("  - `{}`", gate.command));
                if !gate.env.is_empty() {
                    brief.push_str(" with environment ");
                    brief.push_str(
                        &gate
                            .env
                            .iter()
                            .map(|(key, value)| format!("`{key}={value}`"))
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                }
                brief.push('\n');
            }
        }
    }
    brief.push_str(&format!(
        "\n# Finish\n\nCommit the finished work, then run `hp done --report {} --sha <commit-sha>`.\n\n# Paths\n\n- Report: `{}`\n- Library folder for files meant for Rolf: `{}`\n",
        input.report_path, input.report_path, input.library_path
    ));
    brief
}

fn render_task(record: &crate::task::Task, rows: &[crate::note::Row]) -> String {
    let mut out = format!(
        "## {} — {}\n\nRequests: {}\n\nAcceptance conditions:\n",
        record.id,
        record.title.trim(),
        record.authority.join(", ")
    );
    for (index, condition) in record.acceptance.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", index + 1, condition.trim()));
    }
    let notes: Vec<_> = rows.iter().filter(|row| row.kind == "task note").collect();
    if !notes.is_empty() {
        out.push_str("\nTask notes:\n\n");
        for note in notes {
            out.push_str(&render_brief_row(note));
            out.push_str("\n\n");
        }
    }
    out.trim_end().to_string()
}

/// Builds a frozen helper brief from the same current records as PROJECT.md.
pub(crate) fn brief_for(
    project: &Project,
    thread: &Thread,
    supplied_task: &str,
    restart: bool,
) -> Result<String> {
    let task_record = crate::task::list_with_errors(project)
        .0
        .into_iter()
        .find(|record| record.attempts.iter().any(|attempt| attempt == &thread.id));
    let task_id = task_record.as_ref().map(|record| record.id.as_str());
    let mut active = crate::note::active_for(project, task_id);
    crate::note::sort_newest_first(&mut active);
    let instructions = active
        .iter()
        .filter(|row| brief_carries_instruction(row))
        .map(render_brief_row)
        .collect::<Vec<_>>()
        .join("\n\n");
    let facts = MemoryUse::from_rows(active.iter());
    let task = task_record
        .as_ref()
        .map(|record| render_task(record, &active))
        .unwrap_or_else(|| supplied_task.trim().to_string());
    let supplied_task = task_record.as_ref().map(|_| supplied_task);
    let (settings, _) = project.read_project_md()?;
    let repo = settings.repos.iter().find(|repo| repo.path == thread.repo);
    let gates = repo.and_then(|repo| repo.gates.as_deref());
    let machine = if thread.machine.is_empty() {
        "local"
    } else {
        &thread.machine
    };
    Ok(compose_brief(&BriefInput {
        task: &task,
        supplied_task,
        instructions: &instructions,
        facts: &facts.notes,
        repository: &thread.repo,
        machine,
        gates,
        restart,
        report_path: &thread.report_path(),
        library_path: &thread.library_path(),
    }))
}

// ---------------------------------------------------------------- groups

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    ReadyForReview,
    WaitingOnYou,
    Unknown,
    Working,
    Landing,
    Idle,
    Resolved,
}

impl Group {
    /// Display order, shared by the sidebar `rank` token and the overview:
    /// separate from the precedence in `group()`.
    pub(crate) fn rank(self) -> u8 {
        match self {
            Group::ReadyForReview => 1,
            Group::WaitingOnYou => 2,
            Group::Unknown => 3,
            Group::Working => 4,
            Group::Landing => 5,
            Group::Idle => 6,
            Group::Resolved => 7,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Group::ReadyForReview => "Ready for review",
            Group::WaitingOnYou => "Waiting on you",
            Group::Unknown => "Unknown",
            Group::Working => "Working",
            Group::Landing => "Landing",
            Group::Idle => "Idle",
            Group::Resolved => "Resolved",
        }
    }

    /// Lower-case hyphenated form, used in the `review` token and `last_group`.
    pub(crate) fn token(self) -> &'static str {
        match self {
            Group::ReadyForReview => "ready-for-review",
            Group::WaitingOnYou => "waiting-on-you",
            Group::Unknown => "unknown",
            Group::Working => "working",
            Group::Landing => "landing",
            Group::Idle => "idle",
            Group::Resolved => "resolved",
        }
    }

    pub(crate) fn from_token(token: &str) -> Option<Group> {
        [
            Group::ReadyForReview,
            Group::WaitingOnYou,
            Group::Unknown,
            Group::Working,
            Group::Landing,
            Group::Idle,
            Group::Resolved,
        ]
        .into_iter()
        .find(|g| g.token() == token)
    }

    pub(crate) const DISPLAY_ORDER: [Group; 7] = [
        Group::ReadyForReview,
        Group::WaitingOnYou,
        Group::Unknown,
        Group::Working,
        Group::Landing,
        Group::Idle,
        Group::Resolved,
    ];
}

/// What herdr shows for a thread's pane right now.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Live {
    pub(crate) pane_exists: bool,
    /// `None` when no agent is detected in the pane.
    pub(crate) agent_state: Option<String>,
    /// How long the agent has been in that state.
    pub(crate) state_secs: i64,
}

pub(crate) fn seconds_since(timestamp: &str, now: jiff::Timestamp) -> i64 {
    timestamp
        .parse::<jiff::Timestamp>()
        .map(|then| now.as_second() - then.as_second())
        .unwrap_or(0)
}

/// The group of a thread. First matching row wins. One function, so the CLI
/// and the ticker always agree.
pub(crate) fn recorded_group(thread: &Thread, now: jiff::Timestamp) -> Group {
    match thread.status {
        Status::Resolved => Group::Resolved,
        Status::Failed => Group::WaitingOnYou,
        Status::Starting if seconds_since(&thread.created, now) >= STARTING_TIMEOUT_SECS => {
            Group::WaitingOnYou
        }
        Status::Starting => Group::Working,
        Status::Open if thread.is_remote() && thread.last_state.is_empty() => Group::Unknown,
        Status::Open => Group::from_token(&thread.last_group).unwrap_or(if thread.prompt_pending {
            Group::Working
        } else {
            Group::Idle
        }),
    }
}

pub(crate) fn group(thread: &Thread, live: &Live, now: jiff::Timestamp) -> Group {
    let state = live.agent_state.as_deref();
    let has_report = !thread.report_hash.is_empty();
    // 1
    if thread.status == Status::Resolved {
        return Group::Resolved;
    }
    // 2
    if thread.status == Status::Starting {
        return if seconds_since(&thread.created, now) < STARTING_TIMEOUT_SECS {
            Group::Working
        } else {
            Group::WaitingOnYou
        };
    }
    // 3
    let stuck_launch = thread.prompt_pending
        && state.is_some_and(|s| !ready_state(s))
        && live.state_secs >= NOT_READY_SECS;
    let pane_gone_without_report = !live.pane_exists && !has_report;
    let blocked_long = state == Some("blocked") && live.state_secs >= BLOCKED_DEBOUNCE_SECS;
    if thread.status == Status::Failed || stuck_launch || pane_gone_without_report || blocked_long {
        return Group::WaitingOnYou;
    }
    // A listed pane without a matching agent is evidence only that the agent
    // state is unknown. While the first bounded launch attempts are still
    // pending it remains Working; after that it waits for evidence or a
    // coordinator action without being declared gone.
    if live.pane_exists
        && state.is_none()
        && !has_report
        && (!thread.prompt_pending || thread.launch_attempts >= MAX_LAUNCH_ATTEMPTS)
    {
        return Group::Unknown;
    }
    // 4
    if matches!(state, Some("working") | Some("blocked")) || thread.prompt_pending {
        return Group::Working;
    }
    // 5
    let pr_open = thread.pr_state.eq_ignore_ascii_case("open");
    if pr_open && thread.pr_review.eq_ignore_ascii_case("approved") {
        return Group::Landing;
    }
    // 6
    if has_report && (pr_open || thread.report_hash != thread.acked_report_hash) {
        return Group::ReadyForReview;
    }
    // 7
    Group::Idle
}

/// A pane is the thread's pane only when workspace, tab and working directory
/// match the record, and — for threads the binary started — the agent name.
/// Ids are compared only among panes listed through the project's own socket.
pub(crate) fn pane_matches(thread: &Thread, pane: &Pane) -> bool {
    pane.pane_id == thread.pane_id
        && pane.workspace_id == thread.workspace_id
        && pane.tab_id == thread.tab_id
        && pane.cwd == thread.cwd
}

pub(crate) fn agent_matches(thread: &Thread, agent: &Agent) -> bool {
    let ids = agent.pane_id == thread.pane_id
        && agent.workspace_id == thread.workspace_id
        && agent.tab_id == thread.tab_id
        && agent.cwd == thread.cwd;
    match thread.kind {
        // Not started by the binary: whatever name herdr reported at adoption.
        Kind::Adopted => ids,
        _ => ids && agent.name == thread.agent_name,
    }
}

/// Live state from one `agent list` and one `pane list`. `recorded` supplies
/// the duration: the ticker keeps `last_state_change` current; a CLI call uses
/// it when the live state equals the recorded one and zero otherwise.
pub(crate) fn live_state(
    thread: &Thread,
    agents: &[Agent],
    panes: &[Pane],
    now: jiff::Timestamp,
) -> Live {
    let agent = agents.iter().find(|a| agent_matches(thread, a));
    // The matching pane list row, or any agent row with the same terminal
    // identity, proves the pane still exists. A different/missing agent name
    // makes agent state unknown; it does not make the pane disappear.
    let pane_exists = agent.is_some()
        || panes.iter().any(|p| pane_matches(thread, p))
        || agents.iter().any(|a| {
            a.pane_id == thread.pane_id
                && a.workspace_id == thread.workspace_id
                && a.tab_id == thread.tab_id
                && a.cwd == thread.cwd
        });
    let agent_state = agent.map(|a| a.agent_status.clone());
    let state_secs = match &agent_state {
        Some(state) if *state == thread.last_state => seconds_since(&thread.last_state_change, now),
        _ => 0,
    };
    Live {
        pane_exists,
        agent_state,
        state_secs,
    }
}

/// Compare a live agent and process with the stored binding (SPEC-ADE D3).
/// `terminal_id` is never compared.
pub(crate) fn identity_verifies(
    thread: &Thread,
    agent: &Agent,
    live: &[crate::contracts::ProcessIdentity],
) -> bool {
    if !agent_matches(thread, agent) {
        return false;
    }
    if thread.cwd != agent.cwd {
        return false;
    }
    let Some(stored) = thread.identity.process.as_ref() else {
        return false;
    };
    live.iter()
        .any(|p| stored.pid == p.pid && stored.argv0 == p.argv0)
}

pub(crate) fn bind_identity(
    thread: &mut Thread,
    socket: &str,
    agent: &Agent,
    process: Option<crate::contracts::ProcessIdentity>,
) {
    thread.identity = crate::contracts::IdentityBinding {
        socket: socket.to_string(),
        workspace_id: agent.workspace_id.clone(),
        tab_id: agent.tab_id.clone(),
        pane_id: agent.pane_id.clone(),
        cwd: agent.cwd.clone(),
        agent_name: if thread.kind == Kind::Adopted {
            None
        } else {
            Some(thread.agent_name.clone()).filter(|s| !s.is_empty())
        },
        process,
        agent_session: agent
            .agent_session
            .as_ref()
            .map(|s| s.id.clone())
            .filter(|s| !s.is_empty()),
    };
}

// ---------------------------------------------------------------- copy home

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CopyOutcome {
    Complete,
    /// The report was copied but something was skipped; each note says what.
    Partial(Vec<String>),
    Failed(String),
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

fn symlinks_under(dir: &Path, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_symlink(&path) {
            found.push(path.display().to_string());
        } else if path.is_dir() {
            symlinks_under(&path, found);
        }
    }
}

/// The hash of a local thread's report when it is a regular file inside a real
/// thread directory. Cheap enough to run every tick.
pub(crate) fn local_report_hash(thread: &Thread) -> Option<String> {
    let dir = Path::new(&thread.thread_dir);
    if thread.thread_dir.is_empty() || !is_real_dir(dir) {
        return None;
    }
    let report = dir.join("report.md");
    let regular = std::fs::symlink_metadata(&report).is_ok_and(|m| m.is_file());
    regular
        .then(|| std::fs::read(&report).ok())
        .flatten()
        .map(|bytes| sha256_hex(&bytes))
}

pub(crate) struct Copied {
    pub(crate) outcome: CopyOutcome,
    /// The report's hash, when a regular report file exists.
    pub(crate) report_hash: Option<String>,
}

/// Observes a local thread's report and, when `with_library`, copies its real
/// deliverables home. The report itself is never copied: `ha done` stores its
/// sealed artifact. Nothing that is a symbolic link is followed or copied. The
/// caller must not hold the project lock: this runs `du` and `rsync`.
pub(crate) fn copy_home_local(
    project: &Project,
    thread: &Thread,
    with_library: bool,
    runner: &dyn Runner,
) -> Copied {
    let dir = Path::new(&thread.thread_dir);
    let mut notes = Vec::new();
    if thread.thread_dir.is_empty() || !dir.exists() {
        // Nothing was ever written, so nothing can be lost.
        return Copied {
            outcome: CopyOutcome::Complete,
            report_hash: None,
        };
    }
    if !is_real_dir(dir) {
        return Copied {
            outcome: CopyOutcome::Partial(vec![format!(
                "{} is a symbolic link; nothing was copied",
                dir.display()
            )]),
            report_hash: None,
        };
    }

    let report = dir.join("report.md");
    let mut report_hash = None;
    match std::fs::symlink_metadata(&report) {
        Err(_) => {}
        Ok(meta) if meta.is_file() => match std::fs::read(&report) {
            Ok(bytes) => report_hash = Some(sha256_hex(&bytes)),
            Err(error) => {
                return Copied {
                    outcome: CopyOutcome::Failed(format!(
                        "could not read {}: {error}",
                        report.display()
                    )),
                    report_hash: None,
                };
            }
        },
        Ok(_) => notes.push(format!(
            "{} is not a regular file; it was not copied",
            report.display()
        )),
    }

    if with_library {
        let library = dir.join("library");
        if is_symlink(&library) {
            notes.push(format!(
                "{} is a symbolic link; the library was not copied",
                library.display()
            ));
        } else if is_real_dir(&library) {
            match copy_library_local(project, thread, &library, runner) {
                Ok(mut skipped) => notes.append(&mut skipped),
                Err(error) => {
                    return Copied {
                        outcome: CopyOutcome::Failed(format!("{error:#}")),
                        report_hash,
                    };
                }
            }
        }
    }

    let outcome = if notes.is_empty() {
        CopyOutcome::Complete
    } else {
        CopyOutcome::Partial(notes)
    };
    Copied {
        outcome,
        report_hash,
    }
}

fn has_library_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        !is_symlink(&path) && (regular_file(&path) || (path.is_dir() && has_library_file(&path)))
    })
}

fn copy_library_local(
    project: &Project,
    thread: &Thread,
    library: &Path,
    runner: &dyn Runner,
) -> Result<Vec<String>> {
    let du = runner
        .run(&Cmd::new("du", Duration::from_secs(10)).args(["-sk", &library.to_string_lossy()]))?;
    let kb: u64 = du
        .stdout
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .context("could not measure the library folder")?;
    if kb > LIBRARY_CAP_KB {
        return Ok(vec![format!(
            "the library is {} MB, over the {} MB cap; nothing from it was copied",
            kb / 1024,
            LIBRARY_CAP_KB / 1024
        )]);
    }
    let mut links = Vec::new();
    symlinks_under(library, &mut links);
    let notes: Vec<String> = links
        .iter()
        .map(|p| format!("{p} is a symbolic link; it was not copied"))
        .collect();
    if !has_library_file(library) {
        return Ok(notes);
    }

    let library_home = project.dir().join("library");
    let target = library_home.join(&thread.id);
    {
        let _lock = project.lock()?;
        if !library_home.is_dir() {
            // `create_dir`, not `create_dir_all`: never recreate a deleted project.
            std::fs::create_dir(&library_home)
                .with_context(|| format!("could not create {}", library_home.display()))?;
        }
        if !target.is_dir() {
            std::fs::create_dir(&target)
                .with_context(|| format!("could not create {}", target.display()))?;
        }
    }
    // `-rt` without `-l`: symbolic links are skipped, never followed.
    let out = runner.run(&Cmd::new("rsync", Duration::from_secs(60)).args([
        "-rt".to_string(),
        format!("{}/", library.to_string_lossy()),
        format!("{}/", target.to_string_lossy()),
    ]))?;
    if !out.success() {
        bail!("rsync failed: {}", out.error_text());
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RealRunner;

    fn now() -> jiff::Timestamp {
        "2026-09-17T12:00:00Z".parse().unwrap()
    }

    fn ago(secs: i64) -> String {
        (now() - jiff::SignedDuration::from_secs(secs)).to_string()
    }

    fn open_thread() -> Thread {
        Thread {
            id: "t-0001".into(),
            status: Status::Open,
            created: ago(3600),
            ..Thread::default()
        }
    }

    fn live(state: Option<&str>, secs: i64) -> Live {
        Live {
            pane_exists: true,
            agent_state: state.map(str::to_string),
            state_secs: secs,
        }
    }

    #[test]
    fn row1_resolved_wins_over_everything() {
        let t = Thread {
            status: Status::Resolved,
            prompt_pending: true,
            ..open_thread()
        };
        assert_eq!(
            group(&t, &live(Some("blocked"), 999), now()),
            Group::Resolved
        );
    }

    #[test]
    fn row2_starting_is_working_for_five_minutes() {
        let young = Thread {
            status: Status::Starting,
            created: ago(10),
            ..open_thread()
        };
        assert_eq!(group(&young, &Live::default(), now()), Group::Working);
        let old = Thread {
            status: Status::Starting,
            created: ago(301),
            ..open_thread()
        };
        assert_eq!(group(&old, &Live::default(), now()), Group::WaitingOnYou);
    }

    #[test]
    fn an_unpolled_remote_thread_is_unknown() {
        let remote = Thread {
            machine: "box".into(),
            last_state: String::new(),
            ..open_thread()
        };
        assert_eq!(recorded_group(&remote, now()), Group::Unknown);
        assert_eq!(Group::Unknown.label(), "Unknown");
    }

    #[test]
    fn row3_waiting_on_you() {
        let failed = Thread {
            status: Status::Failed,
            ..open_thread()
        };
        assert_eq!(
            group(&failed, &live(Some("working"), 0), now()),
            Group::WaitingOnYou
        );
        let stale_box_view = Thread {
            last_group: "working".into(),
            ..failed
        };
        assert_eq!(
            recorded_group(&stale_box_view, now()),
            Group::WaitingOnYou,
            "a failed box start must not stay Working from its last poll"
        );

        let pending = Thread {
            prompt_pending: true,
            ..open_thread()
        };
        assert_eq!(
            group(&pending, &live(Some("blocked"), 60), now()),
            Group::WaitingOnYou
        );
        assert_eq!(
            group(&pending, &live(Some("unknown"), 60), now()),
            Group::WaitingOnYou
        );

        let gone = Live {
            pane_exists: false,
            agent_state: None,
            state_secs: 0,
        };
        assert_eq!(group(&open_thread(), &gone, now()), Group::WaitingOnYou);

        assert_eq!(
            group(&open_thread(), &live(Some("blocked"), 30), now()),
            Group::WaitingOnYou
        );
    }

    #[test]
    fn row4_working_including_a_launch_in_progress() {
        assert_eq!(
            group(&open_thread(), &live(Some("working"), 0), now()),
            Group::Working
        );
        // A permission prompt answered quickly never shows as waiting.
        assert_eq!(
            group(&open_thread(), &live(Some("blocked"), 29), now()),
            Group::Working
        );
        // A new thread is Working, not Waiting on you, until an undetected-ready
        // agent has lasted 60 seconds.
        let pending = Thread {
            prompt_pending: true,
            ..open_thread()
        };
        assert_eq!(group(&pending, &live(None, 0), now()), Group::Working);
        let attempts_exhausted = Thread {
            launch_attempts: MAX_LAUNCH_ATTEMPTS,
            ..pending.clone()
        };
        assert_eq!(
            group(&attempts_exhausted, &live(None, 0), now()),
            Group::Unknown
        );
        assert_eq!(
            group(&pending, &live(Some("unknown"), 59), now()),
            Group::Working
        );
        assert_eq!(
            group(&pending, &live(Some("idle"), 500), now()),
            Group::Working
        );
    }

    #[test]
    fn row5_landing_needs_open_and_approved() {
        let t = Thread {
            report_hash: "h".into(),
            pr_state: "OPEN".into(),
            pr_review: "APPROVED".into(),
            ..open_thread()
        };
        assert_eq!(group(&t, &live(Some("idle"), 0), now()), Group::Landing);
        let t = Thread {
            pr_review: "CHANGES_REQUESTED".into(),
            ..t
        };
        assert_eq!(
            group(&t, &live(Some("idle"), 0), now()),
            Group::ReadyForReview
        );
    }

    #[test]
    fn row6_ready_for_review_until_ack_or_while_pr_open() {
        let t = Thread {
            report_hash: "h".into(),
            ..open_thread()
        };
        assert_eq!(
            group(&t, &live(Some("done"), 0), now()),
            Group::ReadyForReview
        );
        let acked = Thread {
            acked_report_hash: "h".into(),
            ..t.clone()
        };
        assert_eq!(group(&acked, &live(Some("done"), 0), now()), Group::Idle);
        let with_pr = Thread {
            pr_state: "OPEN".into(),
            ..acked
        };
        assert_eq!(
            group(&with_pr, &live(Some("done"), 0), now()),
            Group::ReadyForReview
        );
    }

    #[test]
    fn row7_idle_and_precedence() {
        assert_eq!(
            group(&open_thread(), &live(Some("idle"), 0), now()),
            Group::Idle
        );
        // Working (row 4) beats Ready for review (row 6).
        let t = Thread {
            report_hash: "h".into(),
            ..open_thread()
        };
        assert_eq!(group(&t, &live(Some("working"), 0), now()), Group::Working);
        // Blocked for long (row 3) beats an approved pull request (row 5).
        let t = Thread {
            pr_state: "OPEN".into(),
            pr_review: "APPROVED".into(),
            ..t
        };
        assert_eq!(
            group(&t, &live(Some("blocked"), 31), now()),
            Group::WaitingOnYou
        );
    }

    #[test]
    fn pane_gone_with_a_report_keeps_its_place() {
        let gone = Live {
            pane_exists: false,
            agent_state: None,
            state_secs: 0,
        };
        let t = Thread {
            report_hash: "h".into(),
            ..open_thread()
        };
        assert_eq!(group(&t, &gone, now()), Group::ReadyForReview);
        let acked = Thread {
            acked_report_hash: "h".into(),
            ..t
        };
        assert_eq!(group(&acked, &gone, now()), Group::Idle);
    }

    #[test]
    fn display_order_and_rank_digits() {
        let ranks: Vec<u8> = Group::DISPLAY_ORDER.iter().map(|g| g.rank()).collect();
        assert_eq!(ranks, [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(Group::ReadyForReview.token(), "ready-for-review");
        assert_eq!(Group::WaitingOnYou.token(), "waiting-on-you");
        assert_eq!(Group::from_token("landing"), Some(Group::Landing));
    }

    fn agent(name: &str, cwd: &str) -> Agent {
        Agent {
            pane_id: "w2:p1".into(),
            tab_id: "w2:t1".into(),
            workspace_id: "w2".into(),
            name: name.into(),
            agent_status: "idle".into(),
            cwd: cwd.into(),
            ..Agent::default()
        }
    }

    fn placed_thread(kind: Kind) -> Thread {
        Thread {
            kind,
            pane_id: "w2:p1".into(),
            tab_id: "w2:t1".into(),
            workspace_id: "w2".into(),
            agent_name: "hp-demo-t-0001".into(),
            cwd: "/wt".into(),
            last_state: "idle".into(),
            last_state_change: ago(45),
            ..open_thread()
        }
    }

    #[test]
    fn identity_check_before_acting_on_a_pane() {
        let t = placed_thread(Kind::Worktree);
        assert!(agent_matches(&t, &agent("hp-demo-t-0001", "/wt")));
        assert!(!agent_matches(&t, &agent("hp-demo-t-0002", "/wt")));
        assert!(!agent_matches(&t, &agent("hp-demo-t-0001", "/other")));
        // Same terminal ids but someone else's agent prove the pane exists;
        // they do not prove our agent's state.
        let state = live_state(&t, &[agent("other", "/wt")], &[], now());
        assert!(state.pane_exists);
        assert_eq!(state.agent_state, None);
        assert_eq!(group(&t, &state, now()), Group::Unknown);
    }

    #[test]
    fn identity_verifies_pid_and_argv0_and_ignores_terminal_id() {
        let process = crate::contracts::ProcessIdentity {
            pid: 9,
            argv0: "/bin/claude".into(),
        };
        let mut t = placed_thread(Kind::Worktree);
        t.identity.process = Some(process.clone());
        t.identity.pane_id = t.pane_id.clone();
        t.identity.cwd = t.cwd.clone();
        let live = agent("hp-demo-t-0001", "/wt");
        assert!(identity_verifies(&t, &live, std::slice::from_ref(&process)));
        let other = crate::contracts::ProcessIdentity {
            pid: 10,
            argv0: "/bin/claude".into(),
        };
        assert!(!identity_verifies(&t, &live, std::slice::from_ref(&other)));
        // A1 review M2: a tool in the foreground beside the agent.
        assert!(identity_verifies(
            &t,
            &live,
            &[other.clone(), process.clone()]
        ));
        let renamed = crate::contracts::ProcessIdentity {
            pid: 9,
            argv0: "/bin/other".into(),
        };
        assert!(!identity_verifies(&t, &live, &[renamed]));
        assert!(!identity_verifies(&t, &live, &[]));
        bind_identity(&mut t, "/sock", &live, Some(process.clone()));
        assert_eq!(t.identity.socket, "/sock");
        assert_eq!(t.identity.agent_name.as_deref(), Some("hp-demo-t-0001"));
        let mut adopted = placed_thread(Kind::Adopted);
        bind_identity(&mut adopted, "/sock", &live, None);
        assert!(adopted.identity.agent_name.is_none());
        assert!(adopted.identity.process.is_none());
    }

    #[test]
    fn adopted_threads_match_without_the_name() {
        let t = Thread {
            agent_name: String::new(),
            ..placed_thread(Kind::Adopted)
        };
        assert!(agent_matches(&t, &agent("whatever", "/wt")));
        assert!(!agent_matches(&t, &agent("whatever", "/elsewhere")));
    }

    #[test]
    fn live_state_duration_comes_from_the_record_only_when_states_agree() {
        let t = placed_thread(Kind::Worktree);
        let same = live_state(&t, &[agent("hp-demo-t-0001", "/wt")], &[], now());
        assert_eq!(same.state_secs, 45);
        let mut other = agent("hp-demo-t-0001", "/wt");
        other.agent_status = "blocked".into();
        assert_eq!(live_state(&t, &[other], &[], now()).state_secs, 0);
    }

    #[test]
    fn ids_branches_and_dirs() {
        assert!(validate_id("t-0001").is_ok());
        assert!(validate_id("t-12345").is_ok());
        for bad in ["", "t-1", "t-00a1", "../t-0001", "x-0001"] {
            assert!(validate_id(bad).is_err(), "{bad}");
        }
        assert_eq!(
            branch_name("demo", "t-0001", "Fix the $(login) bug!"),
            "hp/demo/t-0001-fix-the-login-bug"
        );
        assert_eq!(branch_name("demo", "t-0002", "???"), "hp/demo/t-0002");
        assert_eq!(
            thread_dir("/wt/", "demo", "t-0001"),
            "/wt/.herdr-project/demo-t-0001"
        );
        // A1 H2: a launched lane is primed with its role skill (D9, D14).
        let lane = Thread {
            id: "t-0001".into(),
            role: "reviewer".into(),
            ..placed_thread(Kind::Worktree)
        };
        assert_eq!(
            launch_prompt("ha", "demo", &lane),
            "Run ha skill reviewer, then read .herdr-project/demo-t-0001/brief.md and do what it says."
        );
    }

    #[test]
    fn id_allocation_under_contention() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let project = project.clone();
                std::thread::spawn(move || allocate(&project, |_| {}).unwrap().id)
            })
            .collect();
        let mut ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 8);
        assert_eq!(ids[0], "t-0001");
        assert_eq!(ids[7], "t-0008");
    }

    #[test]
    fn queued_follow_ups_record_attempt_end_dispositions() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let lane = allocate(&project, |lane| {
            lane.attempt = 1;
            lane.follow_ups.push(FollowUp {
                attempt: 1,
                text: "old attempt".into(),
                state: FollowUpState::Queued,
            });
        })
        .unwrap();
        let superseded = update(&project, &lane.id, |lane| lane.attempt = 2).unwrap();
        assert_eq!(superseded.follow_ups[0].state, FollowUpState::Superseded);

        let cancelled = update(&project, &lane.id, |lane| {
            lane.follow_ups.push(FollowUp {
                attempt: 2,
                text: "cancel this".into(),
                state: FollowUpState::Queued,
            });
            lane.status = Status::Resolved;
            lane.resolved_reason = "cancelled".into();
        })
        .unwrap();
        assert_eq!(cancelled.follow_ups[0].state, FollowUpState::Superseded);
        assert_eq!(cancelled.follow_ups[1].state, FollowUpState::Cancelled);
    }

    #[test]
    fn only_unknown_harness_breakage_enters_the_failure_ledger() {
        let root = tempfile::tempdir().unwrap();
        let p = project::create(root.path(), "demo", "", vec![]).unwrap();
        let t = allocate(&p, |t| {
            t.attempt = 1;
            t.launch.kind = "pi".into();
        })
        .unwrap();
        for _ in 0..3 {
            update(&p, &t.id, |t| {
                t.status = Status::Failed;
                t.error = "login expired".into();
                t.last_state = "blocked".into();
            })
            .unwrap();
        }
        let entries = crate::ledger::list(&p).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.count == 1));
        assert!(entries.iter().any(|e| e.kind == "launch-not-attempted"));
        update(&p, &t.id, |t| {
            t.attempt += 1;
            t.launch_attempts = 1;
        })
        .unwrap();
        update(&p, &t.id, |t| t.launch_attempts += 1).unwrap();
        let entries = crate::ledger::list(&p).unwrap();
        assert!(!entries.iter().any(|e| e.kind == "retry"));
    }

    #[test]
    fn updates_are_atomic_and_keep_other_fields() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let t = allocate(&project, |t| t.title = "Hello".into()).unwrap();
        update(&project, &t.id, |t| t.pane_id = "w1:p2".into()).unwrap();
        update(&project, &t.id, |t| t.prompt_pending = true).unwrap();
        let t = load(&project, &t.id).unwrap();
        assert_eq!(
            (t.title.as_str(), t.pane_id.as_str(), t.prompt_pending),
            ("Hello", "w1:p2", true)
        );
        let leftovers = std::fs::read_dir(threads_dir(&project))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn brief_order_and_memory_cap() {
        let notes = vec![
            ("n-0001".to_string(), "alpha fact".to_string()),
            ("n-0002".to_string(), "x".repeat(MEMORY_CAP_CHARS)),
            ("n-0003".to_string(), "gamma fact".to_string()),
        ];
        let gates = vec![crate::project::Gate {
            command: "cargo test".into(),
            env: std::collections::BTreeMap::from([("RUST_BACKTRACE".into(), "1".into())]),
        }];
        let brief = compose_brief(&BriefInput {
            task: "Do the thing.",
            supplied_task: None,
            instructions: "Always run the tests.",
            facts: &notes,
            repository: "/repo",
            machine: "local",
            gates: Some(&gates),
            restart: true,
            report_path: "/wt/.herdr-project/demo-t-0001/report.md",
            library_path: "/wt/.herdr-project/demo-t-0001/library",
        });
        let pos = |needle: &str| {
            brief
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        assert!(brief.starts_with("**A previous attempt"));
        assert!(pos("previous attempt") < pos("Always run the tests."));
        assert!(pos("Do the thing.") < pos("Always run the tests."));
        assert!(pos("Always run the tests.") < pos("# Facts in force"));
        assert!(pos("# Facts in force") < pos("alpha fact"));
        assert!(pos("alpha fact") < pos("/wt/.herdr-project/demo-t-0001/report.md"));
        assert!(brief.contains("- Repository: `/repo`."));
        assert!(brief.contains("- Machine: local."));
        assert!(brief.contains("`cargo test` with environment `RUST_BACKTRACE=1`"));
        assert!(brief.contains("gamma fact"));
        assert!(
            brief.contains("Not included because dated facts are over 32000 characters: n-0002.")
        );
        assert!(!brief.contains(&"x".repeat(100)));

        let fresh = compose_brief(&BriefInput {
            task: "t",
            supplied_task: None,
            instructions: "",
            facts: &[],
            repository: "",
            machine: "local",
            gates: None,
            restart: false,
            report_path: "r",
            library_path: "l",
        });
        assert!(!fresh.contains("previous attempt"));
    }

    #[test]
    fn brief_carries_the_task_and_only_applicable_current_page_facts() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        crate::talk::append(
            &project,
            None,
            crate::talk::Entry::Rolf {
                request: "q-1".into(),
                text: "Keep helper briefs focused.".into(),
                answer: None,
            },
        )
        .unwrap();
        let current = crate::task::add(
            &project,
            "Ship the checked change.",
            vec!["request:q-1".into()],
            vec!["The command reports the new result.".into()],
            None,
            None,
        )
        .unwrap();
        let other = crate::task::add(
            &project,
            "Ship another checked change.",
            vec!["request:q-1".into()],
            vec!["The command reports another result.".into()],
            None,
            None,
        )
        .unwrap();
        crate::note::add(
            &project,
            crate::note::Kind::Instruction,
            "Dated instruction marker.",
            "q-1",
            None,
            vec![],
        )
        .unwrap();
        crate::note::add(
            &project,
            crate::note::Kind::Memory,
            "Applicable dated marker.",
            "q-1",
            None,
            vec![],
        )
        .unwrap();
        crate::note::add(
            &project,
            crate::note::Kind::Memory,
            "Other task marker.",
            "q-1",
            None,
            vec![other.id],
        )
        .unwrap();
        let thread = allocate(&project, |_| {}).unwrap();
        crate::task::link_attempt(&project, &current.id, &thread.id).unwrap();

        let brief = brief_for(&project, &thread, "Do the task.", false).unwrap();
        assert!(
            brief.contains(&format!("## {} — Ship the checked change.", current.id)),
            "{brief}"
        );
        assert!(brief.contains("Requests: request:q-1"), "{brief}");
        assert!(
            brief.contains("The command reports the new result."),
            "{brief}"
        );
        assert!(brief.contains("## Lead brief\n\nDo the task."), "{brief}");
        assert!(brief.contains("Dated instruction marker."), "{brief}");
        assert!(brief.contains("Applicable dated marker."), "{brief}");
        assert!(!brief.contains("Other task marker"), "{brief}");
        assert!(
            brief.contains("# Repository, machine and pinned gates"),
            "{brief}"
        );
        assert!(brief.contains("# Finish"), "{brief}");
        assert!(memory_use(&project).warning().is_none());
    }

    #[test]
    fn the_lane_skill_carries_the_push_rule_in_its_standing_rules() {
        let lane = include_str!("../skill/LANE.md");
        let rules_end = lane.find("## Pictures").expect("the pictures heading");
        let standing = &lane[..rules_end];
        assert!(
            standing.contains("`round merge` pushes the integration branch"),
            "{standing}"
        );
        assert!(
            standing.contains("a lane never pushes it or `main`"),
            "{standing}"
        );
        let box_section = lane.find("## On the cloud box").expect("the box heading");
        assert!(
            lane[box_section..].contains("push the lane branch to the URL-matched remote"),
            "{}",
            &lane[box_section..]
        );
    }

    #[test]
    fn memory_over_budget_warns_with_the_note_and_size() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        crate::talk::append(
            &project,
            None,
            crate::talk::Entry::Rolf {
                request: "q-1".into(),
                text: "Keep helper briefs focused.".into(),
                answer: None,
            },
        )
        .unwrap();
        let note = crate::note::add(
            &project,
            crate::note::Kind::Memory,
            &"x".repeat(MEMORY_CAP_CHARS + 1),
            "q-1",
            None,
            vec![],
        )
        .unwrap();

        let use_ = memory_use(&project);
        assert!(use_.over_budget());
        let warning = use_.warning().unwrap();
        assert!(warning.contains(&note.id), "{warning}");
        assert!(warning.contains("replace stale dated notes"), "{warning}");

        crate::note::add(
            &project,
            crate::note::Kind::Memory,
            "short",
            "q-1",
            Some(&note.id),
            vec![],
        )
        .unwrap();
        assert!(memory_use(&project).warning().is_none());
    }

    fn local_thread(project: &Project, dir: &Path) -> Thread {
        let t = allocate(project, |t| {
            t.thread_dir = dir.to_string_lossy().into_owned()
        })
        .unwrap();
        std::fs::create_dir_all(dir.join("library")).unwrap();
        t
    }

    #[test]
    fn empty_library_creates_no_home_folder_and_report_creates_no_copy() {
        use crate::runner::fake::{FakeRunner, ok};
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let dir = work.path().join(".herdr-project/demo-t-0001");
        let t = local_thread(&project, &dir);
        std::fs::write(dir.join("report.md"), "report\n").unwrap();
        let runner = FakeRunner::new();
        runner.on("du -sk", ok("0\t/x\n"));

        let copied = copy_home_local(&project, &t, true, &runner);

        assert_eq!(copied.outcome, CopyOutcome::Complete);
        assert_eq!(
            copied.report_hash.as_deref(),
            Some(sha256_hex(b"report\n").as_str())
        );
        assert!(!project.dir().join("library").exists());
        assert!(!home_report_path(&project, &t.id).exists());
        assert_eq!(runner.count("rsync"), 0);
    }

    #[test]
    fn sealed_artifact_is_the_report_and_unmatched_history_stays_readable() {
        use crate::contracts::{DonePayload, Event, EventPayload, Recipient};
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let sealed = allocate(&project, |thread| thread.attempt = 1).unwrap();
        let hash = crate::events::store_artifact(&project, b"sealed\n").unwrap();
        crate::events::seal_create_if_absent(
            &project,
            &Event {
                id: "done-1".into(),
                op: "done-1".into(),
                thread: sealed.id.clone(),
                attempt: 1,
                round: None,
                recipient: Recipient::default(),
                created: project::now(),
                payload: EventPayload {
                    done: Some(DonePayload {
                        sha: "abc".into(),
                        report_path: "old/location".into(),
                        artifact: hash.clone(),
                        attestation: None,
                    }),
                    ..EventPayload::default()
                },
            },
        )
        .unwrap();
        std::fs::write(home_report_path(&project, &sealed.id), b"sealed\n").unwrap();
        assert_eq!(
            final_report_path(&project, &sealed),
            Some(crate::events::artifact_path(&project, &hash))
        );

        let historical = allocate(&project, |_| {}).unwrap();
        let historical_path = home_report_path(&project, &historical.id);
        std::fs::write(&historical_path, b"historical only\n").unwrap();
        assert_eq!(
            final_report_path(&project, &historical),
            Some(historical_path)
        );

        let draft = allocate(&project, |thread| {
            thread.thread_dir = work_path(root.path(), "draft")
        })
        .unwrap();
        std::fs::create_dir_all(&draft.thread_dir).unwrap();
        std::fs::write(Path::new(&draft.thread_dir).join("report.md"), b"draft\n").unwrap();
        assert_eq!(final_report_path(&project, &draft), None);
    }

    fn work_path(root: &Path, name: &str) -> String {
        root.join(name).to_string_lossy().into_owned()
    }

    #[test]
    fn hashes_report_copies_real_library_and_skips_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let dir = work.path().join(".herdr-project/demo-t-0001");
        let t = local_thread(&project, &dir);
        std::fs::write(dir.join("report.md"), "## Report\nok\n").unwrap();
        std::fs::write(dir.join("library/out.txt"), "data").unwrap();

        let copied = copy_home_local(&project, &t, true, &RealRunner);
        assert_eq!(copied.outcome, CopyOutcome::Complete);
        assert_eq!(
            copied.report_hash.as_deref(),
            Some(sha256_hex(b"## Report\nok\n").as_str())
        );
        assert!(!home_report_path(&project, &t.id).exists());
        assert_eq!(
            std::fs::read_to_string(project.dir().join("library/t-0001/out.txt")).unwrap(),
            "data"
        );

        std::os::unix::fs::symlink("/etc/passwd", dir.join("library/link")).unwrap();
        let copied = copy_home_local(&project, &t, true, &RealRunner);
        assert!(matches!(copied.outcome, CopyOutcome::Partial(_)));
        assert!(!project.dir().join("library/t-0001/link").exists());
    }

    #[test]
    fn symlinked_library_report_and_thread_dir_are_not_copied() {
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let dir = work.path().join(".herdr-project/demo-t-0001");
        let t = local_thread(&project, &dir);
        std::fs::remove_dir(dir.join("library")).unwrap();
        std::os::unix::fs::symlink("/etc", dir.join("library")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.join("report.md")).unwrap();
        let copied = copy_home_local(&project, &t, true, &RealRunner);
        match copied.outcome {
            CopyOutcome::Partial(notes) => assert_eq!(notes.len(), 2, "{notes:?}"),
            other => panic!("{other:?}"),
        }
        assert!(copied.report_hash.is_none());
        assert!(!home_report_path(&project, &t.id).exists());
        assert!(!project.dir().join("library/t-0001").exists());

        let real = work.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("report.md"), "secret").unwrap();
        let linked = work.path().join("linked");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let t2 = allocate(&project, |t| {
            t.thread_dir = linked.to_string_lossy().into_owned()
        })
        .unwrap();
        let copied = copy_home_local(&project, &t2, true, &RealRunner);
        assert!(matches!(copied.outcome, CopyOutcome::Partial(_)));
        assert!(!home_report_path(&project, &t2.id).exists());
    }

    #[test]
    fn library_over_the_cap_is_not_copied() {
        use crate::runner::fake::{FakeRunner, ok};
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let dir = work.path().join(".herdr-project/demo-t-0001");
        let t = local_thread(&project, &dir);
        std::fs::write(dir.join("report.md"), "r").unwrap();
        let runner = FakeRunner::new();
        runner.on("du -sk", ok("60000\t/x\n"));
        let copied = copy_home_local(&project, &t, true, &runner);
        assert!(
            matches!(&copied.outcome, CopyOutcome::Partial(notes) if notes[0].contains("over the 50 MB cap"))
        );
        assert_eq!(runner.count("rsync"), 0);
        assert!(!home_report_path(&project, &t.id).exists());
    }

    #[test]
    fn failed_rsync_is_a_failed_copy() {
        use crate::runner::fake::{FakeRunner, fail, ok};
        let root = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let dir = work.path().join(".herdr-project/demo-t-0001");
        let t = local_thread(&project, &dir);
        std::fs::write(dir.join("library/out.txt"), "deliverable").unwrap();
        let runner = FakeRunner::new();
        runner.on("du -sk", ok("4\t/x\n"));
        runner.on("rsync", fail(23, "rsync: write failed"));
        assert!(matches!(
            copy_home_local(&project, &t, true, &runner).outcome,
            CopyOutcome::Failed(_)
        ));
    }
}
