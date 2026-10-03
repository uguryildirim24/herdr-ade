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
/// Advisory only: explicitly scoped facts are never truncated.
const MEMORY_WARNING_CHARS: usize = 32_000;
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
    Delivered,
    Uncertain,
    Superseded,
    Cancelled,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub(crate) struct FollowUp {
    pub(crate) attempt: u32,
    /// An undelivered instruction carried into a replacement's first prompt.
    pub(crate) carried_from_attempt: u32,
    pub(crate) text: String,
    pub(crate) state: FollowUpState,
    /// Waiting event visible when this message was accepted, not when it was sent.
    pub(crate) waiting_event: String,
    pub(crate) queued_at: String,
    pub(crate) delivered_at: String,
    pub(crate) closed_at: String,
    /// Completion already sealed when this follow-up landed.
    pub(crate) after_seal: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    #[default]
    Worktree,
    Tab,
    Adopted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RetirementAuthority {
    #[default]
    Resolve,
    Cancel,
    Retained,
}

/// Durable choices for the one resumable retirement sequence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub(crate) struct RetirementRequest {
    pub authority: RetirementAuthority,
    pub retained_tip: String,
    /// Deliverables were preserved before checkout deletion. Tail retries can
    /// finish without reading sources that the driver has already removed.
    pub preserved: bool,
    pub skip_copy: bool,
    pub discard_uncopied: bool,
    pub keep_pane: bool,
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
    /// First brief submission staged for this attempt and pane. Pending can
    /// remain true until activity is observed, without submitting it twice.
    pub(crate) brief_submitted: bool,
    /// Start of the bounded delivery observation window, not a receipt.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) brief_submitted_at: String,
    /// A sealed completion whose pane was closed; its branch and attempt remain live.
    pub(crate) parked: bool,
    /// The current attempt's sealed waiting event answered by the last
    /// successfully delivered follow-up. A later waiting event supersedes it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) answered_waiting_event: String,
    /// Attempt-bound follow-ups accepted while the first brief is pending.
    /// Terminal dispositions remain visible instead of crossing attempts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) follow_ups: Vec<FollowUp>,
    pub(crate) launch_attempts: u32,
    /// A placement blocked by provider readiness; empty outside the one-hour wait.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) provider_wait_started: String,
    /// Submission time of an agent start awaiting readiness after an early
    /// `agent_not_ready`. A blocked startup is not a failed attempt yet.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) startup_wait_started: String,
    /// Trust prompt answered once for this launch, never replayed on a poll.
    #[serde(default)]
    pub(crate) trust_answered: bool,
    pub(crate) failure_event: String,
    /// Start failures awaiting a coordinator wake-up, even across manual retries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) start_notices: Vec<crate::steps::Notice>,
    pub(crate) last_failure: String,
    /// Classification of the current failure evidence. Old records load as
    /// unknown rather than guessing from prose.
    #[serde(default)]
    pub(crate) failure_class: crate::contracts::FailureClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider_failure_kind: Option<String>,
    /// In-place connection recoveries during the current rolling hour.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) connection_resumes: Vec<String>,
    #[serde(default)]
    pub(crate) connection_waiting: bool,
    #[serde(alias = "escalation_pending")]
    pub(crate) recovery_pending: bool,
    pub(crate) kind: Kind,
    pub(crate) repo: String,
    pub(crate) origin: String,
    pub(crate) branch: String,
    pub(crate) base: String,
    /// Writable repository-relative globs; empty means unrestricted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) paths: Vec<String>,
    pub(crate) machine: String,
    /// Why placement selected this machine for the current attempt.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) placement_reason: String,
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
    /// Last working observation; empty if evidence is unavailable or the lane is not working.
    pub(crate) progress_pane: String,
    pub(crate) progress_screen: String,
    pub(crate) progress_head: String,
    pub(crate) progress_since: String,
    pub(crate) stall_notified: bool,
    pub(crate) no_commit_since: String,
    pub(crate) no_commit_notified: bool,
    pub(crate) report_hash: String,
    #[serde(default)]
    pub(crate) final_report_hash: String,
    #[serde(default)]
    pub(crate) final_report_seal: String,
    /// Relative report destinations absent when the worktree was retired.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) missing_report_links: Vec<String>,
    pub(crate) last_report_change: String,
    /// Incomplete report/library copy, kept with the report it describes.
    pub(crate) copy_notes: Vec<String>,
    pub(crate) lineage_mismatch: bool,
    pub(crate) acked_report_hash: String,
    pub(crate) resolved_reason: String,
    /// Why recovery deliberately stopped this thread. Empty on historical and
    /// normally resolved records.
    pub(crate) cancellation_reason: String,
    /// Cleanup still owes the same final-copy, pane/tab, and folder work that
    /// `thread resolve` performs. The ticker retries it instead of making a
    /// landed review wait on an external session.
    pub(crate) cleanup_pending: bool,
    /// Why retirement is pending; authority and pins never live in this prose.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) cleanup_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retirement: Option<RetirementRequest>,
    /// ADE role name (SPEC-ADE D2). Empty on a pre-ADE thread.
    pub(crate) role: String,
    /// Pile review this reviewer belongs to.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) review_id: String,
    /// Source refs exported for a box reviewer, pinned for eventual cleanup.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) review_sources: std::collections::BTreeMap<String, String>,
    pub(crate) has_changes: Option<bool>,
    pub(crate) changes_seal: String,
    /// One-time integration check for a sealed historical attempt.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) historical_seal: String,
    /// Last seal warned about because its repository is no longer configured.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) unconfigured_repo_seal: String,
    pub(crate) merged_sha: String,
    /// Historical seal covered by an installed harness build.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) installed_sha: String,
    #[serde(default)]
    pub(crate) historical_install_required: bool,
    pub(crate) merged_review: String,
    pub(crate) review_after: String,
    pub(crate) review_reason: String,
    /// Basename -> immutable artifact hash. Historical records have none.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) attachments: std::collections::BTreeMap<String, String>,
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
    /// Explicit commands can change their choices, but never drop a durable
    /// retained-removal pin. Automatic retries resume the request unchanged.
    pub(crate) fn retirement_request(&self, mut request: RetirementRequest) -> RetirementRequest {
        if let Some(saved) = &self.retirement {
            if saved.authority == RetirementAuthority::Retained {
                return saved.clone();
            }
            request.preserved = saved.preserved;
        }
        request
    }

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
    let (path, hash) = crate::events::for_thread(project, &thread.id)
        .into_iter()
        .filter(|event| event.thread == thread.id && event.attempt == attempt)
        .filter_map(|event| {
            let done = event.payload.done?;
            Some((event.created, event.id, done.artifact))
        })
        .max_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)))
        .map(|(_, _, hash)| (crate::events::artifact_path(project, &hash), hash))?;
    // A rewritten copy is valid only for the seal from which it was made.
    if thread.final_report_seal == hash && !thread.final_report_hash.is_empty() {
        let final_path = crate::events::artifact_path(project, &thread.final_report_hash);
        if regular_file(&final_path)
            && std::fs::read(&final_path)
                .is_ok_and(|bytes| sha256_hex(&bytes) == thread.final_report_hash)
        {
            return Some(final_path);
        }
    }
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
    #[cfg(test)]
    THREAD_READS.with(|count| count.set(count.get() + 1));
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("no thread `{id}` in `{}`", project.slug))?;
    let mut record: Thread =
        toml::from_str(&text).with_context(|| format!("{} does not parse", path.display()))?;
    // Historical pins lived in prose. Decode once at the record boundary;
    // execution and ref verification only consume the typed request.
    if record.retirement.is_none()
        && let Some(tip) = record
            .cleanup_reason
            .strip_prefix("retained worktree removal: ")
    {
        record.retirement = Some(RetirementRequest {
            authority: RetirementAuthority::Retained,
            retained_tip: tip.into(),
            ..Default::default()
        });
        record.cleanup_reason.clear();
    }
    Ok(record)
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

fn needs_tick(t: &Thread) -> bool {
    t.status != Status::Resolved
        || t.cleanup_pending
        || t.recovery_pending
        || t.start_notices.iter().any(|notice| !notice.submitted)
}

#[derive(Default)]
struct ThreadLists {
    records: crate::record_cache::Records<Thread>,
    live: std::collections::BTreeMap<PathBuf, (std::rc::Rc<Vec<Thread>>, Vec<Thread>)>,
}

thread_local! {
    static TICKER_LISTS: std::cell::RefCell<Option<ThreadLists>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn set_cache(enabled: bool) {
    TICKER_LISTS.with(|cache| {
        *cache.borrow_mut() = enabled.then(ThreadLists::default);
    });
}

fn cached_list(project: &Project, live: bool) -> Option<Vec<Thread>> {
    TICKER_LISTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let cache = cache.as_mut()?;
        let dir = threads_dir(project);
        let (rows, _) = cache.records.read(dir.clone(), |id| load(project, id));
        if !live {
            return Some(rows.as_ref().clone());
        }
        let entry = cache.live.entry(dir).or_insert_with(|| {
            let live = rows.iter().filter(|t| needs_tick(t)).cloned().collect();
            (rows.clone(), live)
        });
        if !std::rc::Rc::ptr_eq(&entry.0, &rows) {
            entry.1 = rows.iter().filter(|t| needs_tick(t)).cloned().collect();
            entry.0 = rows;
        }
        Some(entry.1.clone())
    })
}

pub(crate) fn snapshot(project: &Project) -> std::rc::Rc<Vec<Thread>> {
    TICKER_LISTS
        .with(|cache| {
            cache.borrow_mut().as_mut().map(|cache| {
                cache
                    .records
                    .read(threads_dir(project), |id| load(project, id))
                    .0
            })
        })
        .unwrap_or_else(|| std::rc::Rc::new(list_with_errors(project).0))
}

pub(crate) fn list(project: &Project) -> Vec<Thread> {
    cached_list(project, false).unwrap_or_else(|| list_with_errors(project).0)
}

/// Working lanes and durable cleanup obligations. Full CLI/history reads and
/// historical classification continue to use `list`, not this live index.
pub(crate) fn list_live(project: &Project) -> Vec<Thread> {
    cached_list(project, true)
        .unwrap_or_else(|| list(project).into_iter().filter(needs_tick).collect())
}

#[cfg(test)]
thread_local! {
    static THREAD_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn count_thread_reads(f: impl FnOnce()) -> usize {
    THREAD_READS.with(|count| {
        let previous = count.replace(0);
        f();
        count.replace(previous)
    })
}

fn write_record(project: &Project, thread: &Thread) -> Result<()> {
    let dir = threads_dir_for_write(project)?;
    write_atomic(
        &dir.join(format!("{}.toml", thread.id)),
        toml::to_string(thread)?.as_bytes(),
    )
}

/// Serialize brief and follow-up transport for one lane. The project lock
/// remains free while an agent consumes the prompt and writes its receipt.
pub(crate) fn prompt_lock(project: &Project, id: &str) -> Result<std::fs::File> {
    let lock = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(project.state_dir().join(format!("brief-{id}.lock")))?;
    lock.lock()?;
    Ok(lock)
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
    if thread.attempt != before.attempt || thread.pane_id != before.pane_id {
        thread.brief_submitted = false;
        thread.brief_submitted_at.clear();
    }
    if thread.attempt != before.attempt {
        // A receipt or answered wait proves one exact attempt; a replacement
        // must earn its own.
        thread.bootstrap.clear();
        thread.answered_waiting_event.clear();
        thread.has_changes = None;
        thread.changes_seal.clear();
        thread.merged_sha.clear();
        thread.merged_review.clear();
        thread.review_after.clear();
        thread.review_reason.clear();
        for follow_up in &mut thread.follow_ups {
            if follow_up.attempt == before.attempt.max(1)
                && matches!(
                    follow_up.state,
                    FollowUpState::Queued | FollowUpState::Uncertain
                )
            {
                follow_up.carried_from_attempt = before.attempt.max(1);
                follow_up.attempt = thread.attempt.max(1);
                follow_up.state = FollowUpState::Queued;
                follow_up.delivered_at.clear();
                follow_up.closed_at.clear();
                follow_up.after_seal.clear();
            }
        }
    }
    if thread.status == Status::Resolved && before.status != Status::Resolved {
        let disposition = if thread.resolved_reason == "cancelled" {
            FollowUpState::Cancelled
        } else {
            FollowUpState::Closed
        };
        for (index, follow_up) in thread.follow_ups.iter_mut().enumerate() {
            if matches!(
                follow_up.state,
                FollowUpState::Queued | FollowUpState::Uncertain
            ) {
                thread.start_notices.push(crate::steps::Notice {
                    line: format!(
                        "{} follow-up {} was not delivered: lane resolved ({})",
                        thread.id,
                        index + 1,
                        thread.resolved_reason
                    ),
                    submitted: false,
                });
            }
            if matches!(
                follow_up.state,
                FollowUpState::Queued | FollowUpState::Uncertain | FollowUpState::Delivered
            ) {
                follow_up.state = disposition;
                follow_up.closed_at = project::now();
            }
        }
    }
    thread.updated = project::now();
    write_record(project, &thread)?;
    Ok(thread)
}

/// Allocates the next id under the project lock and writes the first record.
pub(crate) fn allocate(project: &Project, fill: impl FnOnce(&mut Thread)) -> Result<Thread> {
    let _lock = project.lock()?;
    let status = project.status();
    if status != crate::project::Status::Active {
        bail!(
            "`{}` is {status}; new work is refused until it is active again",
            project.slug
        );
    }
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
    let mut continuation = if t.last_failure.is_empty() {
        String::new()
    } else {
        format!(
            " Continue the preserved worktree; do not reset or discard changes. The previous attempt reported this failure: {}.",
            serde_json::to_string(&t.last_failure).unwrap_or_default()
        )
    };
    let queued: Vec<_> = t
        .follow_ups
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.attempt == t.attempt.max(1)
                && f.carried_from_attempt > 0
                && f.state == FollowUpState::Queued
        })
        .map(|(index, f)| format!("Follow-up {}:\n{}", index + 1, f.text))
        .collect();
    if !queued.is_empty() {
        continuation.push_str(&format!(
            "\n\nRead your frozen brief at {}/brief.md and sealed report at {}. Continue in the same folder; apply these instructions after the brief:\n\n{}",
            t.thread_dir, t.report_path(), queued.join("\n\n"),
        ));
    }
    if t.is_remote() {
        return format!(
            "Run the shell command `{prefix} skill {role}`, then read .herdr-project/{slug}-{id}/brief.md and do what it says. You run on the cloud box named `{}`; finish with `ha done`, never with a parent prompt.{continuation}",
            t.machine
        );
    }
    let prompt = match t.kind {
        Kind::Worktree if !t.is_remote() => format!(
            "Run the shell command `{prefix} skill {role}`, then read .herdr-project/{slug}-{id}/brief.md and do what it says."
        ),
        Kind::Tab => {
            format!(
                "Run the shell command `{prefix} skill {role}`, then read brief.md and do what it says."
            )
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

pub(crate) use crate::events::store_artifact;

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
    format!("Commands: `{prefix}`. Use this prefix instead of `ha` if it differs.\n\n")
}

/// A brief read without `ha skill` (adopted, remote) carries the lane skill.
pub(crate) fn with_lane_skill(prefix: &str, brief: &str) -> String {
    format!(
        "{}{}\n\n{brief}",
        commands_line(prefix),
        include_str!("../skill/LANE.md").trim_end()
    )
}

/// The largest explicitly scoped fact payload carried by a task's brief.
/// Unscoped facts are delivered as a frozen attachment, not copied prose.
#[derive(Debug, Clone, Default)]
pub(crate) struct MemoryUse {
    notes: Vec<(String, String)>,
    total_chars: usize,
}

impl MemoryUse {
    fn from_rows<'a>(rows: impl Iterator<Item = &'a crate::note::Row>) -> Self {
        let notes: Vec<_> = rows
            .filter(|row| brief_carries_memory(row) && !row.tasks.is_empty())
            .map(|row| (row.id.clone(), render_brief_row(row)))
            .collect();
        let total_chars = notes
            .iter()
            .map(|(id, text)| memory_block(id, text).chars().count())
            .sum();
        Self { notes, total_chars }
    }

    /// The existing `ha doctor` advisory; this never controls brief contents.
    pub(crate) fn warning(&self) -> Option<String> {
        if self.total_chars <= MEMORY_WARNING_CHARS {
            return None;
        }
        let parts = self
            .notes
            .iter()
            .map(|(id, text)| format!("{id} {}", memory_block(id, text).chars().count()))
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "large scoped fact payload: {} characters ({parts}); all retained; replace stale dated notes",
            self.total_chars
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
    row.kind == "standing instruction"
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
            let mut applicable: Vec<_> = rows.iter().filter(|row| row.applies_to(task)).collect();
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
    pub(crate) paths: &'a [String],
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

    for (id, text) in input.facts {
        brief.push_str(&memory_block(id, text));
    }
    if input.facts.is_empty() {
        brief.push_str("\nNone scoped to this task.\n");
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
    if !input.paths.is_empty() {
        brief.push_str("\n# Writable paths\n\n");
        for path in input.paths {
            brief.push_str(&format!("- `{path}`\n"));
        }
    }
    brief.push_str(&format!(
        "\n# Finish\n\nCommit repository changes if any; leave runtime deliverables untracked; run `ha done`.\n\n# Paths\n\n- Report: `{}`\n- Library folder for files meant for Rolf: `{}`\n",
        input.report_path, input.library_path
    ));
    brief
}

fn render_task(
    project: &Project,
    record: &crate::task::Task,
    rows: &[crate::note::Row],
) -> Result<String> {
    let mut out = format!(
        "## {} — {}\n\nRequests: {}\n",
        record.id,
        record.title.trim(),
        record.authority.join(", ")
    );
    for basis in &record.authority {
        if basis.starts_with("request:") {
            let request = crate::prompt::resolve_request(project, basis)?;
            out.push_str(&format!("\n{basis}:\n{}\n", request.text));
        }
    }
    out.push_str("\nAcceptance conditions:\n");
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
    Ok(out.trim_end().to_string())
}

/// Builds a frozen helper brief from the same current records as PROJECT.md.
/// Unscoped factual prose is pinned as an artifact. Remote lanes also record
/// it in the existing attachment map so placement stages a usable local path.
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
    let task = match task_record.as_ref() {
        Some(record) => render_task(project, record, &active)?,
        None => supplied_task.trim().to_string(),
    };
    let supplied_task = task_record.as_ref().map(|_| supplied_task);
    let (settings, _) = project.read_project_md()?;
    let repo = settings.repos.iter().find(|repo| repo.path == thread.repo);
    let gates = repo.and_then(|repo| repo.gates.as_deref());
    let machine = if thread.machine.is_empty() {
        "local"
    } else {
        &thread.machine
    };
    let mut brief = compose_brief(&BriefInput {
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
        paths: &thread.paths,
    });
    for name in thread.attachments.keys() {
        brief.push_str(&format!(
            "- Attachment: `{}/attachments/{name}`\n",
            thread.thread_dir
        ));
    }
    let unscoped: Vec<_> = active
        .iter()
        .filter(|row| brief_carries_memory(row) && row.tasks.is_empty())
        .collect();
    if !unscoped.is_empty() {
        let details = unscoped
            .iter()
            .map(|row| memory_block(&row.id, &render_brief_row(row)))
            .collect::<String>();
        let hash = store_artifact(project, details.as_bytes())?;
        let path = if thread.is_remote() {
            let mut name = format!("brief-facts-{hash}.md");
            update(project, &thread.id, |record| {
                // Keep even a supplied attachment with the generated basename.
                while record
                    .attachments
                    .get(&name)
                    .is_some_and(|old| old != &hash)
                {
                    name.insert(0, '_');
                }
                record.attachments.insert(name.clone(), hash.clone());
            })?;
            Path::new(&thread.thread_dir).join("attachments").join(name)
        } else {
            project.state_dir().join("artifacts").join(hash)
        };
        brief.push_str(&format!(
            "- Unscoped facts ({}): `{}`. Frozen text and provenance; consult when needed.\n",
            unscoped
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            path.display()
        ));
    }
    Ok(brief)
}

// ---------------------------------------------------------------- groups

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    ReadyForReview,
    Parked,
    WaitingOnYou,
    Unknown,
    Working,
    Idle,
    Resolved,
}

impl Group {
    /// Display order, shared by the sidebar `rank` token and the overview:
    /// separate from the precedence in `group()`.
    pub(crate) fn rank(self) -> u8 {
        match self {
            Group::ReadyForReview => 1,
            Group::Parked => 2,
            Group::WaitingOnYou => 3,
            Group::Unknown => 4,
            Group::Working => 5,
            Group::Idle => 6,
            Group::Resolved => 7,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Group::ReadyForReview => "Ready for review",
            Group::Parked => "Parked",
            Group::WaitingOnYou => "Needs attention",
            Group::Unknown => "Unknown",
            Group::Working => "Working",
            Group::Idle => "Idle",
            Group::Resolved => "Resolved",
        }
    }

    /// Lower-case hyphenated form, used in the `review` token and `last_group`.
    pub(crate) fn token(self) -> &'static str {
        match self {
            Group::ReadyForReview => "ready-for-review",
            Group::Parked => "parked",
            Group::WaitingOnYou => "waiting-on-you",
            Group::Unknown => "unknown",
            Group::Working => "working",
            Group::Idle => "idle",
            Group::Resolved => "resolved",
        }
    }

    pub(crate) fn from_token(token: &str) -> Option<Group> {
        [
            Group::ReadyForReview,
            Group::Parked,
            Group::WaitingOnYou,
            Group::Unknown,
            Group::Working,
            Group::Idle,
            Group::Resolved,
        ]
        .into_iter()
        .find(|g| g.token() == token)
    }

    pub(crate) const DISPLAY_ORDER: [Group; 7] = [
        Group::ReadyForReview,
        Group::Parked,
        Group::WaitingOnYou,
        Group::Unknown,
        Group::Working,
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
        .map(|then| crate::awake::elapsed(then, now))
        .unwrap_or(0)
}

/// The group of a thread. First matching row wins. One function, so the CLI
/// and the ticker always agree.
pub(crate) fn recorded_group(thread: &Thread, now: jiff::Timestamp) -> Group {
    project_group(thread, None, now)
}

pub(crate) fn group(thread: &Thread, live: &Live, now: jiff::Timestamp) -> Group {
    project_group(thread, Some(live), now)
}

fn project_group(thread: &Thread, live: Option<&Live>, now: jiff::Timestamp) -> Group {
    // 1
    if thread.status == Status::Resolved {
        return Group::Resolved;
    }
    if thread.status == Status::Failed {
        return Group::WaitingOnYou;
    }
    if thread.parked {
        return Group::Parked;
    }
    if thread.recovery_pending {
        return Group::Working;
    }
    if thread.connection_waiting {
        return Group::WaitingOnYou;
    }
    // 2
    if thread.status == Status::Starting {
        return if thread.recovery_pending
            || !thread.startup_wait_started.is_empty()
            || !thread.provider_wait_started.is_empty()
            || seconds_since(&thread.created, now) < STARTING_TIMEOUT_SECS
        {
            Group::Working
        } else {
            Group::WaitingOnYou
        };
    }
    // An early startup block is not a user-facing block until its ready
    // window expires. The ticker makes the timed failure transition.
    if !thread.startup_wait_started.is_empty() {
        return Group::Working;
    }
    let Some(live) = live else {
        return if thread.is_remote() && thread.last_state.is_empty() {
            Group::Unknown
        } else {
            Group::from_token(&thread.last_group).unwrap_or(if thread.prompt_pending {
                Group::Working
            } else {
                Group::Idle
            })
        };
    };
    let state = live.agent_state.as_deref();
    let has_report = !thread.report_hash.is_empty();
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
    if has_report && thread.report_hash != thread.acked_report_hash {
        return Group::ReadyForReview;
    }
    // 6
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

/// A failed local lane whose agent missed the startup ready window may
/// reclaim its original pane even when a wrapper omitted the agent name.
/// The pane, worktree and kind provide identity evidence.
pub(crate) fn recoverable_agent<'a>(
    lane: &Thread,
    threads: &[Thread],
    agents: &'a [Agent],
    panes: &[Pane],
) -> Option<&'a Agent> {
    if lane.status != Status::Failed
        || lane.role == "reviewer"
        || lane.is_remote()
        || lane.parked
        || lane.recovery_pending
        || !lane.error.starts_with("agent_not_ready:")
        || lane.worktree_path.is_empty()
        || lane.pane_id.is_empty()
        || threads.iter().any(|other| {
            other.id != lane.id && other.status != Status::Resolved && other.pane_id == lane.pane_id
        })
        || !panes.iter().any(|pane| pane_matches(lane, pane))
    {
        return None;
    }
    agents.iter().find(|agent| {
        agent.pane_id == lane.pane_id
            && agent.tab_id == lane.tab_id
            && agent.workspace_id == lane.workspace_id
            && agent.agent == lane.launch.kind
            && agent.cwd == lane.worktree_path
            && agent.cwd == lane.cwd
    })
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

pub(crate) fn agent_start_timeout(launch: &crate::contracts::Launch) -> u64 {
    if launch.ready_timeout_ms == 0 {
        crate::herdr::AGENT_START_TIMEOUT.as_millis() as u64
    } else {
        launch.ready_timeout_ms
    }
}

/// Placement gives the pane an observation grace period; agent submission
/// resets this clock for its full ready window. No clock means no grace period.
pub(crate) fn in_start_window(thread: &Thread, now: jiff::Timestamp) -> bool {
    !thread.startup_wait_started.is_empty()
        && seconds_since(&thread.startup_wait_started, now).max(0) as u64 * 1000
            < agent_start_timeout(&thread.launch)
}

pub(crate) fn process_bound_to_pane(thread: &Thread) -> bool {
    thread.identity.process.is_some()
        && thread.identity.pane_id == thread.pane_id
        && thread.identity.tab_id == thread.tab_id
        && thread.identity.workspace_id == thread.workspace_id
}

/// Intentional pane closure and pending placement are not process deaths.
pub(crate) fn can_check_gone(thread: &Thread, now: jiff::Timestamp) -> bool {
    !thread.parked
        && !thread.recovery_pending
        && thread.provider_wait_started.is_empty()
        && !in_start_window(thread, now)
}

/// A shell is not a dead agent before submission. An unregistered launch can
/// be checked after its ready window; a bound process supplies direct evidence.
pub(crate) fn can_check_process_gone(thread: &Thread, now: jiff::Timestamp) -> bool {
    can_check_gone(thread, now)
        && (process_bound_to_pane(thread)
            || (thread.launch_attempts > 0 && !thread.startup_wait_started.is_empty()))
}

pub(crate) fn bind_identity(
    thread: &mut Thread,
    socket: &str,
    agent: &Agent,
    process: Option<crate::contracts::ProcessIdentity>,
) {
    let resumed_session = (thread.bootstrap == "resuming")
        .then(|| thread.identity.agent_session.clone())
        .flatten();
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
            .filter(|s| !s.is_empty())
            .or(resumed_session),
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
    format!("{:x}", Sha256::digest(bytes))
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
    fn ticker_reuses_unchanged_thread_records_and_sees_atomic_updates() {
        let home = tempfile::tempdir().unwrap();
        let project = crate::project::create(home.path(), "demo", "", vec![]).unwrap();
        let original = allocate(&project, |t| t.title = "Original".into()).unwrap();
        let _cache = crate::record_cache::Cache::new();
        assert_eq!(list(&project)[0].title, "Original");
        // The unchanged directory is not parsed a second time.
        let path = threads_dir(&project).join(format!("{}.toml", original.id));
        std::fs::write(&path, "invalid = [").unwrap();
        assert_eq!(list(&project)[0].title, "Original");
        // Real writers use atomic replacement and invalidate the snapshot.
        write_record(&project, &original).unwrap();
        update(&project, &original.id, |t| t.title = "Updated".into()).unwrap();
        assert_eq!(list(&project)[0].title, "Updated");
    }

    #[test]
    fn settled_start_notices_stay_in_the_live_index_until_submitted() {
        let home = tempfile::tempdir().unwrap();
        let project = crate::project::create(home.path(), "demo", "", vec![]).unwrap();
        let lane = allocate(&project, |t| {
            t.status = Status::Resolved;
            t.start_notices.push(crate::steps::Notice {
                line: "start failed".into(),
                submitted: false,
            });
        })
        .unwrap();
        let _cache = crate::record_cache::Cache::new();
        assert_eq!(list_live(&project).len(), 1);
        update(&project, &lane.id, |t| t.start_notices[0].submitted = true).unwrap();
        assert!(list_live(&project).is_empty());
        assert_eq!(list(&project).len(), 1);
    }

    #[test]
    fn ticker_retries_unreadable_records_without_directory_changes() {
        let home = tempfile::tempdir().unwrap();
        let project = crate::project::create(home.path(), "demo", "", vec![]).unwrap();
        let lane = allocate(&project, |t| t.title = "Original".into()).unwrap();
        let path = threads_dir(&project).join(format!("{}.toml", lane.id));
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, "invalid = [").unwrap();
        let _cache = crate::record_cache::Cache::new();
        assert!(list(&project).is_empty());
        // Repair in place, not via the atomic writer: no directory rename.
        std::fs::write(&path, original).unwrap();
        assert_eq!(list(&project)[0].title, "Original");
    }

    #[test]
    fn an_unpolled_remote_thread_is_unknown() {
        let remote = Thread {
            machine: "box".into(),
            last_state: String::new(),
            ..open_thread()
        };
        assert_eq!(recorded_group(&remote, now()), Group::Unknown);
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
        assert!(process_bound_to_pane(&t));
        assert!(can_check_process_gone(&t, now()));
        for field in ["pane", "tab", "workspace"] {
            let mut stale = t.clone();
            match field {
                "pane" => stale.identity.pane_id = "old-pane".into(),
                "tab" => stale.identity.tab_id = "old-tab".into(),
                _ => stale.identity.workspace_id = "old-workspace".into(),
            }
            assert!(!process_bound_to_pane(&stale), "{field}");
            assert!(!can_check_process_gone(&stale, now()), "{field}");
        }
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
    fn ids_branches_and_dirs() {
        assert!(validate_id("t-0001").is_ok());
        assert!(validate_id("t-12345").is_ok());
        for bad in ["", "t-1", "t-00a1", "../t-0001", "x-0001"] {
            assert!(validate_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn historical_pr_fields_do_not_prevent_loading_a_thread() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let lane = allocate(&project, |lane| {
            lane.title = "Old lane".into();
            lane.kind = Kind::Tab;
        })
        .unwrap();
        let path = record_path(&project, &lane.id);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("attachments"));
        assert!(!saved.contains("retirement"));
        assert!(!saved.contains("paths"));
        let legacy = format!(
            "pr = \"https://github.com/acme/demo/pull/1\"\npr_state = \"OPEN\"\npr_review = \"APPROVED\"\npr_note = \"\"\npr_summary = {{ state = \"OPEN\", comments = [] }}\n{saved}"
        );
        std::fs::write(&path, legacy).unwrap();
        let loaded = load(&project, &lane.id).unwrap();
        assert_eq!(loaded.title, "Old lane");
        assert_eq!(loaded.kind, Kind::Tab);
        assert!(loaded.paths.is_empty());
        assert!(loaded.attachments.is_empty());
        assert!(loaded.retirement.is_none());
        assert_eq!(list_with_errors(&project).0.len(), 1);
        update(&project, &lane.id, |lane| {
            lane.title = "Updated lane".into()
        })
        .unwrap();
        assert_eq!(load(&project, &lane.id).unwrap().title, "Updated lane");
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("pr_summary"));
        std::fs::write(
            &path,
            format!("cleanup_reason = \"retained worktree removal: old-tip\"\n{saved}"),
        )
        .unwrap();
        let historical = load(&project, &lane.id).unwrap();
        let request = historical.retirement.unwrap();
        assert_eq!(request.authority, RetirementAuthority::Retained);
        assert_eq!(request.retained_tip, "old-tip");
        update(&project, &lane.id, |lane| {
            lane.cleanup_reason = "scratch busy".into()
        })
        .unwrap();
        assert_eq!(load(&project, &lane.id).unwrap().retirement, Some(request));
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
                ..FollowUp::default()
            });
        })
        .unwrap();
        let carried = update(&project, &lane.id, |lane| lane.attempt = 2).unwrap();
        assert_eq!(carried.follow_ups[0].state, FollowUpState::Queued);
        assert_eq!(carried.follow_ups[0].attempt, 2);
        assert_eq!(carried.follow_ups[0].carried_from_attempt, 1);

        let cancelled = update(&project, &lane.id, |lane| {
            lane.follow_ups.push(FollowUp {
                attempt: 2,
                text: "cancel this".into(),
                state: FollowUpState::Queued,
                ..FollowUp::default()
            });
            lane.status = Status::Resolved;
            lane.resolved_reason = "cancelled".into();
        })
        .unwrap();
        assert_eq!(cancelled.follow_ups[0].state, FollowUpState::Cancelled);
        assert_eq!(cancelled.follow_ups[1].state, FollowUpState::Cancelled);
        assert_eq!(cancelled.start_notices.len(), 2);
        assert!(
            cancelled.start_notices[0]
                .line
                .contains("follow-up 1 was not delivered")
        );
        assert!(
            cancelled.start_notices[1]
                .line
                .contains("follow-up 2 was not delivered")
        );
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
    fn large_scoped_facts_warn_but_remain_in_the_brief() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        crate::prompt::record_test_request(&project, "q-1", "Keep helper briefs focused.").unwrap();
        let lane = allocate(&project, |_| {}).unwrap();
        let task = crate::task::add(
            &project,
            "Keep required evidence",
            vec!["request:q-1".into()],
            vec!["Retain all scoped facts.".into()],
            None,
            None,
        )
        .unwrap();
        crate::task::link_attempt(&project, &task.id, &lane.id).unwrap();
        let text = "x".repeat(MEMORY_WARNING_CHARS + 1);
        let note = crate::note::add(
            &project,
            crate::note::Kind::Memory,
            &text,
            "q-1",
            None,
            vec![task.id.clone()],
        )
        .unwrap();
        let last = crate::note::add(
            &project,
            crate::note::Kind::Memory,
            "Required final evidence.",
            "q-1",
            None,
            vec![task.id.clone()],
        )
        .unwrap();
        let warning = memory_use(&project).warning().unwrap();
        assert!(warning.contains(&note.id), "{warning}");
        let brief = brief_for(&project, &lane, "Build from the required evidence.", false).unwrap();
        assert!(brief.contains(&text));
        assert!(brief.contains(&last.text));
        assert!(!brief.contains("Not included"));

        crate::note::add(
            &project,
            crate::note::Kind::Memory,
            "short",
            "q-1",
            Some(&note.id),
            vec![task.id],
        )
        .unwrap();
        assert!(memory_use(&project).warning().is_none());
    }

    #[test]
    fn bounded_briefs_keep_scope_acceptance_instructions_and_retrievable_facts() {
        for remote in [false, true] {
            let fx = crate::testkit::fixture();
            let project = &fx.project;
            crate::prompt::record_test_request(
                project,
                "q-scope",
                "Research the trust dialog, then repair startup.",
            )
            .unwrap();
            let task = crate::task::add(
                project,
                "Trust dialog",
                vec!["request:q-scope".into()],
                vec![
                    "Read both required sources; report unavailable coverage.".into(),
                    "Answer only the exact managed-worktree trust dialog.".into(),
                ],
                None,
                None,
            )
            .unwrap();
            let other = crate::task::add(
                project,
                "Billing",
                vec!["request:q-scope".into()],
                vec!["Unrelated task.".into()],
                None,
                None,
            )
            .unwrap();
            let lane = allocate(project, |t| {
                t.machine = if remote { "oci" } else { "" }.into();
                t.thread_dir = "/lane/.herdr-project/demo-t-0001".into();
                t.paths = vec!["src/claude_trust.rs".into()];
            })
            .unwrap();
            crate::task::link_attempt(project, &task.id, &lane.id).unwrap();
            let global = crate::note::add(
                project,
                crate::note::Kind::Instruction,
                "Never call the Agent tool or copy credentials.",
                "q-scope",
                None,
                vec![],
            )
            .unwrap();
            let scoped = crate::note::add(
                project,
                crate::note::Kind::Instruction,
                "Preserve exact trust proof and verified deletes.",
                "q-scope",
                None,
                vec![task.id.clone()],
            )
            .unwrap();
            let required = crate::note::add(
                project,
                crate::note::Kind::Memory,
                "Required evidence: src/claude_trust.rs and attachments/dialog.txt.",
                "q-scope",
                None,
                vec![task.id.clone()],
            )
            .unwrap();
            let incidental = crate::note::add(
                project,
                crate::note::Kind::Memory,
                "Other task's incidental billing history.",
                "q-scope",
                None,
                vec![other.id.clone()],
            )
            .unwrap();
            let unscoped = crate::note::add(
                project,
                crate::note::Kind::Memory,
                "Old global billing history, kept as reference only.",
                "q-scope",
                None,
                vec![],
            )
            .unwrap();
            for (id, text) in [
                (&task.id, "Historical task source: dialog revision 2."),
                (&other.id, "Other historical task note."),
            ] {
                let mut record = crate::task::load(project, id).unwrap();
                record.notes.push(crate::task::DatedNote {
                    id: String::new(),
                    at: String::new(),
                    request: String::new(),
                    text: text.into(),
                    replaces: None,
                });
                std::fs::write(
                    project.record_dir("tasks").join(format!("{id}.toml")),
                    toml::to_string(&record).unwrap(),
                )
                .unwrap();
            }
            let lead = "## Required source scope\nRead src/claude_trust.rs and attachments/dialog.txt in full.\nReport partial access honestly; retain the exact dialog and path.";
            let view_before = crate::project_view::View::load(&fx.world.ctx(), project, None)
                .unwrap()
                .render(&[
                    "Task notes in force",
                    "Standing instructions in force",
                    "Facts in force",
                ]);
            let brief = brief_for(project, &lane, lead, false).unwrap();
            let view_after = crate::project_view::View::load(&fx.world.ctx(), project, None)
                .unwrap()
                .render(&[
                    "Task notes in force",
                    "Standing instructions in force",
                    "Facts in force",
                ]);
            assert_eq!(
                view_before, view_after,
                "N2's handoff source is not reduced"
            );
            assert!(view_after.contains(&unscoped.text));
            for text in [
                lead,
                &global.text,
                &scoped.text,
                &required.text,
                "Historical task source: dialog revision 2.",
            ] {
                assert!(brief.contains(text), "{brief}");
            }
            for condition in &task.acceptance {
                assert!(brief.contains(condition));
            }
            assert!(brief.contains("Research the trust dialog, then repair startup."));
            assert!(!brief.contains(&incidental.text));
            assert!(!brief.contains("Other historical task note."));
            assert!(!brief.contains(&unscoped.text));
            assert!(brief.contains(&unscoped.id));
            assert!(brief.contains("Commit repository changes if any; leave runtime deliverables untracked; run `ha done`."));
            assert!(!brief.contains("--sha"));
            let frozen = if remote {
                let saved = load(project, &lane.id).unwrap();
                let (name, hash) = saved.attachments.iter().next().unwrap();
                assert!(brief.contains(&format!("{}/attachments/{name}", lane.thread_dir)));
                artifact(project, hash).unwrap()
            } else {
                let pointer = brief
                    .lines()
                    .find(|line| line.starts_with("- Unscoped facts"))
                    .unwrap();
                std::fs::read(pointer.split('`').nth(1).unwrap()).unwrap()
            };
            let frozen = String::from_utf8(frozen).unwrap();
            assert!(frozen.contains(&unscoped.text));
            assert!(frozen.contains("request:q-scope"));
            assert!(!frozen.contains(&incidental.text));
            crate::note::retire(
                project,
                &unscoped.id,
                "q-scope",
                "Superseded billing history.",
            )
            .unwrap();
            // Retirement does not rewrite the already delivered reference.
            assert!(frozen.contains(&unscoped.text));
        }
    }

    #[test]
    fn chainlm_trust_matched_briefs_preserve_acceptance_and_recovery_contract() {
        let fx = crate::testkit::fixture();
        crate::prompt::record_test_request(
            &fx.project,
            "q-chainlm",
            "Repair chainlm's folder-trust startup interruption.",
        )
        .unwrap();
        let task = crate::task::add(
            &fx.project,
            "Chainlm trust recovery",
            vec!["request:q-chainlm".into()],
            vec!["Only answer the managed worktree's exact trust dialog.".into()],
            None,
            None,
        )
        .unwrap();
        let lane = allocate(&fx.project, |t| {
            t.last_failure = "Folder trust blocked startup; preserved worktree.".into();
            t.thread_dir = "/work/chainlm/.herdr-project/demo-t-0001".into();
        })
        .unwrap();
        crate::task::link_attempt(&fx.project, &task.id, &lane.id).unwrap();
        let recovery = crate::note::add(&fx.project, crate::note::Kind::Instruction,
            "Never blind-answer trust; inspect exact dialog and managed path. Continue the preserved branch, never reset it.",
            "q-chainlm", None, vec![]).unwrap();
        let history = crate::note::add(
            &fx.project,
            crate::note::Kind::Memory,
            &"Unrelated settled billing history. ".repeat(100),
            "q-chainlm",
            None,
            vec![],
        )
        .unwrap();
        let lead = "Read src/claude_trust.rs and the supplied dialog. Report unknown when exact trust evidence is missing.";
        let reduced = brief_for(&fx.project, &lane, lead, true).unwrap();
        let mut active = crate::note::active_for(&fx.project, Some(&task.id));
        crate::note::sort_newest_first(&mut active);
        // Reconstruct the pre-change brief's uncapped (<32k) fact payload,
        // with the same task, base, source scope, restart and instructions.
        let facts = active
            .iter()
            .filter(|row| brief_carries_memory(row))
            .map(|row| (row.id.clone(), render_brief_row(row)))
            .collect::<Vec<_>>();
        let original = compose_brief(&BriefInput {
            task: &render_task(&fx.project, &task, &active).unwrap(),
            supplied_task: Some(lead),
            instructions: &active
                .iter()
                .filter(|row| brief_carries_instruction(row))
                .map(render_brief_row)
                .collect::<Vec<_>>()
                .join("\n\n"),
            facts: &facts,
            repository: "",
            machine: "local",
            gates: None,
            restart: true,
            report_path: &lane.report_path(),
            library_path: &lane.library_path(),
            paths: &[],
        });
        assert!(original.contains(&history.text));
        assert!(!reduced.contains(&history.text));
        assert!(reduced.len() < original.len());
        eprintln!(
            "chainlm trust matched fixture: brief bytes {} -> {}; acceptance, source scope, exact-trust and restart guidance retained",
            original.len(),
            reduced.len()
        );
        for brief in [&original, &reduced] {
            for retained in [
                &task.acceptance[0],
                &recovery.text,
                lead,
                "Read its report at the report path below first, look at what is already on the branch, and continue from there.",
                "Commit repository changes if any; leave runtime deliverables untracked; run `ha done`.",
            ] {
                assert!(brief.contains(retained));
            }
        }
        let prompt = launch_prompt("ha", "demo", &lane);
        assert!(
            prompt.contains("Continue the preserved worktree; do not reset or discard changes.")
        );
        assert!(prompt.contains(&lane.last_failure));
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
                usage: None,
                id: "done-1".into(),
                op: "done-1".into(),
                thread: sealed.id.clone(),
                attempt: 1,
                recipient: Recipient::default(),
                created: project::now(),
                payload: EventPayload {
                    done: Some(DonePayload {
                        has_changes: None,
                        sha: "abc".into(),
                        report_path: "old/location".into(),
                        artifact: hash.clone(),
                        attestation: None,
                        published_ref: None,
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
        assert!(matches!(copied.outcome, CopyOutcome::Partial(_)));
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
