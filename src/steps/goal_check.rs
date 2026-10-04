//! One durable outcome-check obligation. Transport receipts never discharge it.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    contracts::StepState,
    herdr::Agent,
    project::{self, Project},
};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub(crate) struct Check {
    pub(crate) generation: u64,
    exhausted: Option<bool>,
    wait_answers: Option<Vec<String>>,
    results: String,
    request: String,
    effects: Vec<String>,
    actions: Vec<String>,
    last: Option<(Disposition, String)>,
    /// Scoped waits survive independent actions; existing lane seals still own
    /// lane input/retry effects. A goal-only wait has an empty task list.
    pub(crate) waits: Vec<(Disposition, String)>,
    /// Indices into the append-only wait history.
    retired_waits: Vec<usize>,
    answers: Vec<Answer>,
    pub(crate) disposition: Option<Disposition>,
    pub(crate) evidence: String,
    queued: bool,
    delivered_at: u64,
    working: bool,
    binding: String,
    unchanged: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Disposition {
    Action {
        task: String,
    },
    Closed {
        tasks: Vec<String>,
        outcome: String,
    },
    Wait {
        tasks: Vec<String>,
        party: String,
        condition: String,
    },
    NeedsRolf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Answer {
    party: String,
    evidence: String,
    waits: Vec<usize>,
}

fn path(project: &Project) -> std::path::PathBuf {
    project.state_dir().join("goal-check.json")
}

pub(crate) fn load(project: &Project) -> Check {
    project::read_json(&path(project)).unwrap_or_default()
}

fn save(project: &Project, check: &Check) -> Result<()> {
    project::write_json(&path(project), check)
}

fn lock(project: &Project) -> Result<std::fs::File> {
    project::lock_file(&project.state_dir().join("goal-check.lock"))
}

struct Snapshot {
    results: String,
    request: String,
    effects: Vec<String>,
    actions: Vec<String>,
    exhausted: bool,
    unfinished: bool,
}

fn snapshot(project: &Project) -> Result<Snapshot> {
    let plan = crate::plan::load(project)?;
    let (tasks, errors) = crate::task::list_with_errors(project);
    if let Some(error) = errors.into_iter().next() {
        return Err(error);
    }
    let (events, readable) = crate::events::list_checked(project);
    if !readable {
        bail!("goal_check: unreadable result evidence");
    }
    let reviews = crate::review::list(project)?;
    let lanes = crate::thread::list(project);
    let mut actions = Vec::new();
    for task in &tasks {
        if task.authority.is_empty() || task.acceptance.is_empty() || !task.dropped.is_empty() {
            continue;
        }
        let linked = plan.as_ref().is_some_and(|p| {
            crate::plan::all_steps(p)
                .any(|s| s.state != StepState::Done && s.tasks.contains(&task.id))
        });
        let started = task
            .attempts
            .last()
            .and_then(|id| lanes.iter().find(|t| &t.id == id))
            .filter(|t| {
                matches!(
                    t.status,
                    crate::thread::Status::Starting | crate::thread::Status::Open
                )
            })
            .filter(|t| crate::events::latest_event(&events, &t.id, t.attempt.max(1)).is_none());
        if (linked || started.is_some())
            && crate::plan::check_prerequisites(project, &task.id).is_ok()
        {
            actions.push(format!(
                "{}:{}",
                task.id,
                started.map_or(String::new(), |t| format!("{}:{}", t.id, t.attempt))
            ));
        }
    }
    actions.sort();
    let exhausted = plan
        .as_ref()
        .is_none_or(|p| p.steps.iter().all(|s| s.state == StepState::Done));
    // Seals remain independent evidence. A review contributes only once its
    // outcome is known, not at start or at each merge/push/install cursor.
    // Keep already-owed review ids through retries/cleanup without a new field.
    let owed = load(project).effects;
    let result_keys: Vec<_> = events.iter().map(|e| &e.id).collect();
    let review_keys: Vec<_> = reviews
        .iter()
        .filter(|r| {
            owed.contains(&r.id)
                || r.phase == crate::review::Phase::Rejected
                || (r.fast_forward && (!r.install_required || r.install))
                || (r.phase == crate::review::Phase::Landing && !r.attention.is_empty())
        })
        .map(|r| &r.id)
        .collect();
    let results = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(&result_keys, &review_keys))?)
    );
    let effects = result_keys
        .iter()
        .map(|id| (*id).clone())
        .chain(review_keys.iter().map(|id| (*id).clone()))
        .collect();
    let request = crate::prompt::latest_request_id(project);
    let evidence = crate::task::EvidenceSnapshot::load(project);
    Ok(Snapshot {
        results,
        request,
        effects,
        actions,
        exhausted,
        unfinished: tasks.iter().any(|t| {
            !crate::task::view_with_evidence(project, t.clone(), &evidence)
                .terminal_with_evidence(project, &evidence)
        }),
    })
}

/// Observe answers to the sealed lane waits covered by recorded dispositions.
/// Starting/linking work and editing its records are not outside evidence.
fn wait_answers(project: &Project, check: &Check) -> Vec<String> {
    if check.waits.is_empty() {
        return Vec::new();
    }
    let (tasks, _) = crate::task::list_with_errors(project);
    let open = open_indices(project, check);
    let events = crate::events::list(project);
    let mut answers: Vec<_> = crate::thread::list(project)
        .into_iter()
        .filter(|lane| {
            open.iter().any(|index| {
                let wait = &check.waits[*index].0;
                matches!(wait, Disposition::Wait { tasks: ids, .. } if ids.is_empty() || tasks.iter().any(|task| ids.contains(&task.id) && task.attempts.contains(&lane.id)))
            })
        })
        .filter_map(|lane| {
            crate::events::latest_event(&events, &lane.id, lane.attempt.max(1))
                .filter(|event| {
                    event.payload.waiting.is_some() && lane.answered_waiting_event == event.id
                })
                .map(|event| event.id.clone())
        })
        .collect();
    answers.sort();
    answers
}

/// Reconcile even while lanes/review run: independent work must not be hidden.
/// Only outside evidence or newly exhausted work owes another judgment.
pub(crate) fn reconcile(project: &Project, agent: Option<&Agent>, now: u64) -> Result<()> {
    // Serialize evidence reads with dispositions too: an older snapshot must
    // not overwrite a command that completed while reconciliation waited.
    let _lock = lock(project)?;
    let snapshot = snapshot(project)?;
    let mut check = load(project);
    let before = check.clone();
    let answers = wait_answers(project, &check);
    let fresh_evidence = check.results != snapshot.results
        || check.request != snapshot.request
        || (check.exhausted == Some(false) && snapshot.exhausted)
        || check
            .wait_answers
            .as_ref()
            .is_some_and(|old| answers.iter().any(|answer| !old.contains(answer)));
    if check.generation > 0
        && check.disposition.is_none()
        && !fresh_evidence
        && let Some(action) = snapshot.actions.iter().find(|a| !check.actions.contains(a))
    {
        let task = action.split(':').next().unwrap().to_string();
        let record = crate::task::load(project, &task)?;
        retire(&mut check, |wait| wait_affects(wait, &task));
        check.disposition = Some(Disposition::Action { task });
        check.evidence = format!(
            "{}; acceptance: {}",
            record.title,
            record.acceptance.join("; ")
        );
    }
    // Remember bookkeeping without owing a wake or resetting bounded diagnosis.
    // Optional baselines let historical records load without a spurious check.
    check.exhausted = Some(snapshot.exhausted);
    check.wait_answers = Some(answers);
    check.actions = snapshot.actions;
    if (check.generation == 0 && (snapshot.exhausted || snapshot.unfinished)) || fresh_evidence {
        check.generation += 1;
        check.results = snapshot.results;
        check.request = snapshot.request;
        check.effects = snapshot.effects;
        if let Some(previous) = check.disposition.take() {
            check.last = Some((previous, std::mem::take(&mut check.evidence)));
        }
        check.evidence.clear();
        check.queued = false;
        check.delivered_at = 0;
        check.working = false;
        check.unchanged = 0;
    }
    if check.disposition.is_none()
        && check.delivered_at > 0
        && let Some(agent) = agent
    {
        if !agent.ready() {
            check.working = true;
        }
        let binding = format!(
            "{}:{}",
            agent.pane_id,
            agent.agent_session.as_ref().map_or("", |s| s.id.as_str())
        );
        // A lost/replaced or missed turn is not progress. Bound the lack
        // of disposition too, without claiming that a turn happened.
        if agent.ready()
            && (check.working
                || binding != check.binding
                || now.saturating_sub(check.delivered_at) >= 600)
        {
            check.unchanged += 1;
            check.delivered_at = 0;
            check.working = false;
            check.queued = false;
            if check.unchanged >= 2 {
                check.disposition = Some(Disposition::NeedsRolf);
                check.evidence = "No outcome disposition after two deliveries; needs Rolf to inspect the coordinator. No further automatic wake.".into();
            }
        }
    }
    if check != before {
        save(project, &check)?;
    }
    Ok(())
}

pub(crate) fn notice(project: &Project) -> Option<(String, String)> {
    let check = load(project);
    if check.generation == 0 || check.disposition.is_some() || check.queued {
        return None;
    }
    let token = format!("{}:{}", check.generation, check.unchanged);
    let diagnosis = if check.unchanged == 0 {
        "Compare results with the original outcome and acceptance, including absent/exhausted plans."
    } else {
        "The previous delivery produced no disposition. Diagnose the missing evidence once; change the approach or record an explicit wait. Do not repeat the previous wake's action."
    };
    Some((
        token,
        format!(
            "{} Goal check owed. {diagnosis} Articulate with ha plan set --does if needed. Record ha plan check {} action <job> --evidence <reason>, close --task <job> --evidence <acceptance evidence>, or wait <party> --condition <condition> --evidence <reason>. Start independent authorized work even during a sealed wait or review. A submitted prompt is not progress.",
            super::TICKER_PROMPT_PREFIX,
            project.slug
        ),
    ))
}

pub(crate) fn current(project: &Project, token: &str) -> bool {
    let check = load(project);
    check.disposition.is_none() && token == format!("{}:{}", check.generation, check.unchanged)
}

pub(crate) fn queued(project: &Project, token: &str) -> Result<()> {
    let _lock = lock(project)?;
    let mut check = load(project);
    if token == format!("{}:{}", check.generation, check.unchanged) && check.disposition.is_none() {
        check.queued = true;
        save(project, &check)?;
    }
    Ok(())
}

pub(crate) fn delivered(project: &Project, token: &str, agent: &Agent, now: u64) -> Result<()> {
    let _lock = lock(project)?;
    let mut check = load(project);
    if token == format!("{}:{}", check.generation, check.unchanged) && check.disposition.is_none() {
        check.queued = true;
        check.delivered_at = now;
        check.binding = format!(
            "{}:{}",
            agent.pane_id,
            agent.agent_session.as_ref().map_or("", |s| s.id.as_str())
        );
        check.working = !agent.ready();
        save(project, &check)?;
    }
    Ok(())
}

/// Semantic judgment remains the coordinator's; require real task/report
/// anchors here, not a transport acknowledgement or checklist count.
pub(crate) fn record(project: &Project, disposition: Disposition, evidence: &str) -> Result<()> {
    if evidence.trim().is_empty() {
        bail!("goal_check: explain the evidence and disposition");
    }
    let _lock = lock(project)?;
    let snapshot = snapshot(project)?;
    let mut check = load(project);
    if check.generation == 0 {
        bail!("goal_check: no check is owed yet");
    }
    match &disposition {
        Disposition::Action { task } => {
            if !snapshot
                .actions
                .iter()
                .any(|a| a.starts_with(&format!("{task}:")))
            {
                bail!(
                    "goal_check: action must link or start a request-backed task with acceptance"
                );
            }
            crate::plan::check_prerequisites(project, task)?;
        }
        Disposition::Closed { tasks, outcome } => {
            let plan = crate::plan::load(project)?.ok_or_else(|| {
                anyhow::anyhow!("goal_check: articulate Plan.does before closure")
            })?;
            if outcome.trim().is_empty() || outcome != &plan.does || tasks.is_empty() {
                bail!("goal_check: closure needs Plan.does and acceptance-bearing task evidence");
            }
            let all = crate::task::EvidenceSnapshot::load(project);
            for id in tasks {
                let task = crate::task::load(project, id)?;
                if task.authority.is_empty()
                    || task.acceptance.is_empty()
                    || !task.dropped.is_empty()
                    || !crate::task::view_with_evidence(project, task.clone(), &all)
                        .terminal_with_evidence(project, &all)
                {
                    bail!("goal_check: {id} has no terminal acceptance-bearing evidence");
                }
                crate::task::require_accepted(project, &task, &all)?;
            }
            if snapshot.unfinished || !snapshot.exhausted {
                bail!("goal_check: unfinished work still needs an action or explicit wait");
            }
        }
        Disposition::Wait {
            tasks,
            party,
            condition,
        } => {
            if party.trim().is_empty() || condition.trim().is_empty() {
                bail!("goal_check: wait needs who/what and its wake condition");
            }
            for id in tasks {
                crate::task::load(project, id)?;
            }
        }
        Disposition::NeedsRolf => bail!("goal_check: escalation is automatic"),
    }
    check.exhausted = Some(snapshot.exhausted);
    check.results = snapshot.results;
    check.request = snapshot.request;
    check.effects = snapshot.effects;
    check.actions = snapshot.actions;
    // This is a later recorded judgment, not merely a reconciliation tick.
    let settled: Vec<_> = check
        .waits
        .iter()
        .enumerate()
        .filter(|(_, (wait, _))| party_answered(project, wait))
        .map(|(index, _)| index)
        .collect();
    for index in settled {
        if !check.retired_waits.contains(&index) {
            check.retired_waits.push(index);
        }
    }
    match &disposition {
        Disposition::Wait { tasks, .. } => {
            retire(&mut check, |wait| match wait {
                Disposition::Wait { tasks: old, .. } => {
                    (old.is_empty() && tasks.is_empty()) || tasks.iter().any(|id| old.contains(id))
                }
                _ => false,
            });
            check
                .waits
                .push((disposition.clone(), evidence.trim().into()));
        }
        Disposition::Action { task } => retire(&mut check, |wait| wait_affects(wait, task)),
        Disposition::Closed { .. } => retire(&mut check, |_| true),
        _ => {}
    }
    check.disposition = Some(disposition);
    check.wait_answers = Some(wait_answers(project, &check));
    check.evidence = evidence.trim().into();
    save(project, &check)
}

fn wait_affects(wait: &Disposition, task: &str) -> bool {
    matches!(wait, Disposition::Wait { tasks, .. } if tasks.is_empty() || tasks.iter().any(|id| id == task))
}

fn retire(check: &mut Check, affects: impl Fn(&Disposition) -> bool) {
    for (index, (wait, _)) in check.waits.iter().enumerate() {
        if affects(wait) && !check.retired_waits.contains(&index) {
            check.retired_waits.push(index);
        }
    }
}

/// Positive lane evidence only; a free-text condition is not guessed from a prompt.
fn party_answered(project: &Project, wait: &Disposition) -> bool {
    let Disposition::Wait { party, .. } = wait else {
        return false;
    };
    let Ok(lane) = crate::thread::load(project, party) else {
        return false;
    };
    lane.status == crate::thread::Status::Resolved
        || crate::events::latest_event(&crate::events::list(project), &lane.id, lane.attempt.max(1))
            .is_some_and(|event| {
                event.payload.done.is_some()
                    || (event.payload.waiting.is_some() && lane.answered_waiting_event == event.id)
            })
}

fn open_indices(project: &Project, check: &Check) -> Vec<usize> {
    if check.waits.is_empty() {
        return Vec::new();
    }
    open_indices_with_evidence(
        project,
        check,
        &crate::task::EvidenceSnapshot::load(project),
    )
}

fn open_indices_with_evidence(
    project: &Project,
    check: &Check,
    evidence: &crate::task::EvidenceSnapshot,
) -> Vec<usize> {
    check
        .waits
        .iter()
        .enumerate()
        .filter_map(|(index, (wait, _))| {
            let Disposition::Wait { tasks, .. } = wait else {
                return None;
            };
            if check.retired_waits.contains(&index) {
                return None;
            }
            // A current wait must still cover unfinished work, even in old files.
            if tasks.is_empty()
                || tasks.iter().all(|id| {
                    crate::task::load(project, id).is_ok_and(|task| {
                        crate::task::view_with_evidence(project, task, evidence)
                            .terminal_with_evidence(project, evidence)
                    })
                })
            {
                return None;
            }
            let supersedes = |newer: &Disposition| match newer {
                Disposition::Action { task } => wait_affects(wait, task),
                Disposition::Wait { tasks: newer, .. } => {
                    (tasks.is_empty() && newer.is_empty())
                        || newer.iter().any(|id| tasks.contains(id))
                }
                Disposition::Closed { .. } => true,
                Disposition::NeedsRolf => false,
            };
            if check.waits[index + 1..]
                .iter()
                .any(|(newer, _)| supersedes(newer))
            {
                return None;
            }
            // Historical files have no retirement indices. Their last/current judgment
            // still establishes supersession, but the wait's own disposition does not.
            if check
                .disposition
                .as_ref()
                .or_else(|| check.last.as_ref().map(|(d, _)| d))
                .is_some_and(|newer| newer != wait && supersedes(newer))
            {
                return None;
            }
            Some(index)
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn open_waits(project: &Project) -> Vec<(Disposition, String)> {
    let check = load(project);
    open_indices(project, &check)
        .into_iter()
        .map(|index| check.waits[index].clone())
        .collect()
}

pub(crate) fn open_waits_with_evidence(
    project: &Project,
    evidence: &crate::task::EvidenceSnapshot,
) -> Vec<(Disposition, String)> {
    let check = load(project);
    open_indices_with_evidence(project, &check, evidence)
        .into_iter()
        .map(|index| check.waits[index].clone())
        .collect()
}

/// An answer is outside evidence: owe a new judgment, retaining its source and waits.
pub(crate) fn answer(project: &Project, party: &str, evidence: &str) -> Result<()> {
    if party.trim().is_empty() || evidence.trim().is_empty() {
        bail!("goal_check: answer needs a party and evidence");
    }
    let _lock = lock(project)?;
    let mut check = load(project);
    let waits: Vec<_> = open_indices(project, &check)
        .into_iter()
        .filter(|index| {
            matches!(&check.waits[*index].0, Disposition::Wait { party: waiting, .. }
            if waiting.eq_ignore_ascii_case(party.trim()))
        })
        .collect();
    check.retired_waits.extend(&waits);
    check.answers.push(Answer {
        party: party.trim().into(),
        evidence: evidence.trim().into(),
        waits,
    });
    if let Some(previous) = check.disposition.take() {
        check.last = Some((previous, std::mem::take(&mut check.evidence)));
    }
    check.generation += 1;
    check.queued = false;
    check.delivered_at = 0;
    check.working = false;
    check.unchanged = 0;
    save(project, &check)
}

#[cfg(test)]
mod tests;

/// Coordinator obligations belong in the plan, not in Rolf's action list.
#[cfg(test)]
pub(crate) fn status(project: &Project) -> Option<String> {
    status_with_waits(project, &open_waits(project))
}

pub(crate) fn status_with_waits(
    project: &Project,
    waits: &[(Disposition, String)],
) -> Option<String> {
    let check = load(project);
    if check.generation == 0 {
        return None;
    }
    let mut text = match &check.disposition {
        None => "Goal check owed: coordinator must link a next action, acceptance evidence, or explicit wait.".into(),
        Some(Disposition::NeedsRolf) => check.evidence.clone(),
        Some(Disposition::Action { task }) => format!("Goal check action {task}: {}", check.evidence),
        Some(Disposition::Closed { outcome, .. }) => format!("Goal check closed: {outcome}; {}", check.evidence),
        Some(Disposition::Wait { .. }) if !waits.is_empty() => "Goal check waiting; next check on outside evidence, exhaustion, or wait answer.".into(),
        Some(Disposition::Wait { .. }) => "Goal check wait retired; awaiting the next outcome judgment.".into(),
    };
    for (wait, evidence) in waits {
        if let Disposition::Wait {
            tasks,
            party,
            condition,
        } = wait
        {
            text.push_str(&format!(
                "\n  Wait for {party} [{}]: {condition}; {evidence}",
                tasks.join(", ")
            ));
        }
    }
    Some(text)
}

/// Only escalation and explicitly personal waits require Rolf's attention.
#[cfg(test)]
pub(crate) fn attention(project: &Project) -> Option<String> {
    attention_with_waits(project, &open_waits(project))
}

pub(crate) fn attention_with_waits(
    project: &Project,
    waits: &[(Disposition, String)],
) -> Option<String> {
    let check = load(project);
    let mut lines = Vec::new();
    if check.disposition == Some(Disposition::NeedsRolf) {
        lines.push(check.evidence);
    }
    for (wait, _) in waits {
        if let Disposition::Wait {
            tasks,
            party,
            condition,
        } = wait
            && party.eq_ignore_ascii_case("Rolf")
        {
            lines.push(format!(
                "Goal check waits for Rolf [{}]: {condition}",
                tasks.join(", ")
            ));
        }
    }
    (!lines.is_empty()).then(|| lines.join("; "))
}
