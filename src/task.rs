//! Stable project tasks and their evidence-derived state.
//!
//! Task records contain intent and links, never a writable status. Every view
//! calls [`view`] so the project page, context and plans use the same
//! projection.

#[cfg(test)]
#[path = "task/incident_tests.rs"]
mod incident_tests;

#[cfg(test)]
#[path = "task/drop_tests.rs"]
mod drop_tests;

use std::collections::{BTreeMap, BTreeSet};
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
    /// Explicitly confirmed causes; never populated by retry or sealing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) incidents: Vec<Incident>,
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
            incidents: Vec::new(),
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
    pub(crate) repairs: Vec<RepairView>,
}
impl View {
    pub(crate) fn terminal_with_evidence(
        &self,
        _project: &Project,
        evidence: &EvidenceSnapshot,
    ) -> bool {
        !self.record.dropped.is_empty()
            || self.state == State::Installed
            || self
                .record
                .attempts
                .last()
                .and_then(|id| evidence.lanes.get(id))
                .is_some_and(|lane| evidence.lane_done(lane))
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

#[derive(Serialize)]
pub(crate) struct DropOutcome {
    pub(crate) task: Task,
    pub(crate) lanes: Vec<crate::threads::ResolveOutcome>,
}

/// A retained terminal seal is not a landing, nor authority to delete unique work.
pub(crate) fn sealed_unlanded(
    lane: &crate::thread::Thread,
    events: &[crate::contracts::Event],
    reviews: &[crate::review::Review],
) -> bool {
    lane.status != crate::thread::Status::Resolved
        && lane.merged_sha.is_empty()
        && crate::review::lane_review_from(reviews, lane).is_none()
        && crate::events::latest_event(events, &lane.id, lane.attempt.max(1)).is_some_and(|event| {
            !crate::threads::follow_up_pending_for_seal(lane, Some(event))
                && (event.payload.done.is_some()
                    || (event.payload.waiting.is_some() && event.id != lane.answered_waiting_event))
        })
}

pub(crate) fn drop_task(
    ctx: &Ctx,
    project: &Project,
    id: &str,
    reason: &str,
) -> Result<DropOutcome> {
    if reason.trim().is_empty() {
        return Err(crate::refusal::error(
            "task_drop: --reason is required",
            "ha task drop <project> <job> --reason \"<reason>\"",
        ));
    }
    let record = load(project, id)?;
    let attempts = record
        .attempts
        .iter()
        .map(|id| crate::thread::load(project, id))
        .collect::<Result<Vec<_>>>()?;
    // Hold the same repository locks as review allocation and landing until
    // cancellation, task retirement and lane retirement are durable.
    let _operations = crate::review::task_drop_locks(ctx, &attempts)?;
    crate::review::cancel_task_reviews(ctx, project, &attempts)?;
    let events = crate::events::checked(project)?;
    let reviews = crate::review::list(project)?;
    let task = update(project, id, |task| {
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
    })?;
    let lanes = attempts
        .iter()
        .filter(|lane| sealed_unlanded(lane, &events, &reviews))
        .map(|lane| {
            crate::threads::resolve_automatically(
                ctx,
                project,
                &lane.id,
                &format!("task {id} dropped: {}", reason.trim()),
            )
        })
        .collect();
    crate::threads::refresh_plan(ctx, project);
    Ok(DropOutcome { task, lanes })
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
    update(project, id, |task| {
        for other in list_with_errors(project).0 {
            if other.id != id && other.attempts.iter().any(|attempt| attempt == thread) {
                return Err(crate::refusal::error(
                    format!("task_attempt: `{thread}` already belongs to `{}`", other.id),
                    "ha task show <project> <job> (use the task already linked to this thread)",
                ));
            }
        }
        if !task.attempts.iter().any(|attempt| attempt == thread) {
            task.attempts.push(thread.to_string());
        }
        Ok(())
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Incident {
    pub(crate) cause: String,
    pub(crate) boundary: String,
    /// The original authorized recovery/spend/time limits, not a renewed budget.
    pub(crate) limits: String,
    pub(crate) confirmations: Vec<Confirmation>,
    #[serde(default)]
    pub(crate) installations: Vec<Evidence>,
    #[serde(default)]
    pub(crate) exercises: Vec<Exercise>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Confirmation {
    pub(crate) event: String,
    pub(crate) at: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Exercise {
    pub(crate) event: String,
    pub(crate) machine: String,
    pub(crate) build: String,
    pub(crate) evidence: String,
}

/// Only explicit coordinator judgments create associations. Retried transients
/// stay in events and recovery; they never create work here.
#[derive(Debug, clap::Args)]
pub(crate) struct RepairArgs {
    pub(crate) slug: String,
    pub(crate) id: String,
    #[command(subcommand)]
    pub(crate) command: RepairCommand,
}

#[derive(Debug, clap::Subcommand)]
pub(crate) enum RepairCommand {
    /// Confirm that these failure/wait events share this diagnosed cause
    Link {
        #[arg(long)]
        cause: String,
        #[arg(long)]
        boundary: String,
        /// Required for the first link; retained unchanged on recurrence
        #[arg(long)]
        limits: Option<String>,
        #[arg(long, required = true)]
        event: Vec<String>,
        #[arg(long)]
        reason: String,
    },
    /// Record machine/build installation proof for an already reviewed repair
    Install {
        #[arg(long)]
        cause: String,
        #[arg(long)]
        machine: String,
        #[arg(long)]
        build: String,
        #[arg(long)]
        at: String,
        #[arg(long)]
        evidence: String,
    },
    /// Judge a sealed successful exercise of the original failed boundary
    Exercise {
        #[arg(long)]
        cause: String,
        #[arg(long)]
        boundary: String,
        #[arg(long)]
        machine: String,
        #[arg(long)]
        build: String,
        #[arg(long)]
        event: String,
        #[arg(long)]
        evidence: String,
    },
}

fn nonempty(values: &[&str]) -> Result<()> {
    if values.iter().any(|value| value.trim().is_empty()) {
        bail!("repair_evidence: cause, boundary, limits and proof must be nonempty");
    }
    Ok(())
}

fn time(value: &str) -> Result<jiff::Timestamp> {
    value
        .parse()
        .context("repair_evidence: expected a timestamp")
}

fn incident_mut<'a>(task: &'a mut Task, cause: &str) -> Result<&'a mut Incident> {
    task.incidents
        .iter_mut()
        .find(|row| row.cause == cause)
        .context("repair_cause: link the confirmed failure first")
}

/// This is an ordinary task mutation, never part of sealing or delivery.
pub(crate) fn record_repair(project: &Project, id: &str, command: RepairCommand) -> Result<Task> {
    let lock = project.lock()?;
    let mut task = load(project, id)?;
    let snapshot = EvidenceSnapshot::load(project);
    match command {
        RepairCommand::Link {
            cause,
            boundary,
            limits,
            event,
            reason,
        } => {
            nonempty(&[&cause, &boundary, &reason])?;
            let (tasks, errors) = list_with_errors(project);
            if !errors.is_empty() {
                bail!("repair_evidence: unreadable tasks; association unknown");
            }
            for other in tasks.iter().filter(|other| other.id != id) {
                if other.incidents.iter().any(|row| row.cause == cause) {
                    bail!(
                        "repair_duplicate: cause `{cause}` already belongs to {}; retain its events and original limits there",
                        other.id
                    );
                }
                if other.incidents.iter().any(|row| {
                    row.confirmations
                        .iter()
                        .any(|row| event.contains(&row.event))
                }) {
                    bail!("repair_duplicate: an event already belongs to {}", other.id);
                }
            }
            if !snapshot.readable {
                bail!("repair_evidence: unreadable events");
            }
            for id in &event {
                let sealed = snapshot
                    .events
                    .iter()
                    .find(|row| &row.id == id)
                    .context("repair_event: failure event not found")?;
                if crate::events::incident_text(sealed).is_none() {
                    bail!("repair_event: {id} is not a single failure or dependency wait");
                }
                time(&sealed.created)?;
                if task.incidents.iter().any(|row| {
                    row.cause != cause && row.confirmations.iter().any(|row| &row.event == id)
                }) {
                    bail!("repair_duplicate: event {id} already has a confirmed cause");
                }
            }
            if event.is_empty() {
                bail!("repair_event: supply a failure event");
            }
            if !task.incidents.iter().any(|row| row.cause == cause) {
                let limits = limits.as_deref().context(
                    "repair_limits: preserve the original limits (use unknown when unavailable)",
                )?;
                nonempty(&[limits])?;
                task.incidents.push(Incident {
                    cause: cause.clone(),
                    boundary: boundary.clone(),
                    limits: limits.into(),
                    confirmations: vec![],
                    installations: vec![],
                    exercises: vec![],
                });
            }
            let incident = incident_mut(&mut task, &cause)?;
            if incident.boundary != boundary
                || limits.is_some_and(|limits| limits != incident.limits)
            {
                bail!("repair_limits: recurrence cannot change the original boundary or limits");
            }
            for event in event {
                if !incident.confirmations.iter().any(|row| row.event == event) {
                    incident.confirmations.push(Confirmation {
                        event,
                        at: project::now(),
                        reason: reason.clone(),
                    });
                }
            }
        }
        RepairCommand::Install {
            cause,
            machine,
            build,
            at,
            evidence,
        } => {
            nonempty(&[&cause, &machine, &build, &evidence])?;
            let installed_at = time(&at)?;
            if installed_at > jiff::Timestamp::now() {
                bail!("repair_install: installation is in the future");
            }
            require_accepted(project, &task, &snapshot)?;
            let lane = crate::thread::load(
                project,
                task.attempts
                    .last()
                    .context("repair_install: no repair attempt")?,
            )?;
            let delivered = if let Some(review) = crate::review::lane_review(project, &lane)? {
                let verdict = crate::events::load(project, &review.verdict_event)?;
                if installed_at < time(&verdict.created)? {
                    bail!("repair_install: installation predates review");
                }
                review.install_required
                    && review.install
                    && review
                        .verdict
                        .as_ref()
                        .is_some_and(|verdict| verdict.candidate == build)
            } else {
                !lane.merged_sha.is_empty() && lane.installed_sha == build
            };
            let recorded = task.installed.iter().any(|row| {
                row.machine.as_deref() == Some(&machine)
                    && row.build.as_deref() == Some(&build)
                    && time(&row.at).is_ok_and(|at| at == installed_at)
            });
            if !delivered && !recorded {
                bail!("repair_install: no reviewed installed outcome for this build");
            }
            let incident = incident_mut(&mut task, &cause)?;
            if !incident.installations.iter().any(|row| {
                row.machine.as_deref() == Some(&machine)
                    && row.build.as_deref() == Some(&build)
                    && row.at == at
            }) {
                incident.installations.push(Evidence {
                    at,
                    command: evidence,
                    acceptance: vec![],
                    machine: Some(machine),
                    build: Some(build),
                });
            }
        }
        RepairCommand::Exercise {
            cause,
            boundary,
            machine,
            build,
            event,
            evidence,
        } => {
            nonempty(&[&cause, &boundary, &machine, &build, &event, &evidence])?;
            let sealed = snapshot
                .events
                .iter()
                .find(|row| row.id == event)
                .context("repair_exercise: event missing")?;
            if !snapshot.readable
                || sealed.payload.failed.is_some()
                || sealed.payload.waiting.is_some()
            {
                bail!("repair_exercise: successful seal evidence unknown");
            }
            let done = sealed
                .payload
                .done
                .as_ref()
                .context("repair_exercise: not a successful seal")?;
            crate::thread::artifact(project, &done.artifact)
                .context("repair_exercise: successful report missing or corrupt")?;
            let at = time(&sealed.created)?;
            let incident = incident_mut(&mut task, &cause)?;
            if incident.boundary != boundary {
                bail!("repair_exercise: did not exercise the failed boundary");
            }
            if !incident.installations.iter().any(|row| {
                row.machine.as_deref() == Some(&machine)
                    && row.build.as_deref() == Some(&build)
                    && time(&row.at).is_ok_and(|installed| installed < at)
            }) {
                bail!(
                    "repair_exercise: success must follow installation on the same machine/build"
                );
            }
            let exercise = Exercise {
                event,
                machine,
                build,
                evidence,
            };
            if !incident.exercises.contains(&exercise) {
                incident.exercises.push(exercise);
            }
        }
    }
    write(project, &task)?;
    drop(lock);
    project::refresh_page(project)?;
    Ok(task)
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RepairView {
    pub(crate) cause: String,
    pub(crate) boundary: String,
    pub(crate) limits: String,
    pub(crate) outcome: String,
    pub(crate) timeline: Vec<String>,
    pub(crate) cost: crate::usage::Cost,
    /// Recorded judgments only. No claim about unrecorded human effort.
    pub(crate) interventions: usize,
}

fn repair_views(project: &Project, task: &Task, snapshot: &EvidenceSnapshot) -> Vec<RepairView> {
    task.incidents
        .iter()
        .map(|incident| {
            let mut timeline = Vec::new();
            let mut cost_events = Vec::new();
            let mut known = snapshot.readable
                && (incident.installations.is_empty()
                    || require_accepted(project, task, snapshot).is_ok());
            let latest_install = incident
                .installations
                .iter()
                .filter_map(|row| time(&row.at).ok().map(|at| (at, row)))
                .max_by_key(|row| row.0);
            let mut last_failure = None;
            for confirmation in &incident.confirmations {
                if let Some(event) = snapshot.events.iter().find(|event| {
                    event.id == confirmation.event && crate::events::incident_text(event).is_some()
                }) {
                    cost_events.push(event);
                    if let Ok(at) = time(&event.created) {
                        last_failure =
                            Some(last_failure.map_or(at, |last: jiff::Timestamp| last.max(at)));
                        let recurrence = incident
                            .installations
                            .iter()
                            .any(|row| time(&row.at).is_ok_and(|installed| at > installed));
                        timeline.push(format!(
                            "{} {} {}/attempt {}{}: {} (confirmed: {})",
                            event.created,
                            event.id,
                            event.thread,
                            event.attempt,
                            if recurrence {
                                " recurrence after install"
                            } else {
                                ""
                            },
                            crate::events::incident_text(event).unwrap_or("unknown"),
                            confirmation.reason
                        ));
                        timeline.extend(
                            crate::events::recovery_facts(project, &event.id)
                                .into_iter()
                                .map(|fact| format!("{} {} {fact}", event.created, event.id)),
                        );
                    } else {
                        known = false;
                    }
                } else {
                    known = false;
                    timeline.push(format!("{} evidence unknown", confirmation.event));
                }
            }
            for id in &task.attempts {
                if let Some(lane) = snapshot.lanes.get(id) {
                    cost_events.extend(snapshot.events.iter().filter(|event| event.thread == *id));
                    match snapshot.lane_review(lane) {
                        Ok(Some(review)) => timeline.push(format!(
                            "review {}: merged; install {}",
                            review.id,
                            if review.install_required && review.install {
                                "recorded"
                            } else {
                                "not established"
                            }
                        )),
                        Ok(None) if !lane.merged_sha.is_empty() => {
                            timeline.push(format!("merged {}", lane.merged_sha))
                        }
                        Err(_) => {
                            known = false;
                            timeline.push("review evidence unknown".into());
                        }
                        _ => {}
                    }
                }
            }
            for row in &incident.installations {
                timeline.push(format!(
                    "{} installed {} {}: {}",
                    row.at,
                    row.machine.as_deref().unwrap_or("unknown"),
                    row.build.as_deref().unwrap_or("unknown"),
                    row.command
                ));
            }
            let mut last_success = None;
            for exercise in &incident.exercises {
                if let Some(event) = snapshot
                    .events
                    .iter()
                    .find(|event| event.id == exercise.event)
                {
                    cost_events.push(event);
                    let successful = event.payload.failed.is_none()
                        && event.payload.waiting.is_none()
                        && event.payload.done.as_ref().is_some_and(|done| {
                            crate::thread::artifact(project, &done.artifact).is_ok()
                        });
                    if !successful {
                        known = false;
                    }
                    if let Some((installed, installation)) = latest_install
                        && installation.machine.as_deref() == Some(&exercise.machine)
                        && installation.build.as_deref() == Some(&exercise.build)
                        && successful
                        && let Ok(at) = time(&event.created)
                        && at > installed
                    {
                        last_success =
                            Some(last_success.map_or(at, |last: jiff::Timestamp| last.max(at)));
                    }
                    timeline.push(format!(
                        "{} {} exercised {} on {}/{}: {}",
                        event.created,
                        event.id,
                        incident.boundary,
                        exercise.machine,
                        exercise.build,
                        exercise.evidence
                    ));
                } else {
                    known = false;
                    timeline.push(format!("{} exercise evidence unknown", exercise.event));
                }
            }
            timeline.sort();
            let outcome = if !known {
                "unknown"
            } else if latest_install.is_some_and(|(installed, _)| {
                last_failure.is_some_and(|failed| {
                    failed > installed && last_success.is_none_or(|success| failed >= success)
                })
            }) {
                "recurred after install"
            } else if last_success.is_some() {
                "effective at exercised boundary"
            } else if latest_install.is_some() {
                "installed; boundary unexercised"
            } else {
                "repair not established"
            };
            RepairView {
                cause: incident.cause.clone(),
                boundary: incident.boundary.clone(),
                limits: incident.limits.clone(),
                outcome: outcome.into(),
                timeline,
                cost: crate::usage::cost(&cost_events),
                interventions: incident.confirmations.len()
                    + incident.installations.len()
                    + incident.exercises.len(),
            }
        })
        .collect()
}

pub(crate) fn repair_summary(repair: &RepairView) -> String {
    format!(
        "{}: {} — boundary {}; original limits {}; {}; {} recorded judgments (other intervention cost unknown)\n{}",
        repair.cause,
        repair.outcome,
        repair.boundary,
        repair.limits,
        repair.cost.summary(),
        repair.interventions,
        repair
            .timeline
            .iter()
            .map(|row| format!("  {row}\n"))
            .collect::<String>()
    )
}

pub(crate) struct EvidenceSnapshot {
    events: Vec<crate::contracts::Event>,
    readable: bool,
    pub(crate) tasks: Vec<Task>,
    task_errors: Vec<anyhow::Error>,
    pub(crate) lanes: BTreeMap<String, crate::thread::Thread>,
    reviews: Result<Vec<crate::review::Review>>,
    pub(crate) binding_changes: Vec<crate::plan::BindingChange>,
}

impl EvidenceSnapshot {
    pub(crate) fn load(project: &Project) -> Self {
        let (events, readable) = crate::events::list_checked(project);
        let binding_changes = crate::plan::binding_changes(project);
        let readable = readable && binding_changes.is_ok();
        let (tasks, errors) = list_with_errors(project);
        Self {
            events,
            readable,
            tasks,
            task_errors: errors,
            lanes: crate::thread::snapshot(project)
                .iter()
                .cloned()
                .map(|lane| (lane.id.clone(), lane))
                .collect(),
            reviews: crate::review::list(project),
            binding_changes: binding_changes.unwrap_or_default(),
        }
    }

    pub(crate) fn tasks_readable(&self) -> bool {
        self.task_errors.is_empty()
    }

    fn lane_review(&self, lane: &crate::thread::Thread) -> Result<Option<&crate::review::Review>> {
        let rows = self
            .reviews
            .as_deref()
            .map_err(|e| anyhow::anyhow!("{e:#}"))?;
        Ok(crate::review::lane_review_from(rows, lane))
    }

    pub(crate) fn lane_done(&self, lane: &crate::thread::Thread) -> bool {
        crate::review::lane_done_from(lane, &self.events, self.lane_review(lane).ok().flatten())
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
    let repairs = repair_views(project, &task, evidence);
    let mut view = View {
        record: task,
        state: State::Open,
        next: "start an attempt".into(),
        failure_class: None,
        provider_kind: None,
        repairs,
    };
    if !view.record.dropped.is_empty() || !view.record.installed.is_empty() {
        view.state = if view.record.dropped.is_empty() {
            State::Installed
        } else {
            State::Dropped
        };
        view.next = "none".into();
        return view;
    }
    let Some(id) = view.record.attempts.last() else {
        return view;
    };
    let Some(lane) = evidence.lanes.get(id) else {
        view.next = "repair the missing attempt record".into();
        return view;
    };
    if !evidence.readable {
        view.next = "repair the unreadable event evidence".into();
        return view;
    }
    let review = match evidence.lane_review(lane) {
        Ok(review) => review,
        Err(_) => {
            view.next = "repair the unreadable review record".into();
            return view;
        }
    };
    if review.is_some() || !lane.merged_sha.is_empty() {
        let required = review.map_or(lane.historical_install_required, |r| r.install_required);
        let installed = match review {
            Some(r) => r.install_required && r.install,
            None => !lane.installed_sha.is_empty(),
        };
        view.state = if installed {
            State::Installed
        } else {
            State::Merged
        };
        view.next = if required && !installed {
            "finish the pile installation".into()
        } else {
            "none".into()
        };
        return view;
    }
    if lane.status == crate::thread::Status::Resolved && !lane.cancellation_reason.is_empty() {
        view.state = State::Open;
        view.next = "start a new attempt or drop the task".into();
        return view;
    }
    if let Some(seal) = crate::review::sealed(&evidence.events, lane) {
        view.state = State::Finished;
        view.next = if evidence.lane_done(lane) {
            "none".into()
        } else {
            "review the repository pile".into()
        };
        if crate::review::changes(lane, seal) == Some(false)
            && require_accepted(project, &view.record, evidence).is_err()
        {
            view.next =
                "judge required acceptance through coordinator/critic (not established)".into();
        }
        return view;
    }
    view.state = State::Working;
    view.next = crate::threads::pending_start_note(lane)
        .unwrap_or_else(|| "finish the current attempt".into());
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
        view.provider_kind = lane.provider_failure_kind.clone();
        view.next = "retry or cancel the current attempt".into();
    }
    if lane.status == crate::thread::Status::Resolved {
        view.state = State::Open;
        view.next = "lane ended without done; retry or attest its stored report".into();
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
    let lane = snapshot
        .lanes
        .get(id)
        .context("acceptance not established: missing attempt")?;
    // Named historical consumer: pre-pile merged/installed records retain
    // their accepted delivery semantics, even without a retained done seal.
    // No-change equality is not that fact.
    if !lane.merged_sha.is_empty() && lane.merged_review.is_empty() {
        return Ok(());
    }
    let event = crate::review::sealed(snapshot.events(), lane)
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
    if let Some(review) = snapshot.lane_review(lane)? {
        let seal = snapshot
            .events()
            .iter()
            .find(|event| event.id == review.verdict_event)
            .context("acceptance not established: reviewer seal missing")?;
        let judged = seal
            .payload
            .done
            .as_ref()
            .context("acceptance not established: reviewer seal missing")?;
        let text = String::from_utf8(crate::thread::artifact(project, &judged.artifact)?)?;
        let criteria = report_criteria(&text)?;
        if criteria_established(task, id, &event.id, &criteria) {
            return Ok(());
        }
    }
    // Reuse an existing critic when one was requested. No compulsory second
    // judge, and a critic's PASS without criterion evidence is not enough.
    for critic in snapshot
        .lanes
        .values()
        .filter(|critic| critic.role == "critic" && critic.id != lane.id)
    {
        let Some(seal) = crate::review::sealed(snapshot.events(), critic) else {
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
    (
        evidence
            .tasks
            .iter()
            .cloned()
            .map(|task| view_with_evidence(project, task, evidence))
            .collect(),
        evidence
            .task_errors
            .iter()
            .map(|error| anyhow::anyhow!("{error:#}"))
            .collect(),
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
    for repair in &view.repairs {
        text.push_str(&format!("  repair {}\n", repair_summary(repair)));
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
