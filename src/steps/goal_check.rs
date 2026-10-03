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
    source: String,
    results: String,
    request: String,
    effects: Vec<String>,
    actions: Vec<String>,
    last: Option<(Disposition, String)>,
    /// Scoped waits survive independent actions; existing lane seals still own
    /// lane input/retry effects. A goal-only wait has an empty task list.
    pub(crate) waits: Vec<(Disposition, String)>,
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
    source: String,
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
    // Do not include delivery journals, context reads, or coordinator bindings:
    // none is evidence of outcome progress. Keep merge/push/install distinct.
    let result_keys: Vec<_> = events.iter().map(|e| &e.id).collect();
    let review_keys: Vec<_> = reviews
        .iter()
        .map(|r| {
            (
                &r.id,
                &r.phase,
                &r.verdict_event,
                r.fast_forward,
                r.push,
                r.install,
                r.close,
                &r.attention,
            )
        })
        .collect();
    let results = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(&result_keys, &review_keys))?)
    );
    let effects = result_keys
        .iter()
        .map(|id| (*id).clone())
        .chain(reviews.iter().map(|r| r.id.clone()))
        .collect();
    let request = crate::prompt::latest_request_id(project);
    let source = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            plan,
            &tasks,
            result_keys,
            review_keys,
            &request
        ))?)
    );
    let evidence = crate::task::EvidenceSnapshot::load(project);
    Ok(Snapshot {
        source,
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

/// Reconcile even while lanes/review run: independent work must not be hidden.
/// A new request, plan/result change, or exhausted checklist owes judgment.
pub(crate) fn reconcile(project: &Project, agent: Option<&Agent>, now: u64) -> Result<()> {
    let snapshot = snapshot(project)?;
    let _lock = lock(project)?;
    let mut check = load(project);
    let before = check.clone();
    if check.generation > 0
        && check.disposition.is_none()
        && check.results == snapshot.results
        && let Some(action) = snapshot.actions.iter().find(|a| !check.actions.contains(a))
    {
        let task = action.split(':').next().unwrap().to_string();
        let record = crate::task::load(project, &task)?;
        check.waits.retain(|(wait, _)| !wait_affects(wait, &task));
        check.disposition = Some(Disposition::Action { task });
        check.evidence = format!(
            "{}; acceptance: {}",
            record.title,
            record.acceptance.join("; ")
        );
        check.source = snapshot.source.clone();
        check.actions = snapshot.actions.clone();
    }
    let changed = check.source != snapshot.source;
    let fresh_evidence = check.results != snapshot.results || check.request != snapshot.request;
    // While judgment is owed, plan wording or bookkeeping cannot buy an
    // endless supply of identical wakes. Coalesce it without resetting the
    // bounded diagnosis count. Only new result/request evidence does that.
    if check.generation > 0 && check.disposition.is_none() && changed && !fresh_evidence {
        check.source = snapshot.source.clone();
        check.actions = snapshot.actions.clone();
    }
    if (check.generation == 0 && (snapshot.exhausted || snapshot.unfinished))
        || (changed && (fresh_evidence || check.disposition.is_some()))
    {
        check.generation += 1;
        check.source = snapshot.source;
        check.results = snapshot.results;
        check.request = snapshot.request;
        check.effects = snapshot.effects;
        check.actions = snapshot.actions;
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
    let snapshot = snapshot(project)?;
    let _lock = lock(project)?;
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
                    || !crate::task::view_with_evidence(project, task, &all)
                        .terminal_with_evidence(project, &all)
                {
                    bail!("goal_check: {id} has no terminal acceptance-bearing evidence");
                }
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
    check.source = snapshot.source;
    check.results = snapshot.results;
    check.request = snapshot.request;
    check.effects = snapshot.effects;
    check.actions = snapshot.actions;
    match &disposition {
        Disposition::Wait { tasks, .. } => {
            check.waits.retain(|(wait, _)| match wait {
                Disposition::Wait { tasks: old, .. } => old != tasks,
                _ => false,
            });
            check
                .waits
                .push((disposition.clone(), evidence.trim().into()));
        }
        Disposition::Action { task } => check.waits.retain(|(wait, _)| !wait_affects(wait, task)),
        Disposition::Closed { .. } => check.waits.clear(),
        _ => {}
    }
    check.disposition = Some(disposition);
    check.evidence = evidence.trim().into();
    save(project, &check)
}

fn wait_affects(wait: &Disposition, task: &str) -> bool {
    matches!(wait, Disposition::Wait { tasks, .. } if tasks.is_empty() || tasks.iter().any(|id| id == task))
}

#[cfg(test)]
mod tests;

pub(crate) fn attention(project: &Project) -> Option<String> {
    let check = load(project);
    if check.generation == 0 {
        return None;
    }
    match check.disposition {
        None => Some("Goal check owed: coordinator must link a next action, acceptance evidence, or explicit wait.".into()),
        Some(Disposition::NeedsRolf) => Some(check.evidence),
        Some(Disposition::Wait { party, condition, .. }) => Some(format!("Goal check waits for {party}: {condition}; next check on request/result/plan change.")),
        _ => None,
    }
}
