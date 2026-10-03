//! Stable project tasks and their evidence-derived state.
//!
//! Task records contain intent and links, never a writable status. Every view
//! calls [`view`] so the project page, context and plans use the same
//! projection.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::FailureClass;
use crate::paths::Ctx;
use crate::project::{self, Project, write_atomic};

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
pub(crate) struct AcceptanceReview {
    pub(crate) coordinator: String,
    pub(crate) at: String,
    pub(crate) event: String,
    pub(crate) artifact: String,
    pub(crate) conditions: Vec<String>,
    pub(crate) criteria: Vec<crate::contracts::CriterionEvidence>,
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
    /// An explicit older note, instruction or task this task supersedes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) replaces: Option<String>,
    pub(crate) notes: Vec<DatedNote>,
    #[serde(default)]
    pub(crate) dropped: Vec<DropEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) withdrawn: Vec<AcceptanceWithdrawal>,
    pub(crate) attempts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) plan_step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) repo: Option<String>,
    pub(crate) installed: Vec<Evidence>,
    /// Semantic acceptance is neither a finish seal nor installation proof.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) acceptance_review: Option<AcceptanceReview>,
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
            plan_step: None,
            repo: None,
            installed: Vec::new(),
            acceptance_review: None,
            created: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum State {
    Open,
    Dropped,
    Working,
    Finished,
    Merged,
    Installed,
}
impl State {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Dropped => "dropped",
            Self::Working => "working",
            Self::Finished => "finished",
            Self::Merged => "merged",
            Self::Installed => "installed",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct View {
    pub(crate) record: Task,
    pub(crate) state: State,
    pub(crate) next: String,
    pub(crate) failure_class: Option<FailureClass>,
    pub(crate) provider_kind: Option<String>,
}
impl View {
    pub(crate) fn terminal_with_evidence(
        &self,
        project: &Project,
        evidence: &EvidenceSnapshot,
    ) -> bool {
        if !self.record.dropped.is_empty() || self.state == State::Installed {
            return true;
        }
        self.record
            .attempts
            .last()
            .and_then(|id| crate::thread::load(project, id).ok())
            .is_some_and(|lane| crate::review::lane_done(project, &lane, &evidence.events))
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
            return Err(crate::refusal::error(
                format!("task_unknown: no task `{id}` in `{}`", project.slug),
                format!("ha task list {}", project.slug),
            ));
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
        bail!("task_acceptance: at least one acceptance condition is required");
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
        return Err(crate::refusal::error(
            "task_title: a title is required",
            "ha task add <project> --title \"<title>\" --request <request-id> --acceptance \"<condition>\"",
        ));
    }
    if authority.is_empty() {
        return Err(crate::refusal::error(
            "task_authority: pass at least one --request request:<id> or ask:<id>@<revision>",
            "ha task add <project> --title \"<title>\" --request <request-id> --acceptance \"<condition>\"",
        ));
    }
    if acceptance.is_empty() || acceptance.iter().any(|line| line.trim().is_empty()) {
        return Err(crate::refusal::error(
            "task_acceptance: pass at least one non-empty --acceptance condition",
            "ha task add <project> --title \"<title>\" --request <request-id> --acceptance \"<condition>\"",
        ));
    }
    let title = title.trim().to_string();
    let checked_acceptance = acceptance
        .iter()
        .map(|condition| condition.trim().to_string())
        .collect();
    let authority = authority
        .iter()
        .map(|reference| {
            crate::note::validate_basis(project, reference)
                .map_err(|error| crate::refusal::error(error.to_string(), format!("ha task add {} --title \"{}\" --request <existing-request-id> --acceptance \"<condition>\"", project.slug, title)))
        })
        .collect::<Result<Vec<_>>>()?;
    let _replacement_lock = replaces
        .as_ref()
        .map(|_| crate::note::replacement_lock(project))
        .transpose()?;
    if let Some(old) = replaces.as_deref() {
        if !crate::note::target_exists(project, old) {
            return Err(crate::refusal::error(
                format!("task_replacement: no note or task `{old}` exists"),
                "ha task list <project> (choose a current note or task for --replaces)",
            ));
        }
        let rows = crate::note::rows(project);
        if crate::note::replacement_map(&rows).contains_key(old) {
            return Err(crate::refusal::error(
                format!("task_replacement: `{old}` already has a replacement"),
                "ha task list <project> (choose a current note or task for --replaces)",
            ));
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
        return Err(crate::refusal::error(
            "task_drop: --reason is required",
            "ha task drop <project> <job> --reason \"<reason>\"",
        ));
    }
    update(project, id, |task| {
        if !task.dropped.is_empty() {
            return Err(crate::refusal::error(
                format!("task_drop_already: `{id}` is already dropped"),
                "ha task show <project> <job> (the task is already dropped)",
            ));
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
        return Err(crate::refusal::error(
            "task_drop: --reason is required",
            "ha task drop <project> <job> --reason \"<reason>\"",
        ));
    }
    update(project, id, |task| {
        if !task.dropped.is_empty() {
            return Err(crate::refusal::error(
                format!("task_drop_already: `{id}` is already dropped"),
                "ha task show <project> <job> (the task is already dropped)",
            ));
        }
        let mut acceptance = acceptance;
        acceptance.sort_unstable();
        acceptance.dedup();
        if acceptance
            .iter()
            .any(|index| *index == 0 || *index > task.acceptance.len())
        {
            return Err(crate::refusal::error(
                format!(
                    "task_drop_acceptance_range: an acceptance number is outside 1..={}",
                    task.acceptance.len()
                ),
                "ha task show <project> <job> (choose an acceptance number shown there)",
            ));
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
            return Err(crate::refusal::error(
                format!("task_drop_acceptance_already: acceptance {index} is already withdrawn"),
                "ha task show <project> <job> (choose an acceptance condition not yet withdrawn)",
            ));
        }
        if already_withdrawn.len() + acceptance.len() == task.acceptance.len() {
            return Err(crate::refusal::error(
                "task_drop_acceptance_all: this would withdraw every acceptance condition; drop the task instead",
                "ha task drop <project> <job> --reason \"<reason>\"",
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
            return Err(crate::refusal::error(
                format!("task_attempt: `{thread}` already belongs to `{}`", other.id),
                "ha task show <project> <job> (use the task already linked to this thread)",
            ));
        }
    }
    let task = update(project, id, |task| {
        if !task.attempts.iter().any(|attempt| attempt == thread) {
            task.attempts.push(thread.to_string());
        }
        Ok(())
    })?;
    Ok(task)
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

    pub(crate) fn readable(&self) -> bool {
        self.readable
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
    let mut view = View {
        record: task,
        state: State::Open,
        next: "start an attempt".into(),
        failure_class: None,
        provider_kind: None,
    };
    if !view.record.dropped.is_empty() {
        view.state = State::Dropped;
        view.next = "none".into();
        return view;
    }
    if !view.record.installed.is_empty() {
        view.state = State::Installed;
        view.next = "none".into();
        return view;
    }
    let Some(id) = view.record.attempts.last() else {
        return view;
    };
    let lane = match crate::thread::load(project, id) {
        Ok(lane) => lane,
        Err(_) => {
            view.next = "repair the missing attempt record".into();
            return view;
        }
    };
    if !evidence.readable {
        view.next = "repair the unreadable event evidence".into();
        return view;
    }
    match crate::review::lane_review(project, &lane) {
        Ok(Some(review)) => {
            view.state = if review.install_required && review.install {
                State::Installed
            } else {
                State::Merged
            };
            view.next = if review.install_required && !review.install {
                "finish the pile installation".into()
            } else {
                "none".into()
            };
            return view;
        }
        Err(_) => {
            view.next = "repair the unreadable review record".into();
            return view;
        }
        Ok(None) => {}
    }
    if !lane.merged_sha.is_empty() {
        view.state = if !lane.installed_sha.is_empty() {
            State::Installed
        } else {
            State::Merged
        };
        view.next = if lane.historical_install_required && lane.installed_sha.is_empty() {
            "finish the pile installation".into()
        } else {
            "none".into()
        };
        return view;
    }
    if let Some(seal) = crate::review::sealed(&evidence.events, &lane) {
        view.state = State::Finished;
        view.next = if crate::review::lane_done(project, &lane, &evidence.events) {
            "none".into()
        } else {
            "review the repository pile".into()
        };
        if crate::review::changes(&lane, seal) == Some(false)
            && require_accepted(project, &view.record, evidence).is_err()
        {
            view.next =
                "judge required acceptance through coordinator/critic (not established)".into();
        }
        return view;
    }
    view.state = State::Working;
    view.next = "finish the current attempt".into();
    if !lane.review_reason.is_empty() {
        view.next = format!("follow up: {}", lane.review_reason);
    }
    if let Some(event) =
        crate::events::latest_event(&evidence.events, &lane.id, lane.attempt.max(1))
    {
        if let Some(failed) = &event.payload.failed {
            view.failure_class = Some(failed.class);
            view.provider_kind = failed.provider_kind.clone();
            view.next = "retry or cancel the current attempt".into();
        }
        if let Some(waiting) = &event.payload.waiting
            && lane.answered_waiting_event != event.id
        {
            view.next = format!("answer the lane wait: {}", waiting.text.trim());
        }
    }
    if lane.status == crate::thread::Status::Failed {
        view.failure_class = Some(lane.failure_class);
        view.provider_kind = lane.provider_failure_kind;
        view.next = "retry or cancel the current attempt".into();
    }
    if lane.status == crate::thread::Status::Resolved {
        view.state = State::Open;
        view.next = if lane.cancellation_reason.is_empty() {
            "lane ended without done; retry or attest its stored report".into()
        } else {
            "cancelled".into()
        };
    }
    view
}

/// The common report format for the existing reviewer and optional critic.
/// Producer claims alone are not acceptance; callers must identify the judge.
pub(crate) fn report_criteria(text: &str) -> Result<Vec<crate::contracts::CriterionEvidence>> {
    #[derive(Deserialize)]
    struct Criteria {
        #[serde(default)]
        acceptance: Vec<crate::contracts::CriterionEvidence>,
    }
    let front = text
        .strip_prefix("+++\n")
        .and_then(|text| text.split_once("\n+++").map(|(front, _)| front));
    match front {
        Some(front) => Ok(toml::from_str::<Criteria>(front)?.acceptance),
        None => Ok(Vec::new()),
    }
}

pub(crate) fn criteria_established(
    task: &Task,
    lane: &str,
    event: &str,
    criteria: &[crate::contracts::CriterionEvidence],
) -> bool {
    !task.acceptance.is_empty()
        && task
            .acceptance
            .iter()
            .enumerate()
            .all(|(index, condition)| {
                let criterion = index + 1;
                if task.withdrawn.iter().any(|row| row.acceptance == criterion) {
                    return true;
                }
                let mut rows = criteria.iter().filter(|row| {
                    row.thread == lane && row.event == event && row.criterion == criterion
                });
                let Some(row) = rows.next() else {
                    return false;
                };
                rows.next().is_none()
                    && row.condition == *condition
                    && row.established
                    && !row.evidence.trim().is_empty()
            })
}

pub(crate) fn require_accepted(
    project: &Project,
    task: &Task,
    snapshot: &EvidenceSnapshot,
) -> Result<()> {
    if !snapshot.readable() {
        bail!("acceptance not established: unreadable seal evidence");
    }
    let id = task
        .attempts
        .last()
        .context("acceptance not established: no attempt")?;
    let lane = crate::thread::load(project, id)?;
    // Named historical consumer: pre-pile merged/installed records retain
    // their accepted delivery semantics, even without a retained done seal.
    // No-change equality is not that fact.
    if !lane.merged_sha.is_empty() && lane.merged_review.is_empty() {
        return Ok(());
    }
    let event = crate::review::sealed(snapshot.events(), &lane)
        .context("acceptance not established: no current seal")?;
    let done = event.payload.done.as_ref().expect("done seal");
    crate::thread::artifact(project, &done.artifact)
        .context("acceptance not established: source report missing or corrupt")?;
    let accepted = task.acceptance_review.as_ref().is_some_and(|review| {
        review.event == event.id
            && review.artifact == done.artifact
            && review.conditions == task.acceptance
            && !review.coordinator.is_empty()
            && criteria_established(task, id, &event.id, &review.criteria)
    });
    if accepted {
        return Ok(());
    }
    // A landed pile's independent reviewer judges the member, not the member's
    // own report. Read that exact sealed verdict, never today's draft.
    if let Some(review) = crate::review::lane_review(project, &lane)? {
        let seal = crate::events::load(project, &review.verdict_event)?;
        let judged = seal
            .payload
            .done
            .context("acceptance not established: reviewer seal missing")?;
        let text = String::from_utf8(crate::thread::artifact(project, &judged.artifact)?)?;
        let criteria = report_criteria(&text)?;
        if criteria_established(task, id, &event.id, &criteria) {
            return Ok(());
        }
    }
    // Reuse an existing critic when one was requested. No compulsory second
    // judge, and a critic's PASS without criterion evidence is not enough.
    for critic in crate::thread::list(project)
        .into_iter()
        .filter(|critic| critic.role == "critic" && critic.id != lane.id)
    {
        let Some(seal) = crate::review::sealed(snapshot.events(), &critic) else {
            continue;
        };
        let judged = seal.payload.done.as_ref().expect("critic done");
        let text = String::from_utf8(crate::thread::artifact(project, &judged.artifact)?)?;
        if crate::lane::critic_verdict(&text).as_deref() == Some("PASS")
            && criteria_established(task, id, &event.id, &report_criteria(&text)?)
        {
            return Ok(());
        }
    }
    bail!(
        "acceptance not established: {} needs independent evidence for every required criterion; a finish seal, partial report or installation is not acceptance",
        task.id
    )
}

/// Extend the existing coordinator `thread attest` path to an already-finished
/// no-change deliverable. The reason carries the same criterion rows as a
/// reviewer/critic report, so an explicit partial judgment remains durable.
pub(crate) fn attest_finished(
    ctx: &Ctx,
    project: &Project,
    lane: &crate::thread::Thread,
    reason: &str,
) -> Result<Option<crate::threads::AttestOutcome>> {
    let allocation = project.lock()?;
    let snapshot = EvidenceSnapshot::load(project);
    let Some(event) = crate::review::sealed(snapshot.events(), lane) else {
        return Ok(None);
    };
    let done = event.payload.done.as_ref().expect("done");
    if crate::review::changes(lane, event) != Some(false) {
        return Ok(None);
    }
    let criteria = report_criteria(&format!("+++\n{reason}\n+++"))?;
    let coordinator = project
        .coordinator()
        .filter(|row| !row.pane_id.is_empty())
        .context("acceptance not established: coordinator missing")?;
    let coordinator_name = if coordinator.agent_name.is_empty() {
        coordinator.pane_id
    } else {
        coordinator.agent_name
    };
    let (tasks, errors) = list_with_errors(project);
    if !errors.is_empty() {
        bail!("acceptance not established: unreadable task records");
    }
    let tasks: Vec<_> = tasks
        .into_iter()
        .filter(|task| task.attempts.last() == Some(&lane.id))
        .collect();
    if tasks.is_empty() {
        bail!("acceptance not established: bind the deliverable to a request-backed task first");
    }
    for task in &tasks {
        if task.acceptance.is_empty()
            || (1..=task.acceptance.len()).any(|criterion| {
                !task.withdrawn.iter().any(|row| row.acceptance == criterion)
                    && criteria
                        .iter()
                        .filter(|row| {
                            row.thread == lane.id
                                && row.event == event.id
                                && row.criterion == criterion
                        })
                        .count()
                        != 1
            })
        {
            bail!(
                "acceptance not established: --reason needs one [[acceptance]] row for every required criterion, with thread, event, criterion, condition, established and evidence"
            );
        }
    }
    for mut task in tasks {
        task.acceptance_review = Some(AcceptanceReview {
            coordinator: coordinator_name.clone(),
            at: project::now(),
            event: event.id.clone(),
            artifact: done.artifact.clone(),
            conditions: task.acceptance.clone(),
            criteria: criteria.clone(),
        });
        write(project, &task)?;
    }
    drop(allocation);
    crate::plan::refresh(ctx, project)?;
    Ok(Some(crate::threads::AttestOutcome {
        thread: lane.id.clone(),
        event: event.id.clone(),
        artifact: done.artifact.clone(),
        sha: Some(done.sha.clone()),
        coordinator: coordinator_name,
        reason: reason.into(),
    }))
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

pub(crate) fn render(_project: &Project, view: &View) -> String {
    let task = &view.record;
    let mut text = format!(
        "{} [{}] {} — next: {}\n",
        task.id,
        view.state.word(),
        task.title,
        view.next
    );
    for (index, condition) in task.acceptance.iter().enumerate() {
        text.push_str(&format!("  {}. {}\n", index + 1, condition));
    }
    if !task.authority.is_empty() {
        text.push_str(&format!("  requests: {}\n", task.authority.join(", ")));
    }
    if !task.attempts.is_empty() {
        text.push_str(&format!("  attempts: {}\n", task.attempts.join(", ")));
    }
    if let Some(review) = &task.acceptance_review {
        text.push_str(&format!(
            "  acceptance judgment snapshot: {} at {}; seal {}, report {}\n",
            review.coordinator, review.at, review.event, review.artifact
        ));
        for row in &review.criteria {
            text.push_str(&format!(
                "    criterion {}: {} — {}\n",
                row.criterion,
                if row.established && !row.evidence.trim().is_empty() {
                    "evidenced"
                } else {
                    "not established"
                },
                row.evidence
            ));
        }
    }
    for note in &task.notes {
        text.push_str(&format!("  note {}: {}\n", note.at, note.text));
    }
    for drop in &task.dropped {
        text.push_str(&format!("  dropped: {}\n", drop.reason));
    }
    for drop in &task.withdrawn {
        text.push_str(&format!(
            "  withdrawn {}: {}\n",
            drop.acceptance, drop.reason
        ));
    }
    text
}
