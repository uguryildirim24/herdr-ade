//! The plan card (SPEC-talk §2.7 and §6.5): the file, its commands, checked
//! writes, revision guards and the shared state projection.
//!
//! A step's `state` is a persisted projection of the bound work, never a
//! coordinator-supplied status. The shared `refresh` reads the plan, the goal
//! and the durable work records under the plan writer lock and writes only on
//! change. It never rewrites the goal into the coordinator's wording.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::contracts::{Plan, PlanStep, StepState};
use crate::paths::Ctx;
use crate::project::{Project, write_atomic};
use crate::task::EvidenceSnapshot;
use crate::thread;

pub(crate) fn plan_path(project: &Project) -> PathBuf {
    project.record_file("plan.toml")
}

/// The plan writer lock, `<project>/.state/plan.lock`. Separate from the
/// project lock so a refresh from a thread or review mutation cannot deadlock.
fn plan_lock(project: &Project) -> Result<File> {
    crate::project::lock_file(&project.state_dir().join("plan.lock"))
}

// Binding history and its reason travel in the same atomic write as the card.
// Old task.plan_step and direct-thread records are never rewritten.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct PlanHistory {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    binding_changes: Vec<BindingChange>,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct BindingChange {
    step: String,
    bindings: Vec<String>,
    unlink: bool,
    why: String,
}

pub(crate) fn load(project: &Project) -> Result<Option<Plan>> {
    read_plan(project)
}

pub(crate) fn binding_changes(project: &Project) -> Result<Vec<BindingChange>> {
    Ok(read_plan::<PlanHistory>(project)?
        .unwrap_or_default()
        .binding_changes)
}

fn read_plan<T: serde::de::DeserializeOwned>(project: &Project) -> Result<Option<T>> {
    let path = plan_path(project);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let read = || -> Result<T> {
                let plan: Plan = toml::from_str(&text)?;
                if plan.schema != 1 {
                    bail!("expected plan schema 1, got {}", plan.schema);
                }
                toml::from_str::<PlanHistory>(&text)?;
                Ok(toml::from_str(&text)?)
            };
            Ok(Some(read().with_context(|| {
                format!("unreadable record: {}", path.display())
            })?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("unreadable record: {}", path.display())),
    }
}

/// A locked write to a temporary file, flushed, atomically renamed and the
/// parent directory flushed (SPEC-talk §6.5).
#[cfg(test)]
fn write(project: &Project, plan: &Plan) -> Result<()> {
    write_with_history(project, plan, binding_changes(project)?)
}

fn write_with_history(
    project: &Project,
    plan: &Plan,
    binding_changes: Vec<BindingChange>,
) -> Result<()> {
    let text = toml::to_string(plan)? + &toml::to_string(&PlanHistory { binding_changes })?;
    let path = project.record_file("plan.toml");
    write_atomic(&path, text.as_bytes())
}

fn step_id_ok(id: &str) -> bool {
    let digits = id.strip_prefix("s-").unwrap_or("");
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

fn dedup(values: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .iter()
        .filter(|value| seen.insert(*value))
        .cloned()
        .collect()
}

fn check_task_refs(project: &Project, tasks: &[String]) -> Result<()> {
    for id in tasks {
        crate::task::load(project, id)
            .with_context(|| format!("plan_ref: no task `{id}` in this project"))?;
    }
    Ok(())
}

/// Checks the candidate card's schema, identifiers and dependencies.
fn validate(plan: &Plan) -> Result<()> {
    if plan.schema != 1 {
        bail!("plan_schema: expected schema 1, got {}", plan.schema);
    }
    let mut ids = BTreeSet::new();
    for step in all_steps(plan) {
        if !step_id_ok(&step.id) {
            bail!(
                "plan_step_id: `{}` is not a step id (expected s-1)",
                step.id
            );
        }
        if !ids.insert(step.id.clone()) {
            bail!("plan_step_id: duplicate step id `{}`", step.id);
        }
        for t in &step.threads {
            thread::validate_id(t)?;
        }
    }
    for sub in plan.steps.iter().flat_map(|step| &step.subtasks) {
        if !sub.subtasks.is_empty() {
            bail!(
                "plan_step_depth: `{}` is a subtask; a subtask cannot have subtasks",
                sub.id
            );
        }
    }
    for step in all_steps(plan) {
        for prerequisite in &step.after {
            if !all_steps(plan).any(|s| &s.id == prerequisite) {
                bail!("plan_after_unknown: `{prerequisite}` is not a step of this plan");
            }
            if prerequisite == &step.id {
                bail!("plan_after_self: `{}` cannot wait on itself", step.id);
            }
            if plan.steps.iter().any(|parent| {
                (parent.id == step.id && parent.subtasks.iter().any(|s| &s.id == prerequisite))
                    || (parent.id == *prerequisite
                        && parent.subtasks.iter().any(|s| s.id == step.id))
            }) {
                bail!(
                    "plan_after_family: `{}` cannot wait on `{prerequisite}`",
                    step.id
                );
            }
            if reaches(plan, prerequisite, &step.id, &mut BTreeSet::new()) {
                bail!(
                    "plan_after_cycle: `{}` and `{prerequisite}` form a cycle",
                    step.id
                );
            }
        }
    }
    Ok(())
}

fn reaches(plan: &Plan, from: &str, target: &str, seen: &mut BTreeSet<String>) -> bool {
    if from == target {
        return true;
    }
    if !seen.insert(from.to_string()) {
        return false;
    }
    // A parent cannot finish before every child finishes. Treat that as an
    // implicit edge when checking for prerequisite cycles.
    all_steps(plan).find(|s| s.id == from).is_some_and(|s| {
        s.after
            .iter()
            .chain(s.subtasks.iter().map(|child| &child.id))
            .any(|id| reaches(plan, id, target, seen))
    })
}

/// One revision-guarded, validated, atomic mutation.
fn with_plan<T>(
    project: &Project,
    expect: impl Into<Option<u64>>,
    change: impl FnOnce(&mut Plan, &mut EvidenceSnapshot) -> Result<T>,
) -> Result<(Plan, T)> {
    let _lock = plan_lock(project)?;
    let mut plan = load(project)?.unwrap_or_default();
    let before = plan.clone();
    let mut evidence = crate::task::EvidenceSnapshot::load(project);
    let history_before = evidence.binding_changes.clone();
    if expect
        .into()
        .is_some_and(|revision| plan.revision != revision)
    {
        bail!(
            "plan_revision_stale: the plan is at revision {}; pass --expect {}",
            plan.revision,
            plan.revision
        );
    }
    if plan.schema == 0 {
        plan.schema = 1;
    }
    let extra = change(&mut plan, &mut evidence)?;
    // Every plan mutation also refreshes the persisted projection. In
    // particular, adding or removing a binding must not leave a stale state
    // until a later `plan sync`.
    evaluate(project, &plan, &evidence).project(&mut plan);
    validate(&plan)?;
    if plan != before || evidence.binding_changes != history_before {
        plan.revision += 1;
        write_with_history(project, &plan, evidence.binding_changes)?;
    }
    crate::project::refresh_page(project)?;
    Ok((plan, extra))
}

fn project_goal(project: &Project) -> String {
    project
        .read_project_md()
        .map(|(settings, _)| settings.goal)
        .unwrap_or_default()
}

/// `ha plan set --does "<outcome>" [--expect <revision>]`.
/// Preserves the steps and refreshes the goal from `PROJECT.md`.
pub(crate) fn set(
    ctx: &Ctx,
    slug: &str,
    does: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    let goal = project_goal(&project);
    let (plan, ()) = with_plan(&project, expect, |plan, _| {
        plan.kind.clear();
        plan.what_you_get.clear();
        plan.does = does.trim().into();
        plan.goal = goal.clone();
        Ok(())
    })?;
    Ok(plan)
}

pub(crate) fn step_add(
    ctx: &Ctx,
    slug: &str,
    text: &str,
    tasks: Vec<String>,
    after: Vec<String>,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    check_task_refs(&project, &tasks)?;
    let (plan, ()) = with_plan(&project, expect, |plan, _| {
        let id = next_id(plan);
        plan.steps.push(PlanStep {
            id,
            text: text.trim().into(),
            state: StepState::Left,
            tasks: dedup(&tasks),
            after: dedup(&after),
            ..PlanStep::default()
        });
        Ok(())
    })?;
    Ok(plan)
}

/// A fresh step id from the one counter steps and subtasks share.
fn next_id(plan: &mut Plan) -> String {
    if plan.next_step == 0 {
        // An old card without `next_step`: never reuse a live id.
        plan.next_step = all_steps(plan)
            .filter_map(|s| s.id.strip_prefix("s-")?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
    }
    let id = format!("s-{}", plan.next_step);
    plan.next_step += 1;
    id
}

/// Every step and subtask, each step before its subtasks.
pub(crate) fn all_steps(plan: &Plan) -> impl Iterator<Item = &PlanStep> {
    plan.steps
        .iter()
        .flat_map(|s| std::iter::once(s).chain(s.subtasks.iter()))
}

/// `ha plan step add <project> "<text>" --under <step>`: a subtask under a
/// top-level step. Returns the plan and the new subtask's id.
pub(crate) fn subtask_add(
    ctx: &Ctx,
    slug: &str,
    under: &str,
    text: &str,
    tasks: Vec<String>,
    after: Vec<String>,
    expect: impl Into<Option<u64>>,
) -> Result<(Plan, String)> {
    let project = Project::load(&ctx.root, slug)?;
    check_task_refs(&project, &tasks)?;
    with_plan(&project, expect, |plan, _| {
        let Some(at) = plan.steps.iter().position(|s| s.id == under) else {
            if all_steps(plan).any(|s| s.id == under) {
                return Err(crate::refusal::error(
                    format!(
                        "plan_step_depth: `{under}` is a subtask; a subtask cannot have subtasks"
                    ),
                    format!("ha plan show {slug}"),
                ));
            }
            return Err(crate::refusal::error(
                format!("plan_step_unknown: `{under}` is not a step of this plan"),
                format!("ha plan show {slug}"),
            ));
        };
        let id = next_id(plan);
        plan.steps[at].subtasks.push(PlanStep {
            id: id.clone(),
            text: text.trim().into(),
            state: StepState::Left,
            tasks: dedup(&tasks),
            after: dedup(&after),
            ..PlanStep::default()
        });
        Ok(id)
    })
}

pub(crate) fn step_edit(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    text: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    let (plan, ()) = with_plan(&project, expect, |plan, _| {
        let step = find_step(plan, id)?;
        step.text = text.trim().into();
        Ok(())
    })?;
    Ok(plan)
}

pub(crate) fn step_link(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    tasks: Vec<String>,
    after: Vec<String>,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    if tasks.is_empty() && after.is_empty() {
        return Err(crate::refusal::error(
            "plan_link: at least one --task or --after is required",
            format!("ha plan step link {slug} {id} --after <step-id>"),
        ));
    }
    check_task_refs(&project, &tasks)?;
    let (plan, ()) = with_plan(&project, expect, |plan, evidence| {
        let changes = &mut evidence.binding_changes;
        let step = find_step(plan, id)?;
        if tasks.iter().any(|task| is_unlinked(changes, id, task)) {
            changes.push(BindingChange {
                step: id.into(),
                bindings: tasks.clone(),
                unlink: false,
                why: String::new(),
            });
        }
        for (bound, added) in [(&mut step.tasks, tasks), (&mut step.after, after)] {
            for id in added {
                if !bound.contains(&id) {
                    bound.push(id);
                }
            }
        }
        Ok(())
    })?;
    Ok(plan)
}

pub(crate) fn step_unlink(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    tasks: Vec<String>,
    after: Vec<String>,
    why: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    if tasks.is_empty() && after.is_empty() {
        return Err(crate::refusal::error(
            "plan_unlink: at least one --task or --after is required",
            format!("ha plan step unlink {slug} {id} --after <step-id> --reason \"<reason>\""),
        ));
    }
    let (plan, ()) = with_plan(&project, expect, |plan, evidence| {
        let step = find_step(plan, id)?;
        let (bound_tasks, bound_threads) = bindings(step, evidence);
        let attempts: BTreeSet<_> = evidence
            .tasks
            .iter()
            .filter(|task| tasks.contains(&task.id))
            .flat_map(|task| &task.attempts)
            .collect();
        let removed: Vec<_> = bound_tasks
            .into_iter()
            .filter(|id| tasks.contains(id))
            .chain(bound_threads.into_iter().filter(|id| attempts.contains(id)))
            .chain(step.after.iter().filter(|id| after.contains(id)).cloned())
            .collect();
        step.tasks.retain(|task| !tasks.contains(task));
        step.after.retain(|edge| !after.contains(edge));
        if !removed.is_empty() {
            evidence.binding_changes.push(BindingChange {
                step: id.into(),
                bindings: removed,
                unlink: true,
                why: why.into(),
            });
        }
        Ok(())
    })?;
    Ok(plan)
}

pub(crate) fn step_remove(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    let (plan, ()) = with_plan(&project, expect, |plan, _| {
        let removed: Vec<_> = plan
            .steps
            .iter()
            .filter(|step| step.id == id)
            .flat_map(|step| step.subtasks.iter().map(|sub| sub.id.as_str()))
            .chain(std::iter::once(id))
            .collect();
        if let Some((dependent, edge)) = all_steps(plan)
            .filter(|step| !removed.contains(&step.id.as_str()))
            .find_map(|step| {
                step.after
                    .iter()
                    .find(|edge| removed.contains(&edge.as_str()))
                    .map(|edge| (step.id.as_str(), edge.as_str()))
            })
        {
            bail!(
                "plan_after_referenced: step `{edge}` is required by `{dependent}`; unlink it first"
            );
        }
        let before = all_steps(plan).count();
        plan.steps.retain(|s| s.id != id);
        for step in &mut plan.steps {
            step.subtasks.retain(|s| s.id != id);
        }
        if all_steps(plan).count() == before {
            return Err(crate::refusal::error(
                format!("plan_step_unknown: `{id}` is not a step of this plan"),
                format!("ha plan show {slug}"),
            ));
        }
        Ok(())
    })?;
    Ok(plan)
}

/// `move` changes display order only. `before` names the step it goes in
/// front of; an empty `before` sends it to the end.
pub(crate) fn step_move(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    before: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    let (plan, ()) = with_plan(&project, expect, |plan, _| {
        let from = plan
            .steps
            .iter()
            .position(|s| s.id == id)
            .with_context(|| format!("plan_step_unknown: `{id}` is not a step of this plan"))?;
        let step = plan.steps.remove(from);
        let to = if before.is_empty() {
            plan.steps.len()
        } else {
            plan.steps
                .iter()
                .position(|s| s.id == before)
                .with_context(|| {
                    format!("plan_step_unknown: `{before}` is not a step of this plan")
                })?
        };
        plan.steps.insert(to, step);
        Ok(())
    })?;
    Ok(plan)
}

fn find_step<'a>(plan: &'a mut Plan, id: &str) -> Result<&'a mut PlanStep> {
    if let Some(at) = plan.steps.iter().position(|s| s.id == id) {
        return Ok(&mut plan.steps[at]);
    }
    plan.steps
        .iter_mut()
        .flat_map(|s| s.subtasks.iter_mut())
        .find(|s| s.id == id)
        .with_context(|| format!("plan_step_unknown: `{id}` is not a step of this plan"))
}

/// The same read-only projection as `show`, without exposing project prose.
pub(crate) fn counts(project: &Project) -> Result<(usize, usize)> {
    let Some(mut plan) = load(project)? else {
        return Ok((0, 0));
    };
    project_states(project, &mut plan);
    Ok((
        all_steps(&plan)
            .filter(|step| step.state == StepState::Done)
            .count(),
        all_steps(&plan).count(),
    ))
}

/// One evidence snapshot for both the Rundown `data.result` and human view.
pub(crate) struct Show {
    plan: Plan,
    present: bool,
    goal: String,
    evaluation: Evaluation,
}

pub(crate) fn show(ctx: &Ctx, slug: &str) -> Result<Show> {
    let project = Project::load(&ctx.root, slug)?;
    let loaded = load(&project)?;
    let present = loaded.is_some();
    let goal = project_goal(&project);
    let mut plan = loaded.unwrap_or_else(|| Plan {
        schema: 1,
        goal: goal.clone(),
        ..Plan::default()
    });
    let evidence = crate::task::EvidenceSnapshot::load(&project);
    let evaluation = evaluate(&project, &plan, &evidence);
    evaluation.project(&mut plan);
    Ok(Show {
        plan,
        present,
        goal,
        evaluation,
    })
}

impl serde::Serialize for Show {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let mut value = serde_json::to_value(&self.plan).map_err(S::Error::custom)?;
        value["present"] = self.present.into();
        for step in value["steps"].as_array_mut().into_iter().flatten() {
            add_hold_json(step, &self.evaluation.holds);
            if let Some(subtasks) = step
                .get_mut("subtasks")
                .and_then(|value| value.as_array_mut())
            {
                for sub in subtasks {
                    add_hold_json(sub, &self.evaluation.holds);
                }
            }
        }
        serde::Serialize::serialize(&value, serializer)
    }
}

impl Show {
    pub(crate) fn message(&self) -> String {
        if !self.present {
            return "no plan is written down yet (revision 0)\n".into();
        }
        let plan = &self.plan;
        let mut out = format!("revision {}\n", plan.revision);
        if plan.goal != self.goal {
            out.push_str("goal: differs from PROJECT.md; run `plan set` to copy it again\n");
        }
        out.push_str(&format!("goal: {}\n", plan.goal));
        let outcome = format!("{} {}", plan.what_you_get, plan.does);
        let outcome = outcome.trim();
        out.push_str(&format!(
            "what you get at the end: {}\n",
            if outcome.is_empty() {
                "not written down yet"
            } else {
                outcome
            }
        ));
        out.push_str("steps:\n");
        if plan.steps.is_empty() {
            out.push_str("  (none)\n");
        }
        for (indent, step) in plan.steps.iter().flat_map(|s| {
            std::iter::once(("  ", s)).chain(s.subtasks.iter().map(|sub| ("      ", sub)))
        }) {
            let mut refs = String::new();
            let evaluated = &self.evaluation.steps[&step.id];
            for (label, ids) in [
                ("tasks", &evaluated.tasks),
                ("threads", &evaluated.threads),
                ("after", &step.after),
            ] {
                if !ids.is_empty() {
                    refs.push_str(&format!(" {label} {}", ids.join(", ")));
                }
            }
            out.push_str(&format!(
                "{indent}{:<7} {}  {}{}\n",
                step.state.word(),
                step.id,
                step.text,
                refs
            ));
            if let Some(hold) = self.evaluation.holds.get(&step.id) {
                out.push_str(&format!("{indent}  {}\n", hold.message()));
            }
        }
        out
    }
}

/// Check current evidence, never the persisted state, before a task launches.
pub(crate) fn check_prerequisites(project: &Project, job: &str) -> Result<()> {
    if job.is_empty() {
        return Ok(());
    }
    let Some(mut plan) = load(project)? else {
        return Ok(());
    };
    if !all_steps(&plan).any(|step| !step.after.is_empty()) {
        return Ok(());
    }
    crate::task::load(project, job)?;
    let evidence = crate::task::EvidenceSnapshot::load(project);
    let evaluation = evaluate(project, &plan, &evidence);
    evaluation.project(&mut plan);
    let readable = evidence.tasks_readable() && evidence.readable();
    for dependent in
        all_steps(&plan).filter(|step| evaluation.steps[&step.id].tasks.iter().any(|id| id == job))
    {
        for id in &dependent.after {
            let prerequisite = all_steps(&plan)
                .find(|step| &step.id == id)
                .with_context(|| {
                    format!("plan_after_unknown: `{id}` is not a step of this plan")
                })?;
            if prerequisite.state != StepState::Done || !readable {
                let state = if !readable && prerequisite.state == StepState::Done {
                    StepState::Running
                } else {
                    prerequisite.state
                };
                if let Some(check) = evaluation.steps[id].checks.first() {
                    let reason = check.diagnostic.as_deref().unwrap_or("verdict FAIL");
                    bail!(
                        "plan_prerequisite: {job} cannot start; step {} waits for {} (check failed: {} {reason}). Send the work back with ha thread prompt and get a fresh verdict, or change the plan.",
                        dependent.id,
                        id,
                        check.lane_id
                    );
                }
                bail!(
                    "plan_prerequisite: {job} cannot start; step {} waits for {} ({}). Finish the bound work and run ha plan sync {}, or change the plan before starting.",
                    dependent.id,
                    id,
                    state.word(),
                    project.slug
                );
            }
            require_step_acceptance(project, prerequisite, &evidence, &evaluation)?;
        }
    }
    Ok(())
}

/// Completion counts remain finish/delivery facts. Launching work that relies
/// on a result additionally needs its criterion judgment; partial research is
/// not an accepted prerequisite merely because its plan step says done.
fn require_step_acceptance(
    project: &Project,
    step: &PlanStep,
    evidence: &EvidenceSnapshot,
    evaluation: &Evaluation,
) -> Result<()> {
    if !evidence.tasks_readable() {
        bail!("plan_prerequisite: acceptance not established: unreadable tasks");
    }
    let evaluated = &evaluation.steps[&step.id];
    let bound: Vec<_> = evidence
        .tasks
        .iter()
        .filter(|task| {
            task.dropped.is_empty()
                && (evaluated.tasks.contains(&task.id)
                    || task
                        .attempts
                        .last()
                        .is_some_and(|id| evaluated.threads.contains(id)))
        })
        .collect();
    for task in &bound {
        crate::task::require_accepted(project, task, evidence)
            .map_err(|error| anyhow::anyhow!("plan_prerequisite: step {}: {error}", step.id))?;
    }
    for id in &evaluated.threads {
        if bound.iter().any(|task| task.attempts.last() == Some(id)) {
            continue;
        }
        let lane = evidence
            .lanes
            .get(id)
            .context("plan_prerequisite: missing thread")?;
        if lane.merged_sha.is_empty() || !lane.merged_review.is_empty() {
            bail!(
                "plan_prerequisite: acceptance not established: {id} needs a request-backed task and criterion evidence"
            );
        }
    }
    for sub in &step.subtasks {
        require_step_acceptance(project, sub, evidence, evaluation)?;
    }
    Ok(())
}

/// Find the stable task of a placed attempt for a deferred launch.
pub(crate) fn check_attempt_prerequisites(project: &Project, thread_id: &str) -> Result<()> {
    let (tasks, errors) = crate::task::list_with_errors(project);
    if !errors.is_empty()
        && load(project)?.is_some_and(|plan| all_steps(&plan).any(|s| !s.after.is_empty()))
    {
        bail!(
            "plan_prerequisite: unreadable task evidence; repair it before launching {thread_id}"
        );
    }
    for task in tasks {
        if task.attempts.iter().any(|id| id == thread_id) {
            check_prerequisites(project, &task.id)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncOutcome {
    Missing,
    Unchanged {
        revision: u64,
        holds: BTreeMap<String, FailedCheckHold>,
    },
    Changed {
        revision: u64,
    },
}

/// `ha plan sync`: derive states from the bound work records and write only
/// on change.
pub(crate) fn sync(ctx: &Ctx, slug: &str) -> Result<SyncOutcome> {
    refresh_project(&Project::load(&ctx.root, slug)?)
}

fn refresh_project(project: &Project) -> Result<SyncOutcome> {
    let _lock = plan_lock(project)?;
    let Some(mut plan) = load(project)? else {
        crate::project::refresh_page(project)?;
        return Ok(SyncOutcome::Missing);
    };
    let evidence = crate::task::EvidenceSnapshot::load(project);
    let evaluation = evaluate(project, &plan, &evidence);
    if !evaluation.project(&mut plan) {
        crate::project::refresh_page(project)?;
        return Ok(SyncOutcome::Unchanged {
            revision: plan.revision,
            holds: evaluation.holds,
        });
    }
    plan.revision += 1;
    write_with_history(project, &plan, evidence.binding_changes)?;
    crate::project::refresh_page(project)?;
    Ok(SyncOutcome::Changed {
        revision: plan.revision,
    })
}

/// The shared projection refresh (SPEC-talk §6.5). Reads the current card and
/// work records and writes only on change; a missing card is a no-op. Callers
/// treat a failure as a separate refresh failure, never a merge failure.
pub(crate) fn refresh(_ctx: &Ctx, project: &Project) -> Result<bool> {
    Ok(matches!(
        refresh_project(project)?,
        SyncOutcome::Changed { .. }
    ))
}

/// Flips every step's persisted state to the state its bindings derive.
pub(crate) fn project_states(project: &Project, plan: &mut Plan) -> bool {
    evaluate(project, plan, &crate::task::EvidenceSnapshot::load(project)).project(plan)
}

#[derive(Default)]
struct Evaluation {
    steps: BTreeMap<String, StepEvaluation>,
    holds: BTreeMap<String, FailedCheckHold>,
}
struct StepEvaluation {
    state: StepState,
    tasks: Vec<String>,
    threads: Vec<String>,
    checks: Vec<HoldingCheck>,
    terminal: bool,
}
impl Evaluation {
    fn project(&self, plan: &mut Plan) -> bool {
        let mut changed = false;
        for step in plan.steps.iter_mut().flat_map(|s| {
            std::iter::once((&mut s.state, &s.id))
                .chain(s.subtasks.iter_mut().map(|s| (&mut s.state, &s.id)))
        }) {
            let state = self.steps[step.1].state;
            changed |= *step.0 != state;
            *step.0 = state;
        }
        changed
    }
}

fn is_unlinked(changes: &[BindingChange], step: &str, id: &str) -> bool {
    changes
        .iter()
        .rev()
        .find(|change| change.step == step && change.bindings.iter().any(|binding| binding == id))
        .is_some_and(|change| change.unlink)
}

/// Normalize in memory; the original records and reasoned binding history stay.
pub(crate) fn bindings(step: &PlanStep, evidence: &EvidenceSnapshot) -> (Vec<String>, Vec<String>) {
    let allowed = |id: &String| !is_unlinked(&evidence.binding_changes, &step.id, id);
    let mut tasks: BTreeSet<_> = step.tasks.iter().cloned().collect();
    tasks.extend(
        evidence
            .tasks
            .iter()
            .filter(|task| task.plan_step.as_deref() == Some(&step.id))
            .map(|task| task.id.clone()),
    );
    tasks.retain(allowed);
    let threads = step
        .threads
        .iter()
        .filter(|id| allowed(id))
        .cloned()
        .collect();
    (tasks.into_iter().collect(), threads)
}

/// One bottom-up pass produces states, normalized bindings and failed-check
/// explanations together. Work completion and critic verdicts are read once.
fn evaluate(project: &Project, plan: &Plan, evidence: &EvidenceSnapshot) -> Evaluation {
    let mut work = BTreeMap::new();
    for lane in evidence.lanes.values() {
        let check = critic_check(project, lane, evidence).map(|mut check| {
            check.task_id = evidence
                .tasks
                .iter()
                .find(|task| task.attempts.last() == Some(&lane.id))
                .map(|task| task.id.clone());
            check
        });
        work.insert(
            lane.id.clone(),
            BoundWork {
                terminal: evidence.lane_done(lane),
                started: true,
                check,
                dropped: false,
            },
        );
    }
    for task in &evidence.tasks {
        let view = crate::task::view_with_evidence(project, task.clone(), evidence);
        let attempt = task.attempts.last().and_then(|id| work.get(id));
        let check = attempt
            .and_then(|work| work.check.clone())
            .map(|mut check| {
                check.task_id = Some(task.id.clone());
                check
            });
        work.insert(
            task.id.clone(),
            BoundWork {
                terminal: view.state == crate::task::State::Installed
                    || attempt.is_some_and(|work| work.terminal),
                started: view.state != crate::task::State::Open,
                check,
                dropped: !task.dropped.is_empty(),
            },
        );
    }
    let mut evaluation = Evaluation::default();
    for step in &plan.steps {
        evaluate_step(step, evidence, &work, &mut evaluation);
    }
    evaluation
}

#[derive(Default)]
struct BoundWork {
    terminal: bool,
    started: bool,
    check: Option<HoldingCheck>,
    dropped: bool,
}

fn evaluate_step(
    step: &PlanStep,
    evidence: &EvidenceSnapshot,
    work: &BTreeMap<String, BoundWork>,
    evaluation: &mut Evaluation,
) {
    for sub in &step.subtasks {
        evaluate_step(sub, evidence, work, evaluation);
    }
    let (tasks, threads) = bindings(step, evidence);
    let missing = BoundWork::default();
    let own: Vec<_> = tasks
        .iter()
        .chain(&threads)
        .map(|id| work.get(id).unwrap_or(&missing))
        .filter(|work| !work.dropped)
        .collect();
    let own_work = !own.is_empty();
    let mut terminal = own.iter().all(|work| work.terminal);
    let mut checks: Vec<_> = own.iter().filter_map(|work| work.check.clone()).collect();
    let own = if own_work && terminal && checks.is_empty() {
        StepState::Done
    } else if own.iter().any(|work| work.started) {
        StepState::Running
    } else {
        StepState::Left
    };
    let children: Vec<_> = step
        .subtasks
        .iter()
        .map(|sub| &evaluation.steps[&sub.id])
        .collect();
    let state = if children.is_empty() {
        own
    } else if (!own_work || own == StepState::Done)
        && children.iter().all(|sub| sub.state == StepState::Done)
    {
        StepState::Done
    } else if own != StepState::Left || children.iter().any(|sub| sub.state != StepState::Left) {
        StepState::Running
    } else {
        StepState::Left
    };
    terminal &= (own_work || !children.is_empty()) && children.iter().all(|sub| sub.terminal);
    for child in children {
        checks.extend(child.checks.clone());
    }
    let mut seen = BTreeSet::new();
    checks.retain(|check| seen.insert((check.lane_id.clone(), check.task_id.clone())));
    if state == StepState::Running
        && terminal
        && !checks.is_empty()
        && evidence.readable()
        && evidence.tasks_readable()
    {
        evaluation.holds.insert(step.id.clone(), FailedCheckHold {
            checks: checks.clone(), next: "get a fresh critic verdict (re-check, sealed PASS), or unlink the check task with a reason".into(),
        });
    }
    evaluation.steps.insert(
        step.id.clone(),
        StepEvaluation {
            state,
            tasks,
            threads,
            checks,
            terminal,
        },
    );
}

/// A display-only explanation: no new persisted state or completion rule.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct HoldingCheck {
    pub(crate) lane_id: String,
    pub(crate) task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct FailedCheckHold {
    pub(crate) checks: Vec<HoldingCheck>,
    pub(crate) next: String,
}

impl FailedCheckHold {
    pub(crate) fn message(&self) -> String {
        let checks = self
            .checks
            .iter()
            .map(|check| {
                let name = match &check.task_id {
                    Some(task) => format!("{} ({task})", check.lane_id),
                    None => check.lane_id.clone(),
                };
                match &check.diagnostic {
                    Some(reason) => format!("{name}: {reason}"),
                    None => name,
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("held by failed check {checks}; {}", self.next)
    }
}

pub(crate) fn add_hold_json(
    value: &mut serde_json::Value,
    holds: &BTreeMap<String, FailedCheckHold>,
) {
    if let Some(hold) = value["id"].as_str().and_then(|id| holds.get(id)) {
        value["failed_check_hold"] = serde_json::to_value(hold).expect("serializable hold");
        value["failed_check_hold"]["message"] = hold.message().into();
    }
}

/// Explain only steps whose work is otherwise satisfied. Reuse the same
/// terminal evidence as derivation, including missing bindings and children.
pub(crate) fn failed_check_holds(
    project: &Project,
    plan: &mut Plan,
    evidence: &EvidenceSnapshot,
) -> BTreeMap<String, FailedCheckHold> {
    let evaluation = evaluate(project, plan, evidence);
    evaluation.project(plan);
    evaluation.holds
}

fn critic_check(
    project: &Project,
    lane: &thread::Thread,
    evidence: &EvidenceSnapshot,
) -> Option<HoldingCheck> {
    if lane.role != "critic" {
        return None;
    }
    // Keep the latest sealed check blocking through a follow-up. Unreadable
    // evidence is not a passing check; readable historical prose still counts.
    let done = crate::events::latest_done_event(evidence.events(), &lane.id, lane.attempt.max(1))?
        .payload
        .done
        .as_ref()?;
    let report = thread::artifact(project, &done.artifact)
        .and_then(|bytes| String::from_utf8(bytes).context("critic report is invalid UTF-8"));
    let diagnostic = match report {
        Ok(text) if crate::lane::critic_verdict(&text).as_deref() == Some("FAIL") => None,
        Ok(_) => return None,
        Err(error) => Some(format!("unreadable critic evidence: {error:#}")),
    };
    Some(HoldingCheck {
        lane_id: lane.id.clone(),
        task_id: None,
        diagnostic,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{Fx, fixture};

    fn add(fx: &Fx, text: &str, expect: u64) -> Plan {
        step_add(&fx.world.ctx(), "demo", text, vec![], vec![], expect).unwrap()
    }

    #[test]
    fn empty_existing_plan_is_unreadable_on_all_count_surfaces() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        assert!(load(&fx.project).unwrap().is_none());
        for text in ["", "revision = 1\n", "schema = 1\n", "goal = 'Goal'\n"] {
            std::fs::write(plan_path(&fx.project), text).unwrap();
            let error = format!("{:#}", load(&fx.project).unwrap_err());
            assert!(error.contains("unreadable record") && error.contains("plan.toml"));
            assert!(counts(&fx.project).is_err());
            assert!(show(&ctx, "demo").is_err());
            assert!(binding_changes(&fx.project).is_err());
            let view = crate::project_view::View::load(&ctx, &fx.project, None).unwrap();
            let overview = view.rundown();
            assert!(
                overview["read_error"]
                    .as_str()
                    .unwrap()
                    .contains("unreadable record")
            );
            assert!(view.render(&[]).contains("unreadable record"));
            assert!(view.render(&["Current work"]).contains("unreadable record"));
        }
    }

    #[test]
    fn show_renders_one_snapshot_and_preserves_the_missing_card_contract() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let missing = show(&ctx, "demo").unwrap();
        assert_eq!(
            serde_json::to_value(&missing).unwrap(),
            serde_json::json!({
                "present": false, "schema": 1, "revision": 0, "next_step": 0,
                "goal": project_goal(&fx.project), "does": "", "steps": []
            })
        );
        assert_eq!(
            missing.message(),
            "no plan is written down yet (revision 0)\n"
        );
        set(&ctx, "demo", "First outcome", None).unwrap();
        add(&fx, "First step", 1);
        let snapshot = show(&ctx, "demo").unwrap();
        let message = snapshot.message();
        let data = serde_json::to_value(&snapshot).unwrap();
        set(&ctx, "demo", "New outcome", None).unwrap();
        step_edit(&ctx, "demo", "s-1", "New step", None).unwrap();
        assert_eq!(snapshot.message(), message);
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), data);
        assert!(message.contains("First outcome") && message.contains("First step"));
        assert_eq!(data["steps"][0]["text"], "First step");
        assert_eq!(data["does"], "First outcome");
    }

    #[test]
    fn omitted_expect_uses_locked_revision_and_json_shows_it() {
        let fx = fixture();
        set(&fx.world.ctx(), "demo", "It shows the result.", None).unwrap();
        let plan = step_add(
            &fx.world.ctx(),
            "demo",
            "Build the screen.",
            vec![],
            vec![],
            None,
        )
        .unwrap();
        assert_eq!(plan.revision, 2);
        let json: serde_json::Value =
            serde_json::to_value(show(&fx.world.ctx(), "demo").unwrap()).unwrap();
        assert_eq!(json["revision"], 2);
        assert!(set(&fx.world.ctx(), "demo", "It runs the task.", Some(0)).is_err());
    }

    #[test]
    fn a_stale_expect_fails_unchanged_and_cannot_break_the_old_file() {
        let fx = fixture();
        set(&fx.world.ctx(), "demo", "It shows pretend trades.", 0).unwrap();
        let before = std::fs::read_to_string(plan_path(&fx.project)).unwrap();
        let e = format!(
            "{:#}",
            set(&fx.world.ctx(), "demo", "It prints the lines.", 9).unwrap_err()
        );
        assert!(e.starts_with("plan_revision_stale"), "{e}");
        assert_eq!(
            std::fs::read_to_string(plan_path(&fx.project)).unwrap(),
            before
        );
    }

    fn link_historical_threads(fx: &Fx, threads: Vec<String>, expect: u64) -> Plan {
        with_plan(&fx.project, expect, |plan, _| {
            find_step(plan, "s-1")?.threads = threads;
            Ok(())
        })
        .unwrap()
        .0
    }

    #[test]
    fn one_of_two_required_threads_is_not_done() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        set(&ctx, "demo", "It shows pretend trades.", 0).unwrap();
        let (a, _) = fx.lane(1);
        let (b, _) = fx.lane(2);
        add(&fx, "Needs two lanes.", 1);
        link_historical_threads(&fx, vec![a, b], 2);
        let _ = sync(&ctx, "demo").unwrap();
        assert_eq!(
            load(&fx.project).unwrap().unwrap().steps[0].state,
            StepState::Running
        );
    }

    fn write_task(fx: &Fx, id: &str) {
        let task = crate::task::Task {
            id: id.into(),
            title: "Ship the checked change.".into(),
            authority: vec!["request:q-1".into()],
            acceptance: vec!["The change lands.".into()],
            created: "2026-09-22T00:00:00Z".into(),
            ..crate::task::Task::default()
        };
        let task_dir = fx.project.state_dir().join("tasks");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(
            task_dir.join(format!("{id}.toml")),
            toml::to_string(&task).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn binding_shapes_pin_current_step_states() {
        let fx = fixture();
        let mut tasks = Vec::new();
        for n in 1..=7 {
            let id = format!("job-{n:04}");
            write_task(&fx, &id);
            tasks.push(crate::task::load(&fx.project, &id).unwrap());
        }
        // Explicit installed evidence needs neither a lane nor a seal.
        tasks[0].installed.push(crate::task::Evidence {
            at: project_goal(&fx.project),
            command: "historical install".into(),
            acceptance: vec![],
            machine: None,
            build: None,
        });
        tasks[1] = tasks[0].clone();
        tasks[1].id = "job-0002".into();
        tasks[1].plan_step = Some("s-2".into());
        tasks[2].dropped.push(crate::task::DropEvidence {
            at: "2026-09-22T00:00:00Z".into(),
            reason: "not required".into(),
        });
        let historical = fx.thread("historical merged");
        thread::update(&fx.project, &historical, |t| {
            t.merged_sha = "old-sha".into()
        })
        .unwrap();
        let no_change = fx.thread("no change");
        let seal = fx.seal_done(&no_change, 1, 1, "old-sha", "research");
        thread::update(&fx.project, &no_change, |t| {
            t.changes_seal = seal.clone();
            t.has_changes = Some(false);
        })
        .unwrap();
        tasks[3].attempts = vec![no_change];
        let critic = fx.thread("failed critic");
        thread::update(&fx.project, &critic, |t| {
            t.role = "critic".into();
            t.merged_sha = "old-sha".into();
        })
        .unwrap();
        fx.seal_done(&critic, 1, 1, "old-sha", "+++\nverdict = \"FAIL\"\n+++\n");
        tasks[4].attempts = vec![critic.clone()];
        let awaiting_install = fx.thread("historical awaiting install");
        thread::update(&fx.project, &awaiting_install, |t| {
            t.merged_sha = "old-sha".into();
            t.historical_install_required = true;
        })
        .unwrap();
        tasks[5].attempts = vec![awaiting_install];
        for task in tasks {
            std::fs::write(
                fx.project
                    .state_dir()
                    .join("tasks")
                    .join(format!("{}.toml", task.id)),
                toml::to_string(&task).unwrap(),
            )
            .unwrap();
        }
        let step = |n: u32, tasks: &[&str], threads: Vec<String>| PlanStep {
            id: format!("s-{n}"),
            tasks: tasks.iter().map(|id| (*id).into()).collect(),
            threads,
            ..PlanStep::default()
        };
        let mut plan = Plan {
            schema: 1,
            steps: vec![
                step(1, &["job-0001"], vec![]),
                step(2, &[], vec![]),
                step(3, &[], vec![historical]),
                step(4, &["job-0003"], vec![]),
                step(5, &["job-0004"], vec![]),
                step(6, &["job-0005"], vec![]),
                step(7, &["job-9999"], vec![]),
                step(8, &["job-0001", "job-0003"], vec![]),
                step(9, &["job-0006"], vec![]),
                step(10, &["job-0007"], vec![]),
                step(11, &[], vec!["t-9999".into()]),
                step(12, &[], vec![]),
                step(14, &["job-9999"], vec![]),
                step(16, &[], vec![critic]),
            ],
            ..Plan::default()
        };
        plan.steps[11].subtasks = vec![step(13, &["job-0001"], vec![])];
        plan.steps[12].subtasks = vec![step(15, &["job-0001"], vec![])];
        write(&fx.project, &plan).unwrap();
        let mut loaded = load(&fx.project).unwrap().unwrap();
        project_states(&fx.project, &mut loaded);
        use StepState::{Done, Left, Running};
        assert_eq!(
            all_steps(&loaded).map(|s| s.state).collect::<Vec<_>>(),
            vec![
                Done, Done, Done, Left, Done, Running, Left, Done, Running, Left, Left, Done, Done,
                Running, Done, Running,
            ]
        );
        assert_eq!(all_steps(&loaded).filter(|s| s.state == Done).count(), 8);
    }

    #[test]
    fn installed_lane_evidence_keeps_task_done_without_a_retained_review() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        for id in ["job-0001", "job-0002"] {
            write_task(&fx, id);
        }
        let lane = fx.thread("installed work");
        thread::update(&fx.project, &lane, |t| {
            t.merged_sha = "old-sha".into();
            t.installed_sha = "old-sha".into();
            t.merged_review = "review-1".into();
        })
        .unwrap();
        crate::task::link_attempt(&fx.project, "job-0001", &lane).unwrap();
        let task = crate::task::load(&fx.project, "job-0001").unwrap();
        let evidence = EvidenceSnapshot::load(&fx.project);
        let view = crate::task::view_with_evidence(&fx.project, task.clone(), &evidence);
        assert_eq!(view.state, crate::task::State::Installed);
        assert!(view.terminal_with_evidence(&fx.project, &evidence));
        assert!(crate::task::require_accepted(&fx.project, &task, &evidence).is_err());
        step_add(&ctx, "demo", "Installed task", vec![task.id], vec![], None).unwrap();
        step_add(&ctx, "demo", "Direct thread", vec![], vec![], None).unwrap();
        with_plan(&fx.project, None, |plan, _| {
            plan.steps[1].threads = vec![lane];
            Ok(())
        })
        .unwrap();
        let shown = show(&ctx, "demo").unwrap();
        // Historical task installation is terminal; a direct lane still needs
        // its review. Neither completion shape invents criterion acceptance.
        assert_eq!(shown.plan.steps[0].state, StepState::Done);
        assert_eq!(shown.plan.steps[1].state, StepState::Running);
        step_add(
            &ctx,
            "demo",
            "Dependent work",
            vec!["job-0002".into()],
            vec!["s-1".into()],
            None,
        )
        .unwrap();
        assert!(
            check_prerequisites(&fx.project, "job-0002")
                .unwrap_err()
                .to_string()
                .contains("acceptance not established")
        );
    }

    #[test]
    fn authored_outcomes_replace_historical_prose_without_changing_steps() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let (lane, _) = fx.lane(1);
        add(&fx, "Compare red.md and blue.md", 0);
        let old = link_historical_threads(&fx, vec![lane], 1);
        let old = with_plan(&fx.project, old.revision, |plan, _| {
            plan.kind = "unlisted historical kind".into();
            plan.what_you_get = "Compare files (Rolf, 2026-09-25).".into();
            Ok(())
        })
        .unwrap()
        .0;
        assert!(
            show(&ctx, "demo")
                .unwrap()
                .message()
                .contains(&old.what_you_get)
        );
        let outcome =
            "Compare red.md and blue.md: show differences; keep names (Rolf, 2026-09-25).";
        let changed = set(&ctx, "demo", outcome, old.revision).unwrap();
        assert_eq!(changed.steps, old.steps);
        assert_eq!(changed.does, outcome);
        assert!(changed.kind.is_empty() && changed.what_you_get.is_empty());
        assert!(show(&ctx, "demo").unwrap().message().contains(outcome));
        assert_eq!(load(&fx.project).unwrap().unwrap(), changed);
    }

    #[test]
    fn a_plan_without_subtasks_reads_exactly_as_before() {
        // Schema 1 before subtasks and prerequisite edges existed.
        let plan: Plan = toml::from_str(
            "schema = 1\nrevision = 3\nnext_step = 3\ngoal = \"\"\nkind = \"screen\"\nwhat_you_get = \"A screen you open.\"\ndoes = \"It shows pretend trades.\"\n\n[[steps]]\nid = \"s-1\"\ntext = \"Build the screen\"\nstate = \"done\"\ntasks = []\nthreads = []\n\n[[steps]]\nid = \"s-2\"\ntext = \"Try it out\"\nstate = \"left\"\ntasks = []\nthreads = []\n",
        )
        .unwrap();
        assert_eq!(plan.revision, 3);
        assert_eq!(plan.kind, "screen");
        assert_eq!(plan.what_you_get, "A screen you open.");
        assert_eq!(plan.does, "It shows pretend trades.");
        assert_eq!(all_steps(&plan).count(), 2);
        assert_eq!(
            all_steps(&plan)
                .filter(|s| s.state == StepState::Done)
                .count(),
            1
        );
        assert!(
            plan.steps
                .iter()
                .all(|s| s.subtasks.is_empty() && s.after.is_empty())
        );
    }

    #[test]
    fn a_step_with_subtasks_is_done_when_every_subtask_is_done() {
        let fx = fixture();
        for id in ["job-0001", "job-0002", "job-0003"] {
            write_task(&fx, id);
        }
        let lane = fx.thread("working");
        crate::task::link_attempt(&fx.project, "job-0002", &lane).unwrap();
        let mut installed = crate::task::load(&fx.project, "job-0001").unwrap();
        installed.installed.push(crate::task::Evidence {
            at: "old".into(),
            command: "installed".into(),
            acceptance: vec![],
            machine: None,
            build: None,
        });
        std::fs::write(
            fx.project.state_dir().join("tasks/job-0001.toml"),
            toml::to_string(&installed).unwrap(),
        )
        .unwrap();
        let project = |parent: PlanStep| {
            let mut plan = Plan {
                steps: vec![parent],
                ..Plan::default()
            };
            project_states(&fx.project, &mut plan);
            plan.steps.remove(0).state
        };
        let step = |states: &[StepState]| PlanStep {
            id: "s-1".into(),
            subtasks: states
                .iter()
                .enumerate()
                .map(|(n, state)| PlanStep {
                    id: format!("s-{}", n + 2),
                    tasks: vec![
                        match state {
                            StepState::Done => "job-0001",
                            StepState::Running => "job-0002",
                            StepState::Left => "job-0003",
                        }
                        .into(),
                    ],
                    ..PlanStep::default()
                })
                .collect(),
            ..PlanStep::default()
        };
        use StepState::{Done, Left, Running};
        for (states, want) in [
            (&[Done, Done][..], Done),
            (&[Done, Left], Running),
            (&[Running, Left], Running),
            (&[Left, Left], Left),
        ] {
            assert_eq!(project(step(states)), want, "{states:?}");
        }
        // Work linked to the step itself also counts: an open task keeps it
        // from being done, and nothing started keeps it left.
        let mut parent = step(&[Done, Done]);
        parent.tasks = vec!["job-0003".into()];
        assert_eq!(project(parent.clone()), Running);
        // A missing explicit task is still required work, even if the
        // children are done. A stale card must not open a dependent step.
        parent.tasks = vec!["job-9999".into()];
        assert_eq!(project(parent), Running);
        let mut parent = step(&[Left]);
        parent.tasks = vec!["job-0003".into()];
        assert_eq!(project(parent), Left);
    }

    #[test]
    #[ignore = "manual on-disk schema verification; requires the Mac project records"]
    fn real_plan_records_load_without_changing_counts() {
        let home = std::env::var("HOME").unwrap();
        for slug in [
            "adeherdr",
            "elicio",
            "flyonenomics",
            "prl-8-53",
            "proprium",
            "somebody",
            "venator",
        ] {
            let path = std::path::Path::new(&home)
                .join(".herdr-ade")
                .join(slug)
                .join(".state/plan.toml");
            let text = std::fs::read_to_string(path).unwrap();
            let plan: Plan = toml::from_str(&text).unwrap();
            let steps: Vec<_> = all_steps(&plan).collect();
            println!(
                "{slug}: {}/{}",
                steps.iter().filter(|s| s.state == StepState::Done).count(),
                steps.len()
            );
        }
    }

    #[test]
    fn prerequisites_gate_bound_tasks_using_current_evidence() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        write_task(&fx, "job-0001");
        write_task(&fx, "job-0002");
        step_add(&ctx, "demo", "A", vec!["job-0001".into()], vec![], None).unwrap();
        step_add(
            &ctx,
            "demo",
            "B",
            vec!["job-0002".into()],
            vec!["s-1".into()],
            None,
        )
        .unwrap();
        let error = check_prerequisites(&fx.project, "job-0002")
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("plan_prerequisite:"), "{error}");
        let rejected = crate::threads::start(
            &ctx,
            "demo",
            crate::threads::StartArgs {
                title: "B".into(),
                repo: None,
                machine: None,
                base: None,
                task: "Build B".into(),
                workflow: None,
                recipe: None,
                task_id: "job-0002".into(),
                review_id: String::new(),
                attach: Vec::new(),
                paths: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(rejected.to_string().starts_with("plan_prerequisite:"));
        assert!(crate::thread::list(&fx.project).is_empty());
        // A task appearing on multiple steps must satisfy every edge.
        step_add(
            &ctx,
            "demo",
            "C",
            vec!["job-0002".into()],
            vec!["s-2".into()],
            None,
        )
        .unwrap();
        check_prerequisites(&fx.project, "job-0001").unwrap();
        let (lane, sha) = fx.lane(1);
        crate::task::link_attempt(&fx.project, "job-0001", &lane).unwrap();
        // A deferred launch is checked against the same current projection.
        let (deferred, _) = fx.lane(2);
        crate::task::link_attempt(&fx.project, "job-0002", &deferred).unwrap();
        assert!(check_attempt_prerequisites(&fx.project, &deferred).is_err());
        fx.seal_done(&lane, 1, 1, &sha, "# report\n");
        crate::thread::update(&fx.project, &lane, |t| t.merged_sha = sha.clone()).unwrap();
        assert_eq!(
            load(&fx.project).unwrap().unwrap().steps[0].state,
            StepState::Left
        );
        let error = check_prerequisites(&fx.project, "job-0002")
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("plan_prerequisite:"), "{error}");
        step_unlink(
            &ctx,
            "demo",
            "s-3",
            vec![],
            vec!["s-2".into()],
            "changed",
            None,
        )
        .unwrap();
        check_prerequisites(&fx.project, "job-0002").unwrap();
        check_attempt_prerequisites(&fx.project, &deferred).unwrap();
    }

    #[test]
    fn failed_critic_blocks_dependent_until_fresh_pass_and_old_seals_still_count() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        for id in ["job-0001", "job-0002", "job-0003"] {
            write_task(&fx, id);
        }
        step_add(&ctx, "demo", "A", vec!["job-0001".into()], vec![], None).unwrap();
        step_add(
            &ctx,
            "demo",
            "C",
            vec!["job-0002".into()],
            vec!["s-1".into()],
            None,
        )
        .unwrap();
        step_add(
            &ctx,
            "demo",
            "D",
            vec!["job-0003".into()],
            vec!["s-2".into()],
            None,
        )
        .unwrap();
        let (producer, sha) = fx.lane(1);
        crate::task::link_attempt(&fx.project, "job-0001", &producer).unwrap();
        fx.seal_done(&producer, 1, 1, &sha, "# report\n");
        crate::thread::update(&fx.project, &producer, |t| t.merged_sha = sha.clone()).unwrap();
        let (critic, sha) = fx.lane(2);
        crate::thread::update(&fx.project, &critic, |t| t.role = "critic".into()).unwrap();
        crate::task::link_attempt(&fx.project, "job-0002", &critic).unwrap();
        let failed_event = fx.seal_done(
            &critic,
            1,
            2,
            &sha,
            "+++\nverdict = \"FAIL\"\n+++\nneeds work\n",
        );
        crate::thread::update(&fx.project, &critic, |t| t.merged_sha = sha.clone()).unwrap();
        let shown: serde_json::Value = serde_json::to_value(show(&ctx, "demo").unwrap()).unwrap();
        assert_eq!(shown["steps"][1]["state"], "running");
        let error = check_prerequisites(&fx.project, "job-0003")
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("plan_prerequisite:"), "{error}");
        crate::thread::update(&fx.project, &critic, |t| t.review_after = failed_event).unwrap();
        assert!(check_prerequisites(&fx.project, "job-0003").is_err());
        fx.seal_done(
            &critic,
            1,
            3,
            &sha,
            "+++\nverdict = \"PASS\"\n+++\napproved\n",
        );
        check_prerequisites(&fx.project, "job-0003").unwrap();
        fx.seal_done(&critic, 1, 4, &sha, "# historical report\n");
        check_prerequisites(&fx.project, "job-0003").unwrap();
    }

    #[test]
    fn corrupt_failed_critic_evidence_never_completes_a_step() {
        for binding in ["task", "thread"] {
            for fault in ["missing", "mismatch", "utf8"] {
                let fx = fixture();
                let ctx = fx.world.ctx();
                write_task(&fx, "job-0001");
                write_task(&fx, "job-0002");
                let (critic, sha) = fx.lane(1);
                thread::update(&fx.project, &critic, |t| {
                    t.role = "critic".into();
                    t.merged_sha = sha.clone();
                })
                .unwrap();
                crate::task::link_attempt(&fx.project, "job-0001", &critic).unwrap();
                step_add(
                    &ctx,
                    "demo",
                    "Check the work",
                    if binding == "task" {
                        vec!["job-0001".into()]
                    } else {
                        vec![]
                    },
                    vec![],
                    None,
                )
                .unwrap();
                if binding == "thread" {
                    // Historical plans can bind a lane directly.
                    let mut plan = load(&fx.project).unwrap().unwrap();
                    plan.steps[0].threads = vec![critic.clone()];
                    write(&fx.project, &plan).unwrap();
                }
                step_add(
                    &ctx,
                    "demo",
                    "Use the work",
                    vec!["job-0002".into()],
                    vec!["s-1".into()],
                    None,
                )
                .unwrap();
                let report = "+++\nverdict = \"FAIL\"\n+++\nneeds work\n";
                let event_id = fx.seal_done(&critic, 1, 1, &sha, report);
                let shown: serde_json::Value =
                    serde_json::to_value(show(&ctx, "demo").unwrap()).unwrap();
                assert_eq!(shown["steps"][0]["state"], "running");
                let artifact_path = fx
                    .project
                    .state_dir()
                    .join("artifacts")
                    .join(thread::sha256_hex(report.as_bytes()));
                match fault {
                    "missing" => std::fs::remove_file(artifact_path).unwrap(),
                    "mismatch" => std::fs::write(artifact_path, "changed bytes").unwrap(),
                    "utf8" => {
                        // A hash-valid artifact still has to decode before its verdict can be trusted.
                        let artifact = thread::store_artifact(&fx.project, &[0xff]).unwrap();
                        let path = fx
                            .project
                            .state_dir()
                            .join("events")
                            .join(format!("{event_id}.toml"));
                        let mut event: crate::contracts::Event =
                            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                        event.payload.done.as_mut().unwrap().artifact = artifact;
                        std::fs::write(path, toml::to_string(&event).unwrap()).unwrap();
                    }
                    _ => unreachable!(),
                }
                let shown: serde_json::Value =
                    serde_json::to_value(show(&ctx, "demo").unwrap()).unwrap();
                assert_eq!(shown["steps"][0]["state"], "running", "{binding}: {fault}");
                let diagnostic = match fault {
                    "missing" => "brief_artifact_missing",
                    "mismatch" => "brief_artifact_mismatch",
                    _ => "invalid UTF-8",
                };
                assert!(show(&ctx, "demo").unwrap().message().contains(diagnostic));
                assert!(
                    shown["steps"][0]["failed_check_hold"]["checks"][0]["diagnostic"]
                        .as_str()
                        .unwrap()
                        .contains(diagnostic)
                );
                assert!(
                    check_prerequisites(&fx.project, "job-0002")
                        .unwrap_err()
                        .to_string()
                        .contains(diagnostic)
                );
                sync(&ctx, "demo").unwrap();
                assert_eq!(
                    load(&fx.project).unwrap().unwrap().steps[0].state,
                    StepState::Running
                );
            }
        }
    }

    #[test]
    fn unlink_atomically_keeps_only_changes_with_reason_and_revision() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        write_task(&fx, "job-0001");
        step_add(&ctx, "demo", "A", vec![], vec![], None).unwrap();
        step_add(
            &ctx,
            "demo",
            "B",
            vec!["job-0001".into()],
            vec!["s-1".into()],
            None,
        )
        .unwrap();
        // An unavailable old journal cannot separate the reason from the unlink.
        std::fs::create_dir(fx.project.state_dir().join("dispatch.jsonl")).unwrap();
        let plan = step_unlink(
            &ctx,
            "demo",
            "s-2",
            vec!["job-0001".into()],
            vec!["s-1".into()],
            "Rolf released the hold",
            None,
        )
        .unwrap();
        let rows = binding_changes(&fx.project).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].step, "s-2");
        assert_eq!(rows[0].bindings, vec!["job-0001", "s-1"]);
        assert_eq!(rows[0].why, "Rolf released the hold");
        assert_eq!(load(&fx.project).unwrap().unwrap().revision, plan.revision);
        let before = std::fs::read(plan_path(&fx.project)).unwrap();
        step_unlink(
            &ctx,
            "demo",
            "s-2",
            vec!["job-0001".into()],
            vec!["s-1".into()],
            "unchanged",
            None,
        )
        .unwrap();
        assert_eq!(std::fs::read(plan_path(&fx.project)).unwrap(), before);
        // A later writer must not silently discard unreadable reason history.
        let corrupt = String::from_utf8(before)
            .unwrap()
            .replace("why =", "lost_why =");
        std::fs::write(plan_path(&fx.project), &corrupt).unwrap();
        assert!(step_edit(&ctx, "demo", "s-2", "changed", None).is_err());
        assert_eq!(
            std::fs::read_to_string(plan_path(&fx.project)).unwrap(),
            corrupt
        );
    }

    #[test]
    fn unlink_covers_historical_tasks_and_direct_attempts_without_rewriting_history() {
        for shape in ["explicit", "historical", "direct", "union"] {
            let fx = fixture();
            let ctx = fx.world.ctx();
            write_task(&fx, "job-0001");
            let lane = fx.thread("failed check");
            thread::update(&fx.project, &lane, |t| {
                t.role = "critic".into();
                t.merged_sha = "old".into();
            })
            .unwrap();
            fx.seal_done(&lane, 1, 1, "old", "+++\nverdict = \"FAIL\"\n+++\n");
            crate::task::link_attempt(&fx.project, "job-0001", &lane).unwrap();
            let mut task = crate::task::load(&fx.project, "job-0001").unwrap();
            if shape == "historical" || shape == "union" {
                task.plan_step = Some("s-1".into());
            }
            let task_path = fx.project.state_dir().join("tasks/job-0001.toml");
            std::fs::write(&task_path, toml::to_string(&task).unwrap()).unwrap();
            step_add(
                &ctx,
                "demo",
                "Check",
                if shape == "explicit" || shape == "union" {
                    vec![task.id.clone()]
                } else {
                    vec![]
                },
                vec![],
                None,
            )
            .unwrap();
            if shape == "direct" || shape == "union" {
                with_plan(&fx.project, None, |plan, _| {
                    plan.steps[0].threads = vec![lane.clone()];
                    Ok(())
                })
                .unwrap();
            }
            let original = std::fs::read(&task_path).unwrap();
            assert_eq!(
                show(&ctx, "demo").unwrap().plan.steps[0].state,
                StepState::Running
            );
            let frozen = show(&ctx, "demo").unwrap();
            step_unlink(
                &ctx,
                "demo",
                "s-1",
                vec![task.id.clone()],
                vec![],
                "Rolf changed the required work",
                None,
            )
            .unwrap();
            let shown = show(&ctx, "demo").unwrap();
            assert_eq!(shown.plan.steps[0].state, StepState::Left, "{shape}");
            assert!(shown.evaluation.holds.is_empty());
            assert_eq!(std::fs::read(&task_path).unwrap(), original);
            assert_eq!(
                binding_changes(&fx.project).unwrap()[0].why,
                "Rolf changed the required work"
            );
            assert_eq!(frozen.plan.steps[0].state, StepState::Running);
            step_link(&ctx, "demo", "s-1", vec![task.id.clone()], vec![], None).unwrap();
            assert_eq!(
                show(&ctx, "demo").unwrap().plan.steps[0].state,
                StepState::Running
            );
            let before = std::fs::read(plan_path(&fx.project)).unwrap();
            step_link(&ctx, "demo", "s-1", vec![task.id.clone()], vec![], None).unwrap();
            assert_eq!(std::fs::read(plan_path(&fx.project)).unwrap(), before);
        }
    }

    #[test]
    fn concurrent_attempt_links_have_one_task_winner() {
        let fx = fixture();
        for id in ["job-0001", "job-0002"] {
            write_task(&fx, id);
        }
        let lane = fx.thread("one attempt");
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = ["job-0001", "job-0002"]
                .into_iter()
                .map(|id| {
                    let project = &fx.project;
                    let lane = &lane;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        crate::task::link_attempt(project, id, lane)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(
            results
                .iter()
                .find_map(|result| result.as_ref().err())
                .unwrap()
                .to_string()
                .contains("already belongs")
        );
        assert_eq!(
            crate::task::list_with_errors(&fx.project)
                .0
                .iter()
                .filter(|task| task.attempts.contains(&lane))
                .count(),
            1
        );
    }

    #[test]
    fn merged_work_with_a_dropped_task_finishes_and_opens_its_dependent() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        for id in ["job-0001", "job-0002", "job-0003", "job-0004"] {
            write_task(&fx, id);
        }
        step_add(
            &ctx,
            "demo",
            "A",
            vec!["job-0001".into(), "job-0002".into(), "job-0003".into()],
            vec![],
            None,
        )
        .unwrap();
        step_add(
            &ctx,
            "demo",
            "B",
            vec!["job-0004".into()],
            vec!["s-1".into()],
            None,
        )
        .unwrap();
        for n in 1..=2 {
            let (lane, sha) = fx.lane(n);
            crate::task::link_attempt(&fx.project, &format!("job-{n:04}"), &lane).unwrap();
            fx.seal_done(&lane, 1, n, &sha, "# report\n");
            crate::thread::update(&fx.project, &lane, |t| t.merged_sha = sha.clone()).unwrap();
        }
        crate::task::drop_task(&ctx, &fx.project, "job-0003", "No longer needed").unwrap();
        // Both the display and the launch gate must use the same projection,
        // even when the persisted card has not yet been synced.
        let shown: serde_json::Value = serde_json::to_value(show(&ctx, "demo").unwrap()).unwrap();
        assert_eq!(shown["steps"][0]["state"], "done");
        check_prerequisites(&fx.project, "job-0004").unwrap();
    }

    #[test]
    fn invalid_edges_are_refused_and_old_cards_have_no_gate() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        write_task(&fx, "job-0001");
        step_add(&ctx, "demo", "A", vec!["job-0001".into()], vec![], None).unwrap();
        check_prerequisites(&fx.project, "job-0001").unwrap();
        for edge in ["s-1", "s-99"] {
            assert!(step_link(&ctx, "demo", "s-1", vec![], vec![edge.into()], None).is_err());
        }
        step_add(&ctx, "demo", "B", vec![], vec!["s-1".into()], None).unwrap();
        assert!(step_link(&ctx, "demo", "s-1", vec![], vec!["s-2".into()], None).is_err());
        assert!(step_remove(&ctx, "demo", "s-1", None).is_err());
        let (_, sub) = subtask_add(&ctx, "demo", "s-1", "part", vec![], vec![], None).unwrap();
        assert!(step_link(&ctx, "demo", "s-1", vec![], vec![sub.clone()], None).is_err());
        assert!(step_link(&ctx, "demo", &sub, vec![], vec!["s-1".into()], None).is_err());
        // A child waiting on a step that waits on its parent also deadlocks:
        // the parent's completion implicitly waits on that child.
        assert!(step_link(&ctx, "demo", &sub, vec![], vec!["s-2".into()], None).is_err());
        step_unlink(
            &ctx,
            "demo",
            "s-2",
            vec![],
            vec!["s-1".into()],
            "changed",
            None,
        )
        .unwrap();
        assert!(step_remove(&ctx, "demo", "s-1", None).is_ok());
    }

    #[test]
    fn subtask_states_come_from_their_own_work() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        set(&ctx, "demo", "It shows pretend trades.", 0).unwrap();
        let (lane, sha) = fx.lane(1);
        add(&fx, "Land the lane", 1);
        subtask_add(&ctx, "demo", "s-1", "The lane's part", vec![], vec![], 2).unwrap();
        subtask_add(&ctx, "demo", "s-1", "The rest", vec![], vec![], 3).unwrap();
        let plan = with_plan(&fx.project, 4, |plan, _| {
            find_step(plan, "s-2")?.threads = vec![lane.clone()];
            Ok(())
        })
        .unwrap()
        .0;
        assert_eq!(plan.steps[0].subtasks[0].state, StepState::Running);
        assert_eq!(plan.steps[0].subtasks[1].state, StepState::Left);
        assert_eq!(plan.steps[0].state, StepState::Running);

        fx.seal_done(&lane, 1, 1, &sha, "# report\n");
        let before = load(&fx.project).unwrap().unwrap().revision;
        // A historical merged lane with a thread binding completes its subtask.
        crate::thread::update(&fx.project, &lane, |t| {
            t.merged_sha = sha.clone();
        })
        .unwrap();
        refresh(&ctx, &fx.project).unwrap();
        // Refresh writes a subtask that moved even when its step did not.
        let plan = load(&fx.project).unwrap().unwrap();
        assert!(plan.revision > before);
        assert_eq!(plan.steps[0].subtasks[0].state, StepState::Done);
        assert_eq!(plan.steps[0].state, StepState::Running);

        let plan = step_remove(&ctx, "demo", "s-3", None).unwrap();
        assert_eq!(plan.steps[0].state, StepState::Done);
    }
}
