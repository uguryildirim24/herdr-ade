//! Stable project tasks and their evidence-derived state.
//!
//! Task records contain intent and links, never a writable status. Every view
//! calls [`view`] so the project page, context, plans and talk use the same
//! projection.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::{FailureClass, RoundPhase};
use crate::project::{self, Project, write_atomic};

pub(crate) const STATES: [&str; 5] = ["finished", "reviewed", "merged", "installed", "verified"];
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DatedNote {
    /// Empty on historical notes written before provenance was required.
    #[serde(default)]
    pub(crate) id: String,
    pub(crate) at: String,
    /// The request id behind this note. Empty historical notes are displayed
    /// as undated rather than being assigned guessed authority.
    #[serde(default)]
    pub(crate) request: String,
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) replaces: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DropEvidence {
    pub(crate) at: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AcceptanceWithdrawal {
    pub(crate) acceptance: usize,
    pub(crate) at: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Evidence {
    pub(crate) at: String,
    pub(crate) command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) acceptance: Vec<usize>,
    /// Installation evidence is machine-specific. Historical and verification
    /// evidence has no machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) machine: Option<String>,
    /// The exact repository commit carried by that machine's build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) build: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RunningEvidence {
    pub(crate) at: String,
    pub(crate) command: String,
    pub(crate) machines: Vec<String>,
    pub(crate) processes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct Task {
    pub(crate) schema: u32,
    pub(crate) id: String,
    pub(crate) title: String,
    /// `request:<id>` or `ask:<id>@<revision>`, all validated at creation.
    pub(crate) authority: Vec<String>,
    pub(crate) acceptance: Vec<String>,
    /// An explicit older note, instruction, decision or task this task supersedes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) replaces: Option<String>,
    pub(crate) notes: Vec<DatedNote>,
    #[serde(default)]
    pub(crate) dropped: Vec<DropEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) withdrawn: Vec<AcceptanceWithdrawal>,
    pub(crate) attempts: Vec<String>,
    pub(crate) rounds: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) plan_step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) repo: Option<String>,
    pub(crate) installed: Vec<Evidence>,
    /// A successful check that every running harness process uses an installed
    /// image. This is deliberately separate from acceptance verification.
    #[serde(default)]
    pub(crate) running: Vec<RunningEvidence>,
    pub(crate) verified: Vec<Evidence>,
    pub(crate) created: String,
}

impl Default for Task {
    fn default() -> Self {
        Self {
            schema: 1,
            id: String::new(),
            title: String::new(),
            authority: Vec::new(),
            acceptance: Vec::new(),
            replaces: None,
            notes: Vec::new(),
            dropped: Vec::new(),
            withdrawn: Vec::new(),
            attempts: Vec::new(),
            rounds: Vec::new(),
            plan_step: None,
            repo: None,
            installed: Vec::new(),
            running: Vec::new(),
            verified: Vec::new(),
            created: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum State {
    Open,
    Working,
    Finished,
    Reviewed,
    Merged,
    Installed,
    Verified,
    Failed,
    Cancelled,
    Dropped,
    Unknown,
}

impl State {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Working => "working",
            Self::Finished => "finished",
            Self::Reviewed => "reviewed",
            Self::Merged => "merged",
            Self::Installed => "installed",
            Self::Verified => "verified",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Dropped => "dropped",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct View {
    pub(crate) record: Task,
    pub(crate) state: State,
    pub(crate) next: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) failure_class: Option<FailureClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provider_kind: Option<String>,
}

impl View {
    pub(crate) fn terminal(&self, project: &Project) -> bool {
        if matches!(self.state, State::Cancelled | State::Dropped) {
            return true;
        }
        self.terminal_with_evidence(project, &EvidenceSnapshot::load(project))
    }

    pub(crate) fn terminal_with_evidence(
        &self,
        project: &Project,
        evidence: &EvidenceSnapshot,
    ) -> bool {
        if matches!(self.state, State::Cancelled | State::Dropped) {
            return true;
        }
        required_states_with_events(project, &self.record, &evidence.events)
            .ok()
            .and_then(|states| states.last().cloned())
            .is_some_and(|last| last == self.state.word())
    }

    fn status(&self) -> &'static str {
        if self.state == State::Verified && !running_current(&self.record) {
            "verified; latest process check unknown"
        } else {
            self.state.word()
        }
    }
}

fn dir(project: &Project) -> PathBuf {
    project.record_dir("tasks")
}

fn dir_for_write(project: &Project) -> Result<PathBuf> {
    project.record_dir_for_write("tasks")
}

fn path(project: &Project, id: &str) -> PathBuf {
    dir(project).join(format!("{id}.toml"))
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
    let digits = id.strip_prefix("job-").unwrap_or("");
    if digits.len() < 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
        bail!("task_id: `{id}` is not a task id (expected job-0001)");
    }
    Ok(())
}

pub(crate) fn load(project: &Project, id: &str) -> Result<Task> {
    validate_id(id)?;
    let file = path(project, id);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(crate::refusal::error(format!(
                "task_unknown: no task `{id}` in `{}`",
                project.slug
            )));
        }
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", file.display()));
        }
    };
    let task: Task = toml::from_str(&text)
        .with_context(|| format!("task_unreadable: {} does not parse", file.display()))?;
    validate_record(&task)?;
    if task.id != id {
        bail!(
            "task_identity: {} contains `{}` instead of `{id}`",
            file.display(),
            task.id
        );
    }
    Ok(task)
}

fn validate_record(task: &Task) -> Result<()> {
    if task.schema != 1 {
        bail!("task_schema: expected 1, got {}", task.schema);
    }
    validate_id(&task.id)?;
    if task.title.trim().is_empty() {
        bail!("task_title: the title is empty");
    }
    if task.authority.is_empty() {
        bail!("task_authority: at least one request or answered ask is required");
    }
    if task.acceptance.is_empty() || task.acceptance.iter().any(|line| line.trim().is_empty()) {
        bail!("task_acceptance: at least one plain acceptance condition is required");
    }
    let mut withdrawn = BTreeSet::new();
    for evidence in &task.withdrawn {
        if evidence.acceptance == 0 || evidence.acceptance > task.acceptance.len() {
            bail!(
                "task_withdrawn: acceptance {} is outside 1..={}",
                evidence.acceptance,
                task.acceptance.len()
            );
        }
        if evidence.reason.trim().is_empty() {
            bail!("task_withdrawn: a withdrawal reason is empty");
        }
        if !withdrawn.insert(evidence.acceptance) {
            bail!(
                "task_withdrawn: acceptance {} is withdrawn more than once",
                evidence.acceptance
            );
        }
    }
    if withdrawn.len() == task.acceptance.len() {
        bail!("task_withdrawn: every acceptance condition is withdrawn; drop the task instead");
    }
    let verified: BTreeSet<_> = task
        .verified
        .iter()
        .flat_map(|evidence| evidence.acceptance.iter().copied())
        .collect();
    if let Some(index) = withdrawn.intersection(&verified).next() {
        bail!("task_withdrawn: acceptance {index} also has verification evidence");
    }
    Ok(())
}

fn write(project: &Project, task: &Task) -> Result<()> {
    validate_record(task)?;
    dir_for_write(project)?;
    write_atomic(&path(project, &task.id), toml::to_string(task)?.as_bytes())
}

fn next_id(project: &Project) -> Result<String> {
    let mut highest = 0_u64;
    for entry in std::fs::read_dir(dir(project))? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(id) = name.strip_suffix(".toml") else {
            continue;
        };
        if validate_id(id).is_err() {
            continue;
        }
        let number = id
            .strip_prefix("job-")
            .expect("validated task id")
            .parse::<u64>()
            .with_context(|| format!("task_id: `{id}` is too large"))?;
        highest = highest.max(number);
    }
    let next = highest
        .checked_add(1)
        .context("task_id: the task id space is exhausted")?;
    Ok(format!("job-{next:04}"))
}

pub(crate) fn list_with_errors(project: &Project) -> (Vec<Task>, Vec<anyhow::Error>) {
    let entries = match std::fs::read_dir(dir(project)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), Vec::new());
        }
        Err(error) => return (Vec::new(), vec![error.into()]),
    };
    let mut tasks = Vec::new();
    let mut errors = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(error.into());
                continue;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            errors.push(anyhow::anyhow!(
                "task_unreadable: a task file name is not UTF-8"
            ));
            continue;
        };
        let Some(id) = name.strip_suffix(".toml") else {
            continue;
        };
        match load(project, id) {
            Ok(task) => tasks.push(task),
            Err(error) => errors.push(error),
        }
    }
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    (tasks, errors)
}

pub(crate) fn add(
    project: &Project,
    title: &str,
    authority: Vec<String>,
    acceptance: Vec<String>,
    repo: Option<String>,
    replaces: Option<String>,
) -> Result<Task> {
    if title.trim().is_empty() {
        return Err(crate::refusal::error("task_title: a title is required"));
    }
    if authority.is_empty() {
        return Err(crate::refusal::error(
            "task_authority: pass at least one --request request:<id> or ask:<id>@<revision>",
        ));
    }
    if acceptance.is_empty() || acceptance.iter().any(|line| line.trim().is_empty()) {
        return Err(crate::refusal::error(
            "task_acceptance: pass at least one non-empty --acceptance condition",
        ));
    }
    let title = crate::glossary::check_internal_sentence("task title", title)?;
    let checked_acceptance = acceptance
        .iter()
        .map(|condition| condition.trim().to_string())
        .collect();
    let authority = authority
        .iter()
        .map(|reference| {
            crate::decide::validate_basis(project, reference)
                .map_err(|error| crate::refusal::error(error.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    let _replacement_lock = replaces
        .as_ref()
        .map(|_| crate::note::replacement_lock(project))
        .transpose()?;
    if let Some(old) = replaces.as_deref() {
        if !crate::note::target_exists(project, old) {
            return Err(crate::refusal::error(format!(
                "task_replacement: no note, decision or task `{old}` exists"
            )));
        }
        let rows = crate::note::rows(project);
        if crate::note::replacement_map(&rows).contains_key(old) {
            return Err(crate::refusal::error(format!(
                "task_replacement: `{old}` already has a replacement"
            )));
        }
    }
    let repo = repo.map(|repo| {
        std::fs::canonicalize(&repo)
            .or_else(|_| std::path::absolute(&repo))
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or(repo)
    });
    let _lock = project.lock()?;
    dir_for_write(project)?;
    let task = Task {
        id: next_id(project)?,
        title,
        authority,
        acceptance: checked_acceptance,
        replaces,
        plan_step: None,
        repo,
        created: project::now(),
        ..Task::default()
    };
    write(project, &task)?;
    drop(_lock);
    project::refresh_page(project)?;
    Ok(task)
}

fn update(
    project: &Project,
    id: &str,
    change: impl FnOnce(&mut Task) -> Result<()>,
) -> Result<Task> {
    let task = update_deferred(project, id, change)?;
    project::refresh_page(project)?;
    Ok(task)
}

fn update_deferred(
    project: &Project,
    id: &str,
    change: impl FnOnce(&mut Task) -> Result<()>,
) -> Result<Task> {
    let _lock = project.lock()?;
    let mut task = load(project, id)?;
    change(&mut task)?;
    write(project, &task)?;
    Ok(task)
}

pub(crate) fn drop_task(project: &Project, id: &str, reason: &str) -> Result<Task> {
    if reason.trim().is_empty() {
        return Err(crate::refusal::error("task_drop: --reason is required"));
    }
    update(project, id, |task| {
        if !task.verified.is_empty() {
            return Err(crate::refusal::error(
                "task_drop_verified: a task with verification evidence cannot be dropped",
            ));
        }
        if !task.dropped.is_empty() {
            return Err(crate::refusal::error(format!(
                "task_drop_already: `{id}` is already dropped"
            )));
        }
        task.dropped.push(DropEvidence {
            at: project::now(),
            reason: reason.trim().to_string(),
        });
        Ok(())
    })
}

pub(crate) fn withdraw_acceptance(
    project: &Project,
    id: &str,
    acceptance: Vec<usize>,
    reason: &str,
) -> Result<Task> {
    if reason.trim().is_empty() {
        return Err(crate::refusal::error("task_drop: --reason is required"));
    }
    update(project, id, |task| {
        if !task.dropped.is_empty() {
            return Err(crate::refusal::error(format!(
                "task_drop_already: `{id}` is already dropped"
            )));
        }
        let mut acceptance = acceptance;
        acceptance.sort_unstable();
        acceptance.dedup();
        if acceptance
            .iter()
            .any(|index| *index == 0 || *index > task.acceptance.len())
        {
            return Err(crate::refusal::error(format!(
                "task_drop_acceptance_range: an acceptance number is outside 1..={}",
                task.acceptance.len()
            )));
        }
        let already_withdrawn: BTreeSet<_> = task
            .withdrawn
            .iter()
            .map(|evidence| evidence.acceptance)
            .collect();
        if let Some(index) = acceptance
            .iter()
            .find(|index| already_withdrawn.contains(index))
        {
            return Err(crate::refusal::error(format!(
                "task_drop_acceptance_already: acceptance {index} is already withdrawn"
            )));
        }
        let verified: BTreeSet<_> = task
            .verified
            .iter()
            .flat_map(|evidence| evidence.acceptance.iter().copied())
            .collect();
        if let Some(index) = acceptance.iter().find(|index| verified.contains(index)) {
            return Err(crate::refusal::error(format!(
                "task_drop_acceptance_verified: acceptance {index} already has verification evidence"
            )));
        }
        if already_withdrawn.len() + acceptance.len() == task.acceptance.len() {
            return Err(crate::refusal::error(
                "task_drop_acceptance_all: this would withdraw every acceptance condition; drop the task instead",
            ));
        }
        let at = project::now();
        let reason = reason.trim().to_string();
        task.withdrawn.extend(
            acceptance
                .into_iter()
                .map(|acceptance| AcceptanceWithdrawal {
                    acceptance,
                    at: at.clone(),
                    reason: reason.clone(),
                }),
        );
        task.withdrawn.sort_by_key(|evidence| evidence.acceptance);
        Ok(())
    })
}

pub(crate) fn link_attempt(project: &Project, id: &str, thread: &str) -> Result<Task> {
    crate::thread::load(project, thread)
        .with_context(|| format!("task_attempt: no thread `{thread}`"))?;
    for other in list_with_errors(project).0 {
        if other.id != id && other.attempts.iter().any(|attempt| attempt == thread) {
            return Err(crate::refusal::error(format!(
                "task_attempt: `{thread}` already belongs to `{}`",
                other.id
            )));
        }
    }
    let task = update(project, id, |task| {
        if !task.attempts.iter().any(|attempt| attempt == thread) {
            task.attempts.push(thread.to_string());
        }
        Ok(())
    })?;
    for round in crate::round::list(project)
        .into_iter()
        .filter(|round| round.carries(thread))
    {
        link_round_for_thread(project, &round.round, thread)?;
    }
    load(project, &task.id)
}

/// Attach a historical thread and every round carrying it to a stable task.
/// Unlike an ordinary start, adoption also establishes the task repository
/// when the task did not have one yet.
pub(crate) fn adopt(project: &Project, id: &str, thread_id: &str) -> Result<Task> {
    let thread = crate::thread::load(project, thread_id)
        .with_context(|| format!("task_adopt: no thread `{thread_id}`"))?;
    let _lock = project.lock()?;
    for task in list_with_errors(project).0 {
        if task.attempts.iter().any(|attempt| attempt == thread_id) {
            return Err(crate::refusal::error(format!(
                "task_adopt_linked: `{thread_id}` already belongs to `{}`",
                task.id
            )));
        }
    }
    let mut task = load(project, id)?;
    if let Some(repo) = task.repo.as_deref() {
        let expected = std::fs::canonicalize(repo).unwrap_or_else(|_| PathBuf::from(repo));
        let actual =
            std::fs::canonicalize(&thread.repo).unwrap_or_else(|_| PathBuf::from(&thread.repo));
        if expected != actual {
            return Err(crate::refusal::error(format!(
                "task_adopt_repository: `{thread_id}` works in `{}`, not `{repo}`",
                thread.repo
            )));
        }
    }
    if task.repo.is_none() && !thread.repo.is_empty() {
        task.repo = Some(thread.repo.clone());
    }
    task.attempts.push(thread_id.to_string());
    write(project, &task)?;
    drop(_lock);
    project::refresh_page(project)?;
    for round in crate::round::list(project)
        .into_iter()
        .filter(|round| round.carries(thread_id))
    {
        link_round_for_thread(project, &round.round, thread_id)?;
    }
    load(project, &task.id)
}

pub(crate) fn link_round_for_thread(project: &Project, round: &str, thread: &str) -> Result<()> {
    if let Some(task) = list_with_errors(project)
        .0
        .into_iter()
        .find(|task| task.attempts.iter().any(|attempt| attempt == thread))
    {
        update(project, &task.id, |task| {
            if !task.rounds.iter().any(|candidate| candidate == round) {
                task.rounds.push(round.to_string());
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn validate_states(states: &[String]) -> Result<()> {
    if states.is_empty() || states.first().map(String::as_str) != Some("finished") {
        bail!("task_states: the ordered list must start with `finished`");
    }
    let mut previous = None;
    for state in states {
        let index = STATES
            .iter()
            .position(|candidate| candidate == state)
            .with_context(|| format!("task_states: `{state}` is not a task milestone"))?;
        if previous.is_some_and(|old| index <= old) {
            bail!("task_states: milestones must be unique and in evidence order");
        }
        previous = Some(index);
    }
    Ok(())
}

fn has_commit_changes(project: &Project, task: &Task) -> Result<bool> {
    if task.attempts.is_empty() {
        return Ok(false);
    }
    has_commit_changes_with_events(project, task, &crate::events::list(project))
}

fn has_commit_changes_with_events(
    project: &Project,
    task: &Task,
    events: &[crate::contracts::Event],
) -> Result<bool> {
    if task.attempts.is_empty() {
        return Ok(false);
    }
    for attempt in &task.attempts {
        let thread = crate::thread::load(project, attempt)?;
        let done: Vec<_> = events
            .iter()
            .filter(|event| event.thread == *attempt)
            .filter_map(|event| event.payload.done.as_ref())
            .collect();
        // An unfinished attempt may still have commits which are not sealed in
        // an event yet. Only completed attempts can prove that they stayed at
        // their recorded base.
        if done.is_empty() || done.iter().any(|evidence| evidence.sha != thread.base) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn required_states(project: &Project, task: &Task) -> Result<Vec<String>> {
    if task.repo.is_none() {
        return Ok(vec!["finished".into(), "verified".into()]);
    }
    required_states_with_events(project, task, &crate::events::list(project))
}

pub(crate) fn required_states_with_evidence(
    project: &Project,
    task: &Task,
    evidence: &EvidenceSnapshot,
) -> Result<Vec<String>> {
    required_states_with_events(project, task, &evidence.events)
}

fn required_states_with_events(
    project: &Project,
    task: &Task,
    events: &[crate::contracts::Event],
) -> Result<Vec<String>> {
    if task.repo.is_none() {
        return Ok(vec!["finished".into(), "verified".into()]);
    }
    let settings = project.read_project_md()?.0;
    let mut states = task
        .repo
        .as_deref()
        .and_then(|repo| settings.repos.iter().find(|row| row.path == repo))
        .filter(|row| !row.task_states.is_empty())
        .map(|row| row.task_states.clone())
        .unwrap_or(settings.task_states);
    validate_states(&states)?;
    if !has_commit_changes_with_events(project, task, events)? {
        states.retain(|state| !matches!(state.as_str(), "reviewed" | "merged" | "installed"));
    }
    Ok(states)
}

fn running_current(task: &Task) -> bool {
    let latest_install = task.installed.iter().map(|evidence| &evidence.at).max();
    let latest_running = task.running.iter().map(|evidence| &evidence.at).max();
    latest_install
        .is_none_or(|installed| latest_running.is_some_and(|running| running >= installed))
}

fn withdrawn_acceptance(task: &Task) -> BTreeSet<usize> {
    task.withdrawn
        .iter()
        .map(|evidence| evidence.acceptance)
        .collect()
}

fn live_unverified_count(task: &Task) -> usize {
    let withdrawn = withdrawn_acceptance(task);
    let verified: BTreeSet<_> = task
        .verified
        .iter()
        .flat_map(|evidence| evidence.acceptance.iter().copied())
        .collect();
    (1..=task.acceptance.len())
        .filter(|index| !withdrawn.contains(index) && !verified.contains(index))
        .count()
}

fn next_for(state: State, required: &[String], task: &Task) -> String {
    match state {
        State::Open => "start an attempt".into(),
        State::Working => "finish the current attempt".into(),
        State::Failed => "retry or cancel the current attempt".into(),
        State::Cancelled => "none".into(),
        State::Dropped => String::new(),
        State::Unknown => "repair the unreadable evidence".into(),
        _ => {
            let at = required
                .iter()
                .position(|candidate| *candidate == state.word());
            match at
                .and_then(|index| required.get(index + 1))
                .map(String::as_str)
            {
                Some("reviewed") => "put the attempt in a review round".into(),
                Some("merged") => "merge its reviewed round".into(),
                Some("installed") => "record installation evidence".into(),
                Some("verified") if !running_current(task) => {
                    "check the running harness processes".into()
                }
                Some("verified") => format!(
                    "verify {} acceptance condition(s)",
                    live_unverified_count(task)
                ),
                Some(other) => format!("record {other} evidence"),
                None => "none".into(),
            }
        }
    }
}

pub(crate) struct EvidenceSnapshot {
    events: Vec<crate::contracts::Event>,
    readable: bool,
}

impl EvidenceSnapshot {
    pub(crate) fn load(project: &Project) -> Self {
        let (events, readable) = crate::events::list_checked(project);
        Self { events, readable }
    }

    pub(crate) fn events(&self) -> &[crate::contracts::Event] {
        &self.events
    }
}

pub(crate) fn view(project: &Project, task: Task) -> View {
    view_with_evidence(project, task, &EvidenceSnapshot::load(project))
}

pub(crate) fn view_with_evidence(
    project: &Project,
    task: Task,
    evidence: &EvidenceSnapshot,
) -> View {
    if !task.dropped.is_empty() {
        return View {
            record: task,
            state: State::Dropped,
            next: String::new(),
            failure_class: None,
            provider_kind: None,
        };
    }
    let required = match required_states_with_events(project, &task, &evidence.events) {
        Ok(required) => required,
        Err(_) => {
            return View {
                record: task,
                state: State::Unknown,
                next: "repair the task state configuration".into(),
                failure_class: None,
                provider_kind: None,
            };
        }
    };
    if task.attempts.is_empty() {
        let verified: BTreeSet<usize> = task
            .verified
            .iter()
            .flat_map(|evidence| evidence.acceptance.iter().copied())
            .collect();
        let withdrawn = withdrawn_acceptance(&task);
        let all_verified = (1..=task.acceptance.len())
            .filter(|index| !withdrawn.contains(index))
            .all(|index| verified.contains(&index));
        let state = if required.iter().any(|state| state == "verified") && all_verified {
            State::Verified
        } else {
            State::Open
        };
        let next = if state == State::Open && required.iter().any(|state| state == "verified") {
            format!(
                "verify {} acceptance condition(s)",
                live_unverified_count(&task)
            )
        } else {
            next_for(state, &required, &task)
        };
        return View {
            record: task,
            state,
            next,
            failure_class: None,
            provider_kind: None,
        };
    }
    let current = task.attempts.last().expect("attempt checked");
    let thread = match crate::thread::load(project, current) {
        Ok(thread) => thread,
        Err(_) => {
            return View {
                record: task,
                state: State::Unknown,
                next: "repair the missing attempt record".into(),
                failure_class: None,
                provider_kind: None,
            };
        }
    };
    if !evidence.readable {
        return View {
            record: task,
            state: State::Unknown,
            next: "repair the unreadable event evidence".into(),
            failure_class: None,
            provider_kind: None,
        };
    }
    let events = &evidence.events;
    let attempt = thread.attempt.max(1);
    let event = crate::round::latest_event(events, current, attempt);
    let done = events
        .iter()
        .filter(|event| event.thread == *current && event.attempt == attempt)
        .filter(|event| event.payload.done.is_some())
        .max_by(|left, right| (&left.created, &left.id).cmp(&(&right.created, &right.id)));

    // A round's completion pin is later, stronger evidence than the event that
    // originally fed it. In particular, a merged round must not be hidden by
    // an older failed event, and an open round must remain visible as the
    // current place where the attempt is waiting.
    let mut relevant = Vec::new();
    for id in &task.rounds {
        let round = match crate::round::load(project, id) {
            Ok(round) => round,
            Err(_) => {
                return View {
                    record: task,
                    state: State::Unknown,
                    next: "repair the unreadable round evidence".into(),
                    failure_class: None,
                    provider_kind: None,
                };
            }
        };
        if round
            .manifest
            .members
            .iter()
            .any(|member| member.thread == *current)
        {
            relevant.push(round);
        }
    }
    let pins_current_attempt = |round: &&crate::contracts::RoundRecord| {
        round.phase != RoundPhase::Abandoned
            && round.manifest.members.iter().any(|member| {
                member.thread == *current
                    && member
                        .pin
                        .as_ref()
                        .is_some_and(|pin| pin.attempt == attempt)
            })
    };
    let pinned_round = relevant.iter().rev().find(pins_current_attempt);
    let round_proves_completion = pinned_round.is_some();

    if thread.status == crate::thread::Status::Resolved && !thread.cancellation_reason.is_empty() {
        return View {
            record: task,
            state: State::Cancelled,
            next: "none".into(),
            failure_class: None,
            provider_kind: None,
        };
    }
    if thread.status == crate::thread::Status::Resolved
        && done.is_none()
        && !round_proves_completion
    {
        return View {
            record: task,
            state: State::Unknown,
            next: "the lane ended without `done`, so retry it or attest its stored report".into(),
            failure_class: None,
            provider_kind: None,
        };
    }
    if !round_proves_completion
        && let Some(failed) = event.and_then(|event| event.payload.failed.as_ref())
    {
        return View {
            record: task,
            state: State::Failed,
            next: "retry or cancel the current attempt".into(),
            failure_class: Some(failed.class),
            provider_kind: failed.provider_kind.clone(),
        };
    }
    if !round_proves_completion
        && let Some(waiting_event) = event.filter(|event| event.payload.waiting.is_some())
        && thread.answered_waiting_event != waiting_event.id
    {
        let waiting = waiting_event
            .payload
            .waiting
            .as_ref()
            .expect("waiting event checked");
        return View {
            record: task,
            state: State::Working,
            next: format!("wait for Rolf: {}", waiting.text.trim()),
            failure_class: None,
            provider_kind: None,
        };
    }
    if !round_proves_completion && thread.status == crate::thread::Status::Failed {
        return View {
            record: task,
            state: State::Failed,
            next: "retry or cancel the current attempt".into(),
            failure_class: Some(thread.failure_class),
            provider_kind: thread.provider_failure_kind.clone(),
        };
    }
    if done.is_none() && !round_proves_completion {
        return View {
            record: task,
            state: State::Working,
            next: "finish the current attempt".into(),
            failure_class: None,
            provider_kind: None,
        };
    }

    if relevant.iter().filter(pins_current_attempt).any(|round| {
        round.verdict.is_some()
            && round.phase == RoundPhase::VerdictIn
            && round.verdict_kind.is_none()
    }) {
        return View {
            record: task,
            state: State::Unknown,
            next: "read the review verdict evidence".into(),
            failure_class: None,
            provider_kind: None,
        };
    }
    let reviewed = relevant.iter().filter(pins_current_attempt).any(|round| {
        matches!(
            round.verdict_kind.as_deref(),
            Some("MERGE" | "MERGE-AFTER-DECISION")
        ) || matches!(
            round.phase,
            RoundPhase::Merging | RoundPhase::Checkpointing | RoundPhase::Merged
        )
    });
    let merged = relevant
        .iter()
        .filter(pins_current_attempt)
        .any(|round| round.phase == RoundPhase::Merged);
    let verified: BTreeSet<usize> = task
        .verified
        .iter()
        .flat_map(|evidence| evidence.acceptance.iter().copied())
        .collect();
    let withdrawn = withdrawn_acceptance(&task);
    let all_verified = (1..=task.acceptance.len())
        .filter(|index| !withdrawn.contains(index))
        .all(|index| verified.contains(&index));
    let facts = |state: &str| match state {
        "finished" => true,
        "reviewed" => reviewed,
        "merged" => merged,
        "installed" => !task.installed.is_empty(),
        "verified" => {
            all_verified
                && (!required.iter().any(|state| state == "installed") || !task.running.is_empty())
        }
        _ => false,
    };
    let mut state = State::Finished;
    for milestone in &required {
        if !facts(milestone) {
            break;
        }
        state = match milestone.as_str() {
            "finished" => State::Finished,
            "reviewed" => State::Reviewed,
            "merged" => State::Merged,
            "installed" => State::Installed,
            "verified" => State::Verified,
            _ => State::Unknown,
        };
    }
    let active_round = pinned_round.filter(|round| !round.phase.closed());
    let next = match active_round {
        Some(round)
            if round.phase == RoundPhase::VerdictIn
                && round.verdict_kind.as_deref() == Some("MERGE-AFTER-DECISION") =>
        {
            format!(
                "wait for Rolf: round {} has a MERGE-AFTER-DECISION verdict",
                round.round
            )
        }
        Some(round) if round.phase == RoundPhase::VerdictIn => format!(
            "round {} has a {} verdict; decide its outcome",
            round.round,
            round.verdict_kind.as_deref().unwrap_or("recorded")
        ),
        Some(round) => match round.phase {
            RoundPhase::Admitting => {
                format!("round {} is waiting for completed attempts", round.round)
            }
            RoundPhase::PreparingReview => format!("round {} is preparing review", round.round),
            RoundPhase::UnderReview => {
                format!("round {} is waiting for its review verdict", round.round)
            }
            RoundPhase::Merging => format!("round {} is merging", round.round),
            RoundPhase::Checkpointing => {
                format!("round {} is waiting for its merge checkpoint", round.round)
            }
            RoundPhase::Diverged => format!("repair diverged round {}", round.round),
            RoundPhase::VerdictIn | RoundPhase::Merged | RoundPhase::Abandoned => {
                unreachable!("handled verdict or closed phase")
            }
        },
        None => next_for(state, &required, &task),
    };
    View {
        record: task,
        state,
        next,
        failure_class: None,
        provider_kind: None,
    }
}

pub(crate) fn attestation(project: &Project, task: &Task) -> Option<crate::contracts::Attestation> {
    let current = task.attempts.last()?;
    let thread = crate::thread::load(project, current).ok()?;
    crate::events::list(project)
        .into_iter()
        .filter(|event| event.thread == *current && event.attempt == thread.attempt.max(1))
        .filter_map(|event| {
            let attestation = event.payload.done?.attestation?;
            Some((event.created, event.id, attestation))
        })
        .max_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)))
        .map(|(_, _, attestation)| attestation)
}

pub(crate) fn views(project: &Project) -> (Vec<View>, Vec<anyhow::Error>) {
    views_with_evidence(project, &EvidenceSnapshot::load(project))
}

pub(crate) fn views_with_evidence(
    project: &Project,
    evidence: &EvidenceSnapshot,
) -> (Vec<View>, Vec<anyhow::Error>) {
    let (tasks, errors) = list_with_errors(project);
    (
        tasks
            .into_iter()
            .map(|task| view_with_evidence(project, task, evidence))
            .collect(),
        errors,
    )
}

pub(crate) fn record_evidence(
    project: &Project,
    id: &str,
    command: &str,
    acceptance: Vec<usize>,
) -> Result<Task> {
    if command.trim().is_empty() {
        return Err(crate::refusal::error(
            "task_evidence: --command is required",
        ));
    }
    let before = view(project, load(project, id)?);
    let required = required_states(project, &before.record)?;
    let word = "verified";
    if !required.iter().any(|state| state == word) {
        return Err(crate::refusal::error(format!(
            "task_evidence: `{word}` is not enabled for this task's repository"
        )));
    }
    let prerequisite = required
        .iter()
        .position(|state| state == word)
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| required.get(index))
        .map(String::as_str)
        .unwrap_or("finished");
    let reached = STATES
        .iter()
        .position(|state| *state == before.state.word());
    let needed = STATES
        .iter()
        .position(|state| *state == prerequisite)
        .unwrap_or(usize::MAX);
    let no_attempt_needed = before.record.attempts.is_empty()
        && !has_commit_changes(project, &before.record)?
        && prerequisite == "finished";
    if !reached.is_some_and(|reached| reached >= needed) && !no_attempt_needed {
        return Err(crate::refusal::error(format!(
            "task_evidence: task is {}, but `{word}` needs it to be {prerequisite}",
            before.state.word()
        )));
    }
    if required.iter().any(|state| state == "installed") && !running_current(&before.record) {
        return Err(crate::refusal::error(
            "task_evidence: verification needs a successful running-processes check after installation",
        ));
    }
    if acceptance.is_empty() {
        return Err(crate::refusal::error(
            "task_evidence: verification needs at least one --acceptance number",
        ));
    }
    if acceptance
        .iter()
        .any(|index| *index == 0 || *index > before.record.acceptance.len())
    {
        return Err(crate::refusal::error(format!(
            "task_evidence: an acceptance number is outside 1..={}",
            before.record.acceptance.len()
        )));
    }
    let mut acceptance = acceptance;
    acceptance.sort_unstable();
    acceptance.dedup();
    update(project, id, |task| {
        let withdrawn = withdrawn_acceptance(task);
        if let Some(index) = acceptance.iter().find(|index| withdrawn.contains(index)) {
            return Err(crate::refusal::error(format!(
                "task_evidence_withdrawn: acceptance {index} is withdrawn"
            )));
        }
        let evidence = Evidence {
            at: project::now(),
            command: command.trim().to_string(),
            acceptance,
            machine: None,
            build: None,
        };
        task.verified.push(evidence);
        Ok(())
    })
}

/// Record one machine's installed build without asking the coordinator to
/// translate a successful install back into task state.
#[cfg(test)]
pub(crate) fn record_installed(
    project: &Project,
    id: &str,
    machine: &str,
    build: &str,
) -> Result<Task> {
    let task = record_installed_deferred(project, id, machine, build)?;
    project::refresh_page(project)?;
    Ok(task)
}

pub(crate) fn record_installed_deferred(
    project: &Project,
    id: &str,
    machine: &str,
    build: &str,
) -> Result<Task> {
    update_deferred(project, id, |task| {
        task.installed.push(Evidence {
            at: project::now(),
            command: "ha harness install".into(),
            acceptance: Vec::new(),
            machine: Some(machine.to_string()),
            build: Some(build.to_string()),
        });
        Ok(())
    })
}

#[cfg(test)]
pub(crate) fn record_running(
    project: &Project,
    id: &str,
    machines: Vec<String>,
    processes: Vec<String>,
) -> Result<Task> {
    let task = record_running_deferred(project, id, machines, processes)?;
    project::refresh_page(project)?;
    Ok(task)
}

pub(crate) fn record_running_deferred(
    project: &Project,
    id: &str,
    machines: Vec<String>,
    processes: Vec<String>,
) -> Result<Task> {
    update_deferred(project, id, |task| {
        task.running.push(RunningEvidence {
            at: project::now(),
            command: "ha harness install".into(),
            machines,
            processes,
        });
        Ok(())
    })
}

fn date(timestamp: &str) -> &str {
    &timestamp[..timestamp.len().min(10)]
}

pub(crate) fn withdrawal_summary(task: &Task) -> String {
    if task.withdrawn.is_empty() {
        return String::new();
    }
    let details = task
        .withdrawn
        .iter()
        .map(|evidence| {
            format!(
                "Acceptance {} withdrawn {}: {}",
                evidence.acceptance,
                date(&evidence.at),
                evidence.reason
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(" — {details}")
}

pub(crate) fn render(view: &View) -> String {
    let mut out = format!(
        "{} [{}] {}\n",
        view.record.id,
        view.status(),
        view.record.title
    );
    if let Some(drop) = view.record.dropped.last() {
        out.push_str(&format!("dropped: {}\n", drop.reason));
    }
    if !view.next.is_empty() {
        out.push_str(&format!("next: {}\n", view.next));
    }
    out.push_str(&format!(
        "authority: {}\n",
        view.record.authority.join(", ")
    ));
    if let Some(old) = &view.record.replaces {
        out.push_str(&format!("replaces: {old}\n"));
    }
    out.push_str("acceptance:\n");
    for (index, condition) in view.record.acceptance.iter().enumerate() {
        let number = index + 1;
        if let Some(withdrawal) = view
            .record
            .withdrawn
            .iter()
            .find(|evidence| evidence.acceptance == number)
        {
            out.push_str(&format!(
                "  {number}. [withdrawn {}: {}] {condition}\n",
                date(&withdrawal.at),
                withdrawal.reason
            ));
        } else {
            out.push_str(&format!("  {number}. {condition}\n"));
        }
    }
    if let Some(class) = view.failure_class {
        out.push_str(&format!("failure: {}\n", class.plain()));
    }
    if !view.record.notes.is_empty() {
        out.push_str("notes:\n");
        for (index, note) in view.record.notes.iter().enumerate().rev() {
            let id = if note.id.is_empty() {
                format!("undated:{}:note-{:04}", view.record.id, index + 1)
            } else {
                note.id.clone()
            };
            let provenance = if note.request.is_empty() {
                "undated".to_string()
            } else {
                format!(
                    "{} request:{}",
                    &note.at[..note.at.len().min(10)],
                    note.request
                )
            };
            let replaces = note
                .replaces
                .as_deref()
                .map(|old| format!("; replaces {old}"))
                .unwrap_or_default();
            out.push_str(&format!("  {id} [{provenance}{replaces}] {}\n", note.text));
        }
    }
    for evidence in &view.record.installed {
        if let (Some(machine), Some(build)) = (&evidence.machine, &evidence.build) {
            out.push_str(&format!("installed: {machine} runs {build}\n"));
        }
    }
    if running_current(&view.record) {
        if let Some(evidence) = view.record.running.last() {
            out.push_str(&format!(
                "running processes: checked on {} ({})\n",
                evidence.machines.join(", "),
                evidence.processes.join(", ")
            ));
        }
    } else if !view.record.installed.is_empty() {
        if view.state == State::Verified {
            out.push_str("running processes: latest check unknown; earlier verification remains\n");
        } else {
            out.push_str("running processes: unknown; verification is blocked\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_do_not_overlap_lane_ids() {
        assert!(validate_id("job-0001").is_ok());
        assert!(validate_id("t-0001").is_err());
    }

    #[test]
    fn historical_records_without_running_evidence_still_load() {
        let text = r#"
schema = 1
id = "job-0001"
title = "Historical task"
authority = ["request:q-1"]
acceptance = ["The old record loads."]
attempts = []
rounds = []
installed = []
verified = []
created = "2026-09-21T00:00:00Z"
"#;

        let task: Task = toml::from_str(text).unwrap();

        assert!(task.running.is_empty());
        assert!(task.withdrawn.is_empty());
        validate_record(&task).unwrap();

        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        assert_eq!(view(&project, task).state, State::Open);
    }

    #[test]
    fn acceptance_conditions_keep_long_technical_evidence() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let condition = "README, docs and skill files stay aligned. Each t-0284 check records r109 and src/plain.rs. Each writes a review file in the code folder.";

        crate::talk::append(
            &project,
            None,
            crate::talk::Entry::Rolf {
                request: "q-1".into(),
                text: "Check each sentence on its own.".into(),
                answer: None,
            },
        )
        .unwrap();
        let task = add(
            &project,
            "Check README and SPEC-ADE for t-0284.",
            vec!["request:q-1".into()],
            vec![condition.into()],
            None,
            None,
        )
        .unwrap();

        assert_eq!(task.acceptance, [condition]);
        let long = "The first check passes. This sentence has far too many words for the audience prose check because it keeps exact technical evidence about README, t-0284, r109 and src/plain.rs without losing any detail. The last check passes.";
        let second = add(
            &project,
            "Keep exact evidence in SPEC-ADE.",
            vec!["request:q-1".into()],
            vec![long.into()],
            None,
            None,
        )
        .unwrap();
        assert_eq!(second.acceptance, [long]);
    }

    #[test]
    fn unreadable_records_keep_their_identity_and_are_never_replaced() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        std::fs::write(dir(&project).join("job-0001.toml"), "not toml").unwrap();
        assert_eq!(next_id(&project).unwrap(), "job-0002");

        let mut wrong = record(&project, "job-0002");
        wrong.id = "job-0003".into();
        std::fs::write(
            dir(&project).join("job-0002.toml"),
            toml::to_string(&wrong).unwrap(),
        )
        .unwrap();
        assert!(format!("{:#}", load(&project, "job-0002").unwrap_err()).contains("task_identity"));
    }

    #[test]
    fn a_task_without_a_repository_goes_straight_to_verified() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let task = record(&project, "job-0001");
        assert_eq!(
            required_states(&project, &task).unwrap(),
            ["finished", "verified"]
        );
        let open = view(&project, task);
        assert_eq!(open.state, State::Open);
        assert_eq!(open.next, "verify 1 acceptance condition(s)");

        let checked = record_evidence(&project, "job-0001", "checked output", vec![1]).unwrap();
        let verified = view(&project, checked);
        assert_eq!(verified.state, State::Verified);
        assert!(verified.terminal(&project));
        let page = std::fs::read_to_string(project.project_md()).unwrap();
        assert!(page.contains("`job-0001` [verified] Ship the checked change."));
    }

    #[test]
    fn state_lists_must_follow_evidence_order() {
        assert!(validate_states(&["finished".into(), "merged".into(), "verified".into()]).is_ok());
        assert!(
            validate_states(&["finished".into(), "installed".into(), "merged".into()]).is_err()
        );
    }

    fn record(project: &Project, id: &str) -> Task {
        let task = Task {
            id: id.into(),
            title: "Ship the checked change.".into(),
            authority: vec!["request:q-1".into()],
            acceptance: vec!["The command reports the new result.".into()],
            created: project::now(),
            ..Task::default()
        };
        std::fs::create_dir_all(dir(project)).unwrap();
        write(project, &task).unwrap();
        task
    }

    #[test]
    fn add_then_drop_renders_the_reason_in_every_task_view() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        crate::talk::append(
            &project,
            None,
            crate::talk::Entry::Rolf {
                request: "q-1".into(),
                text: "Remove the task if its premise is wrong.".into(),
                answer: None,
            },
        )
        .unwrap();
        let task = add(
            &project,
            "Ship the checked change.",
            vec!["request:q-1".into()],
            vec!["The command reports the new result.".into()],
            None,
            None,
        )
        .unwrap();

        let dropped = drop_task(&project, &task.id, "The premise was wrong.").unwrap();
        assert_eq!(dropped.dropped.len(), 1);
        assert!(!dropped.dropped[0].at.is_empty());
        let view = view(&project, dropped);
        assert_eq!(view.state, State::Dropped);
        assert!(view.next.is_empty());
        assert!(view.terminal(&project));
        assert!(render(&view).contains("dropped: The premise was wrong."));
        let listed = views(&project).0.iter().map(render).collect::<String>();
        assert!(listed.contains("dropped: The premise was wrong."));
        let page = std::fs::read_to_string(project.project_md()).unwrap();
        assert!(page.contains("`job-0001` [dropped] Ship the checked change."));
        let context = crate::coordinator::digest(&world.ctx(), &project, "ha")
            .unwrap()
            .0;
        assert!(context.contains("dropped: The premise was wrong."));
    }

    #[test]
    fn withdrawing_the_only_unverified_condition_completes_the_task_everywhere() {
        use crate::round::testkit::fixture;

        let fx = fixture();
        let mut task = record(&fx.project, "job-0001");
        task.acceptance = vec![
            "The first result is checked.".into(),
            "The replaced result is no longer required.".into(),
            "The final result is checked.".into(),
        ];
        task.verified.push(Evidence {
            at: project::now(),
            command: "checked results one and three".into(),
            acceptance: vec![1, 3],
            machine: None,
            build: None,
        });
        write(&fx.project, &task).unwrap();
        let (lane, sha) = fx.lane(1);
        link_attempt(&fx.project, "job-0001", &lane).unwrap();
        fx.seal_done(&lane, 1, 1, &sha, "# report\n");

        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.task_states = vec!["finished".into(), "verified".into()];
        let front = toml::to_string(&settings).unwrap();
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{front}+++\n\n{body}"),
        )
        .unwrap();

        let task = withdraw_acceptance(
            &fx.project,
            "job-0001",
            vec![2, 2],
            "The newer cleanup choice replaced it.",
        )
        .unwrap();
        let withdrawn_date = date(&task.withdrawn[0].at).to_string();
        let view = view(&fx.project, task);
        assert_eq!(view.state, State::Verified);
        assert!(view.terminal(&fx.project));
        assert_eq!(view.next, "none");
        assert!(render(&view).contains(&format!(
            "2. [withdrawn {withdrawn_date}: The newer cleanup choice replaced it.]"
        )));

        let summary = format!(
            "Acceptance 2 withdrawn {withdrawn_date}: The newer cleanup choice replaced it."
        );
        let overview = crate::talk::overview::Overview::load(
            &fx.project,
            &crate::talk::Journal::default(),
            &crate::talk::view::Conversation::default(),
            &crate::talk::overview::Live::default(),
        );
        assert!(
            overview
                .tasks
                .iter()
                .any(|row| row.full_text().contains(&summary)),
            "{:?}",
            overview
                .tasks
                .iter()
                .map(|row| row.full_text())
                .collect::<Vec<_>>()
        );

        let error = record_evidence(
            &fx.project,
            "job-0001",
            "checked the replaced result",
            vec![2],
        )
        .unwrap_err();
        assert!(error.to_string().contains("task_evidence_withdrawn"));

        let mut contradictory = load(&fx.project, "job-0001").unwrap();
        contradictory.verified.push(Evidence {
            at: project::now(),
            command: "contradictory evidence".into(),
            acceptance: vec![2],
            machine: None,
            build: None,
        });
        assert!(
            validate_record(&contradictory)
                .unwrap_err()
                .to_string()
                .contains("also has verification evidence")
        );
    }

    #[test]
    fn acceptance_withdrawal_refuses_each_invalid_target() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let mut task = record(&project, "job-0001");
        task.acceptance = vec!["First.".into(), "Second.".into(), "Third.".into()];
        task.verified.push(Evidence {
            at: project::now(),
            command: "checked first".into(),
            acceptance: vec![1],
            machine: None,
            build: None,
        });
        write(&project, &task).unwrap();

        let no_reason = withdraw_acceptance(&project, "job-0001", vec![2], "").unwrap_err();
        assert!(
            no_reason
                .to_string()
                .contains("task_drop: --reason is required")
        );
        let verified = withdraw_acceptance(&project, "job-0001", vec![1], "Replaced.").unwrap_err();
        assert!(
            verified
                .to_string()
                .contains("task_drop_acceptance_verified")
        );
        let range = withdraw_acceptance(&project, "job-0001", vec![4], "Replaced.").unwrap_err();
        assert!(range.to_string().contains("task_drop_acceptance_range"));
        withdraw_acceptance(&project, "job-0001", vec![2], "Replaced.").unwrap();
        let partly_withdrawn = load(&project, "job-0001").unwrap();
        assert_eq!(live_unverified_count(&partly_withdrawn), 1);
        assert_eq!(
            next_for(
                State::Merged,
                &["finished".into(), "merged".into(), "verified".into()],
                &partly_withdrawn
            ),
            "verify 1 acceptance condition(s)"
        );
        let already =
            withdraw_acceptance(&project, "job-0001", vec![2], "Replaced again.").unwrap_err();
        assert!(already.to_string().contains("task_drop_acceptance_already"));

        record(&project, "job-0002");
        let all = withdraw_acceptance(&project, "job-0002", vec![1], "Replaced.").unwrap_err();
        assert!(all.to_string().contains("task_drop_acceptance_all"));
        assert!(load(&project, "job-0002").unwrap().withdrawn.is_empty());
    }

    #[test]
    fn dropping_a_task_with_verification_evidence_is_refused() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let mut task = record(&project, "job-0001");
        task.verified.push(Evidence {
            at: project::now(),
            command: "ha doctor".into(),
            acceptance: vec![1],
            machine: None,
            build: None,
        });
        write(&project, &task).unwrap();

        let error = drop_task(&project, "job-0001", "The premise was wrong.").unwrap_err();
        assert!(
            error.to_string().contains("task_drop_verified"),
            "{error:#}"
        );
        assert!(load(&project, "job-0001").unwrap().dropped.is_empty());
    }

    #[test]
    fn a_missing_request_is_refused() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let error = add(
            &project,
            "Ship the checked change.",
            vec!["request:q-missing".into()],
            vec!["The command reports the new result.".into()],
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "request_authority: no request `q-missing` in project `demo`"
        );
        assert!(list_with_errors(&project).0.is_empty());
    }

    #[test]
    fn tasks_without_commits_skip_code_milestones_but_changed_tasks_keep_them() {
        use crate::round::testkit::{fixture, git};

        let fx = fixture();
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.task_states = STATES.iter().map(|state| state.to_string()).collect();
        let front = toml::to_string(&settings).unwrap();
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{front}+++\n\n{body}"),
        )
        .unwrap();

        let no_attempt = record(&fx.project, "job-0001");
        let open = view(&fx.project, no_attempt);
        assert_eq!(open.state, State::Open);
        assert_eq!(open.next, "verify 1 acceptance condition(s)");
        let verified =
            record_evidence(&fx.project, "job-0001", "checked published result", vec![1]).unwrap();
        assert_eq!(view(&fx.project, verified).state, State::Verified);

        let base = git(&fx.repo, &["rev-parse", "main"]);
        record(&fx.project, "job-0002");
        update(&fx.project, "job-0002", |task| {
            task.repo = Some(fx.repo.to_string_lossy().into_owned());
            Ok(())
        })
        .unwrap();
        let (unchanged_lane, _) = fx.lane(1);
        let unchanged_thread = crate::thread::load(&fx.project, &unchanged_lane).unwrap();
        git(
            std::path::Path::new(&unchanged_thread.worktree_path),
            &["reset", "--hard", &base],
        );
        crate::thread::update(&fx.project, &unchanged_lane, |thread| {
            thread.base = base.clone();
        })
        .unwrap();
        link_attempt(&fx.project, "job-0002", &unchanged_lane).unwrap();
        fx.seal_done(&unchanged_lane, 1, 1, &base, "# review\n");
        let unchanged = load(&fx.project, "job-0002").unwrap();
        assert_eq!(
            required_states(&fx.project, &unchanged).unwrap(),
            ["finished", "verified"]
        );
        assert_eq!(view(&fx.project, unchanged).state, State::Finished);
        let verified =
            record_evidence(&fx.project, "job-0002", "checked report result", vec![1]).unwrap();
        assert_eq!(view(&fx.project, verified).state, State::Verified);

        record(&fx.project, "job-0003");
        update(&fx.project, "job-0003", |task| {
            task.repo = Some(fx.repo.to_string_lossy().into_owned());
            Ok(())
        })
        .unwrap();
        let (changed_lane, changed_sha) = fx.lane(2);
        crate::thread::update(&fx.project, &changed_lane, |thread| {
            thread.base = base.clone();
        })
        .unwrap();
        link_attempt(&fx.project, "job-0003", &changed_lane).unwrap();
        fx.seal_done(&changed_lane, 1, 1, &changed_sha, "# code\n");
        let changed = load(&fx.project, "job-0003").unwrap();
        assert_eq!(required_states(&fx.project, &changed).unwrap(), STATES);
        assert_eq!(view(&fx.project, changed).state, State::Finished);
        let error = record_evidence(&fx.project, "job-0003", "checked code", vec![1]).unwrap_err();
        assert!(error.to_string().contains("needs it to be installed"));
    }

    #[test]
    fn one_record_drives_every_milestone_and_skips_install_when_not_configured() {
        use crate::round::testkit::{fixture, git};
        let fx = fixture();
        let ctx = fx.world.ctx();
        crate::plan::set(&ctx, "demo", "command", "It reports the checked result.", 0).unwrap();
        crate::plan::step_add(&ctx, "demo", "Ship the checked change.", vec![], 1).unwrap();
        record(&fx.project, "job-0001");
        let task = update(&fx.project, "job-0001", |task| {
            task.repo = Some(fx.repo.to_string_lossy().into_owned());
            task.plan_step = Some("s-1".into());
            Ok(())
        })
        .unwrap();
        assert_eq!(view(&fx.project, task).state, State::Open);

        let (lane, sha) = fx.lane(1);
        link_attempt(&fx.project, "job-0001", &lane).unwrap();
        assert_eq!(
            view(&fx.project, load(&fx.project, "job-0001").unwrap()).state,
            State::Working
        );
        assert!(
            crate::plan::show(&ctx, "demo", false)
                .unwrap()
                .contains("running s-1")
        );
        let digest = crate::coordinator::digest(&ctx, &fx.project, "ha")
            .unwrap()
            .0;
        assert!(digest.contains("`job-0001` [working]"));
        fx.seal_done(&lane, 1, 1, &sha, "# report\n");
        assert_eq!(
            view(&fx.project, load(&fx.project, "job-0001").unwrap()).state,
            State::Finished
        );

        crate::round::open(
            &ctx,
            "demo",
            crate::round::OpenArgs {
                round: "r1".into(),
                branch: "main".into(),
                plain: Some("The checked change is ready.".into()),
                repo: Some(fx.repo.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        crate::round::admit(&ctx, "demo", "r1", &lane).unwrap();
        let review = crate::round::review(&ctx, "demo", "r1").unwrap();
        git(&review.worktree, &["merge", "-q", "--no-edit", &sha]);
        let candidate = git(&review.worktree, &["rev-parse", "HEAD"]);
        let round = crate::round::load(&fx.project, "r1").unwrap();
        let verdict_text = format!(
            "+++\nverdict = \"MERGE\"\nround = \"r1\"\ncandidate = \"{candidate}\"\nmanifest_hash = \"{}\"\npolicy_hash = \"{}\"\ngates = []\n+++\n\nAll gates pass.\n",
            round.manifest_hash.unwrap(),
            round.policy_hash
        );
        let reviewer = fx.thread("Reviewer");
        fx.seal_done(&reviewer, 1, 1, &candidate, &verdict_text);
        crate::round::bind_reviewer(&ctx, "demo", "r1", &reviewer).unwrap();
        crate::round::advance(&ctx, "demo").unwrap();
        assert_eq!(
            view(&fx.project, load(&fx.project, "job-0001").unwrap()).state,
            State::Reviewed
        );
        crate::round::merge(&ctx, "demo", "r1", None).unwrap();
        let merged = view(&fx.project, load(&fx.project, "job-0001").unwrap());
        assert_eq!(merged.state, State::Merged);
        assert_eq!(merged.next, "none");
        // This repository inherits a workflow with no install state.
        assert!(
            !required_states(&fx.project, &merged.record)
                .unwrap()
                .iter()
                .any(|state| state == "installed")
        );

        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.task_states = STATES.iter().map(|state| state.to_string()).collect();
        let front = toml::to_string(&settings).unwrap();
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{front}+++\n\n{body}"),
        )
        .unwrap();
        let installed = record_installed(
            &fx.project,
            "job-0001",
            "local",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        assert_eq!(view(&fx.project, installed).state, State::Installed);
        record_running(
            &fx.project,
            "job-0001",
            vec!["local".into()],
            vec!["local:ticker:running".into()],
        )
        .unwrap();
        let verified = record_evidence(&fx.project, "job-0001", "ha doctor", vec![1]).unwrap();
        assert_eq!(view(&fx.project, verified).state, State::Verified);
        assert!(
            crate::plan::show(&ctx, "demo", false)
                .unwrap()
                .contains("done    s-1")
        );
        let overview = crate::talk::overview::Overview::load(
            &fx.project,
            &crate::talk::Journal::default(),
            &crate::talk::view::Conversation::default(),
            &crate::talk::overview::Live::default(),
        );
        assert!(
            overview
                .tasks
                .iter()
                .any(|row| row.prefix == "verified" && row.marker == "job-0001")
        );

        project::refresh_page(&fx.project).unwrap();
        let page = std::fs::read_to_string(fx.project.project_md()).unwrap();
        assert!(page.contains("`job-0001` [verified] Ship the checked change."));

        // A later install can finish while its process proof is unavailable.
        // That uncertainty is visible, but it does not erase acceptance
        // evidence that was gated by an earlier successful process check.
        update(&fx.project, "job-0001", |task| {
            task.installed.push(Evidence {
                at: "9999-12-31T23:59:59Z".into(),
                command: "ha harness install".into(),
                acceptance: Vec::new(),
                machine: Some("local".into()),
                build: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
            });
            Ok(())
        })
        .unwrap();
        let still_verified = view(&fx.project, load(&fx.project, "job-0001").unwrap());
        assert_eq!(still_verified.state, State::Verified);
        assert!(still_verified.terminal(&fx.project));
        assert_eq!(still_verified.next, "none");
        assert_eq!(
            still_verified.status(),
            "verified; latest process check unknown"
        );
        assert!(
            render(&still_verified)
                .contains("running processes: latest check unknown; earlier verification remains")
        );
        project::refresh_page(&fx.project).unwrap();
        let page = std::fs::read_to_string(fx.project.project_md()).unwrap();
        assert!(page.contains("`job-0001` [verified] Ship the checked change."));
    }

    #[test]
    fn adopt_links_a_historical_thread_and_its_round_and_refuses_reuse() {
        use crate::round::testkit::fixture;
        let fx = fixture();
        let mut task = record(&fx.project, "job-0001");
        task.repo = Some(fx.repo.to_string_lossy().into_owned());
        write(&fx.project, &task).unwrap();
        let (lane, _sha) = fx.lane(1);
        crate::thread::update(&fx.project, &lane, |thread| {
            thread.repo = fx.repo.to_string_lossy().into_owned();
        })
        .unwrap();
        crate::round::open(
            &fx.world.ctx(),
            "demo",
            crate::round::OpenArgs {
                round: "r1".into(),
                branch: "main".into(),
                plain: Some("The historical change is reviewed.".into()),
                repo: Some(fx.repo.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        crate::round::admit(&fx.world.ctx(), "demo", "r1", &lane).unwrap();

        let adopted = adopt(&fx.project, "job-0001", &lane).unwrap();
        assert_eq!(adopted.attempts.as_slice(), std::slice::from_ref(&lane));
        assert_eq!(adopted.rounds, ["r1"]);

        let mut second = record(&fx.project, "job-0002");
        second.repo = Some(fx.repo.to_string_lossy().into_owned());
        write(&fx.project, &second).unwrap();
        let error = adopt(&fx.project, "job-0002", &lane).unwrap_err();
        assert!(error.to_string().contains("task_adopt_linked"), "{error:#}");
    }

    #[test]
    fn adopt_refuses_a_thread_from_another_repository() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let mut task = record(&project, "job-0001");
        task.repo = Some("/expected/repository".into());
        write(&project, &task).unwrap();
        let thread = crate::thread::allocate(&project, |thread| {
            thread.repo = "/other/repository".into();
        })
        .unwrap();
        let error = adopt(&project, "job-0001", &thread.id).unwrap_err();
        assert!(
            error.to_string().contains("task_adopt_repository"),
            "{error:#}"
        );
    }

    #[test]
    fn resolved_attempt_without_done_is_unknown_and_actionable() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        record(&project, "job-0001");
        let thread = crate::thread::allocate(&project, |thread| {
            thread.status = crate::thread::Status::Resolved;
            thread.attempt = 1;
        })
        .unwrap();
        link_attempt(&project, "job-0001", &thread.id).unwrap();

        let unresolved = view(&project, load(&project, "job-0001").unwrap());
        assert_eq!(unresolved.state, State::Unknown);
        assert_eq!(
            unresolved.next,
            "the lane ended without `done`, so retry it or attest its stored report"
        );
    }

    #[test]
    fn attested_report_finishes_the_task_and_keeps_the_attestation() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        record(&project, "job-0001");
        let report = b"stored final report\n";
        let hash = crate::thread::sha256_hex(report);
        let thread = crate::thread::allocate(&project, |thread| {
            thread.status = crate::thread::Status::Resolved;
            thread.attempt = 1;
            thread.report_hash = hash.clone();
        })
        .unwrap();
        std::fs::write(
            crate::thread::home_report_path(&project, &thread.id),
            report,
        )
        .unwrap();
        link_attempt(&project, "job-0001", &thread.id).unwrap();

        let outcome = crate::threads::attest(
            &world.ctx(),
            "demo",
            &thread.id,
            "The final copy is the completed work.",
        )
        .unwrap();
        assert_eq!(outcome.sha, None);
        assert_eq!(
            std::fs::read(crate::events::artifact_path(&project, &outcome.artifact)).unwrap(),
            report
        );
        let sealed = crate::events::load(&project, &outcome.event).unwrap();
        assert_eq!(sealed.payload.done.unwrap().sha, "");

        let finished = view(&project, load(&project, "job-0001").unwrap());
        assert_eq!(finished.state, State::Finished);
        let attested = attestation(&project, &finished.record).unwrap();
        assert_eq!(attested.coordinator, "hp-demo-coordinator");
        assert_eq!(attested.reason, "The final copy is the completed work.");
    }

    #[test]
    fn waiting_attempt_says_that_its_next_step_is_rolf() {
        use crate::round::testkit::fixture;
        let fx = fixture();
        record(&fx.project, "job-0001");
        let (lane, _) = fx.lane(1);
        link_attempt(&fx.project, "job-0001", &lane).unwrap();
        fx.seal_waiting(&lane, 1, 1, "Choose the final colour.");

        let waiting = view(&fx.project, load(&fx.project, "job-0001").unwrap());
        assert_eq!(waiting.state, State::Working);
        assert_eq!(waiting.next, "wait for Rolf: Choose the final colour.");
    }

    #[test]
    fn delivered_prompt_clears_the_waiting_line() {
        use crate::round::testkit::fixture;
        let fx = fixture();
        record(&fx.project, "job-0001");
        let (lane, _) = fx.lane(1);
        link_attempt(&fx.project, "job-0001", &lane).unwrap();
        fx.seal_waiting(&lane, 1, 1, "Choose the final colour.");

        let events = crate::round::sealed_events(&fx.project).unwrap();
        let answered_id = events[0].id.as_str();
        crate::threads::record_answered_wait(&fx.project, &lane, 1, answered_id).unwrap();

        let answered = view(&fx.project, load(&fx.project, "job-0001").unwrap());
        assert_eq!(answered.state, State::Working);
        assert_eq!(answered.next, "finish the current attempt");

        fx.seal_waiting(&lane, 1, 2, "Choose the final shape.");
        let later = view(&fx.project, load(&fx.project, "job-0001").unwrap());
        assert_eq!(later.next, "wait for Rolf: Choose the final shape.");
    }

    fn task_with_pinned_round_and_newer_failure() -> (crate::round::testkit::Fx, String) {
        use crate::contracts::{Event, EventPayload, Recipient, WaitingPayload};
        let fx = crate::round::testkit::fixture();
        let ctx = fx.world.ctx();
        let mut task = record(&fx.project, "job-0001");
        task.repo = Some(fx.repo.to_string_lossy().into_owned());
        write(&fx.project, &task).unwrap();
        let (lane, sha) = fx.lane(1);
        link_attempt(&fx.project, "job-0001", &lane).unwrap();
        fx.seal_done(&lane, 1, 1, &sha, "# report\n");
        crate::round::open(
            &ctx,
            "demo",
            crate::round::OpenArgs {
                round: "r1".into(),
                branch: "main".into(),
                plain: Some("The checked change is ready.".into()),
                repo: Some(fx.repo.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        crate::round::admit(&ctx, "demo", "r1", &lane).unwrap();
        let failure = Event {
            id: format!("{lane}-1-2"),
            op: format!("{lane}-1-2"),
            thread: lane,
            attempt: 1,
            round: None,
            recipient: Recipient::default(),
            created: "9999-12-31T23:59:59Z".into(),
            payload: EventPayload {
                failed: Some(WaitingPayload {
                    text: "old provider failure".into(),
                    class: FailureClass::Provider,
                    provider_kind: Some("pi".into()),
                }),
                ..EventPayload::default()
            },
        };
        std::fs::write(
            crate::round::events_dir(&fx.project).join(format!("{}.toml", failure.id)),
            toml::to_string(&failure).unwrap(),
        )
        .unwrap();
        (fx, sha)
    }

    #[test]
    fn verdict_in_round_is_the_current_task_action() {
        let (fx, _) = task_with_pinned_round_and_newer_failure();
        let round_path = crate::round::rounds_dir(&fx.project).join("r1.toml");
        let mut round = crate::round::load(&fx.project, "r1").unwrap();
        round.phase = RoundPhase::VerdictIn;
        round.verdict = round.manifest.members[0].pin.clone();
        round.verdict_kind = Some("MERGE-AFTER-DECISION".into());
        std::fs::write(&round_path, toml::to_string(&round).unwrap()).unwrap();

        let held = view(&fx.project, load(&fx.project, "job-0001").unwrap());
        assert_eq!(held.state, State::Reviewed);
        assert_eq!(
            held.next,
            "wait for Rolf: round r1 has a MERGE-AFTER-DECISION verdict"
        );
    }

    #[test]
    fn merged_round_supersedes_an_older_failure() {
        use crate::contracts::{MergeIntent, MergePhase};
        let (fx, sha) = task_with_pinned_round_and_newer_failure();
        let round_path = crate::round::rounds_dir(&fx.project).join("r1.toml");
        let mut round = crate::round::load(&fx.project, "r1").unwrap();
        round.phase = RoundPhase::Merged;
        round.merge = Some(MergeIntent {
            op: "test-merge".into(),
            expected_old: sha.clone(),
            candidate: sha.clone(),
            verdict: sha.clone(),
            phase: MergePhase::Checkpointed,
            merged: Some(sha),
            checkpoint: None,
            head: None,
        });
        std::fs::write(&round_path, toml::to_string(&round).unwrap()).unwrap();

        let merged = view(&fx.project, load(&fx.project, "job-0001").unwrap());
        assert_eq!(merged.state, State::Merged);
        assert_eq!(merged.next, "none");
        assert_eq!(merged.failure_class, None);
    }

    #[test]
    fn provider_failure_keeps_its_class_and_is_not_finished() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        record(&project, "job-0001");
        let thread = crate::thread::allocate(&project, |thread| {
            thread.status = crate::thread::Status::Failed;
            thread.attempt = 1;
            thread.failure_class = FailureClass::Provider;
            thread.provider_failure_kind = Some("pi".into());
        })
        .unwrap();
        link_attempt(&project, "job-0001", &thread.id).unwrap();
        let view = view(&project, load(&project, "job-0001").unwrap());
        assert_eq!(view.state, State::Failed);
        assert_eq!(view.failure_class, Some(FailureClass::Provider));
        assert_eq!(view.provider_kind.as_deref(), Some("pi"));
    }
}
