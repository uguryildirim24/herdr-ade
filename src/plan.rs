//! The plan card (SPEC-talk §2.7 and §6.5): the file, its commands, checked
//! writes, revision guards and the shared state projection.
//!
//! A step's `state` is a persisted projection of the bound work, never a
//! coordinator-supplied status. The shared `refresh` reads the plan, the goal
//! and the durable work records under the plan writer lock and writes only on
//! change. It never rewrites the goal into the coordinator's wording.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::contracts::{Plan, PlanStep, StepState};
use crate::paths::Ctx;
use crate::project::{Project, write_atomic};
use crate::thread;

pub(crate) fn plan_path(project: &Project) -> PathBuf {
    project.record_file("plan.toml")
}

/// The plan writer lock, `<project>/.state/plan.lock`. Separate from the
/// project lock so a refresh from a thread or review mutation cannot deadlock.
fn plan_lock(project: &Project) -> Result<File> {
    crate::project::lock_file(&project.state_dir().join("plan.lock"))
}

pub(crate) fn load(project: &Project) -> Result<Option<Plan>> {
    let path = plan_path(project);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            Ok(Some(toml::from_str(&text).with_context(|| {
                format!("{} does not parse", path.display())
            })?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// A locked write to a temporary file, flushed, atomically renamed and the
/// parent directory flushed (SPEC-talk §6.5).
fn write(project: &Project, plan: &Plan) -> Result<()> {
    let text = toml::to_string(plan)?;
    let path = project.record_file("plan.toml");
    write_atomic(&path, text.as_bytes())?;
    sync_dir(project.state_dir().as_path())?;
    Ok(())
}

fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir)?.sync_all()?;
    Ok(())
}

fn step_id_ok(id: &str) -> bool {
    let digits = id.strip_prefix("s-").unwrap_or("");
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

fn dedup(values: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            out.push(value.clone());
        }
    }
    out
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
    for step in &plan.steps {
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
    validate_subtasks(plan, ids)?;
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

/// Subtasks: step ids unique across both levels, and one level only.
fn validate_subtasks(plan: &Plan, mut ids: BTreeSet<String>) -> Result<()> {
    for step in &plan.steps {
        for sub in &step.subtasks {
            if !step_id_ok(&sub.id) {
                bail!("plan_step_id: `{}` is not a step id (expected s-1)", sub.id);
            }
            if !ids.insert(sub.id.clone()) {
                bail!("plan_step_id: duplicate step id `{}`", sub.id);
            }
            if !sub.subtasks.is_empty() {
                bail!(
                    "plan_step_depth: `{}` is a subtask; a subtask cannot have subtasks",
                    sub.id
                );
            }
            for t in &sub.threads {
                thread::validate_id(t)?;
            }
        }
    }
    Ok(())
}

/// One revision-guarded, validated, atomic mutation.
fn with_plan<T>(
    project: &Project,
    expect: impl Into<Option<u64>>,
    change: impl FnOnce(&mut Plan) -> Result<T>,
) -> Result<(Plan, T)> {
    let _lock = plan_lock(project)?;
    let mut plan = load(project)?.unwrap_or_default();
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
    let before = plan.clone();
    if plan.schema == 0 {
        plan.schema = 1;
    }
    let extra = change(&mut plan)?;
    // Every plan mutation also refreshes the persisted projection. In
    // particular, adding or removing a binding must not leave a stale state
    // until a later `plan sync`.
    project_states(project, &mut plan);
    validate(&plan)?;
    if plan == before {
        crate::project::refresh_page(project)?;
        return Ok((plan, extra));
    }
    plan.revision += 1;
    write(project, &plan)?;
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
    let does = does.trim().to_string();
    let goal = project_goal(&project);
    let (plan, ()) = with_plan(&project, expect, |plan| {
        plan.kind.clear();
        plan.what_you_get.clear();
        plan.does = does.clone();
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
    let text = text.trim().to_string();
    check_task_refs(&project, &tasks)?;
    let (plan, ()) = with_plan(&project, expect, |plan| {
        let id = next_id(plan);
        plan.steps.push(PlanStep {
            id,
            text: text.clone(),
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
    let text = text.trim().to_string();
    check_task_refs(&project, &tasks)?;
    with_plan(&project, expect, |plan| {
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
            text: text.clone(),
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
    let text = text.trim().to_string();
    let (plan, ()) = with_plan(&project, expect, |plan| {
        let step = find_step(plan, id)?;
        step.text = text.clone();
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
    let (plan, ()) = with_plan(&project, expect, |plan| {
        let step = find_step(plan, id)?;
        for task in &tasks {
            if !step.tasks.contains(task) {
                step.tasks.push(task.clone());
            }
        }
        for id in &after {
            if !step.after.contains(id) {
                step.after.push(id.clone());
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
    let (plan, (removed_tasks, removed_after, changed)) = with_plan(&project, expect, |plan| {
        let step = find_step(plan, id)?;
        let removed_tasks: Vec<_> = step
            .tasks
            .iter()
            .filter(|t| tasks.contains(t))
            .cloned()
            .collect();
        let removed_after: Vec<_> = step
            .after
            .iter()
            .filter(|a| after.contains(a))
            .cloned()
            .collect();
        step.tasks.retain(|task| !tasks.contains(task));
        step.after.retain(|edge| !after.contains(edge));
        let changed = !removed_tasks.is_empty() || !removed_after.is_empty();
        Ok((removed_tasks, removed_after, changed))
    })?;
    if changed {
        crate::launch::dispatch(
            &project,
            serde_json::json!({
                "kind": "plan-unlink", "step": id, "after": removed_after,
                "tasks": removed_tasks, "why": why, "revision": plan.revision,
            }),
        )?;
    }
    Ok(plan)
}

pub(crate) fn step_remove(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    let (plan, ()) = with_plan(&project, expect, |plan| {
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
    let (plan, ()) = with_plan(&project, expect, |plan| {
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

/// `ha plan show [--json]`. Missing returns revision zero and `present:
/// false`; a normal call reports a goal that drifted from `PROJECT.md`.
pub(crate) fn show(ctx: &Ctx, slug: &str, json: bool) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let mut plan = load(&project)?;
    let evidence = crate::task::EvidenceSnapshot::load(&project);
    if let Some(card) = &mut plan {
        project_states_with_evidence(&project, card, &evidence);
    }
    let holds = plan
        .as_ref()
        .map(|card| failed_check_holds(&project, card, &evidence))
        .unwrap_or_default();
    if json {
        let view = match &plan {
            Some(plan) => {
                let mut value = serde_json::to_value(plan)?;
                value["present"] = serde_json::json!(true);
                for step in value["steps"].as_array_mut().into_iter().flatten() {
                    add_hold_json(step, &holds);
                    if let Some(subtasks) = step
                        .get_mut("subtasks")
                        .and_then(|value| value.as_array_mut())
                    {
                        for sub in subtasks {
                            add_hold_json(sub, &holds);
                        }
                    }
                }
                value
            }
            None => serde_json::json!({
                "present": false,
                "schema": 1,
                "revision": 0,
                "next_step": 0,
                "goal": project_goal(&project),
                "does": "",
                "steps": [],
            }),
        };
        return Ok(format!("{}\n", serde_json::to_string_pretty(&view)?));
    }
    let Some(plan) = plan else {
        return Ok("no plan is written down yet (revision 0)\n".into());
    };
    let mut out = String::new();
    out.push_str(&format!("revision {}\n", plan.revision));
    if plan.goal != project_goal(&project) {
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
        let linked_tasks: Vec<String> = crate::task::list_with_errors(&project)
            .0
            .into_iter()
            .filter(|task| {
                task.plan_step.as_deref() == Some(step.id.as_str()) || step.tasks.contains(&task.id)
            })
            .map(|task| task.id)
            .collect();
        if !linked_tasks.is_empty() {
            refs.push_str(&format!(" tasks {}", linked_tasks.join(", ")));
        }
        if !step.threads.is_empty() {
            refs.push_str(&format!(" threads {}", step.threads.join(", ")));
        }
        if !step.after.is_empty() {
            refs.push_str(&format!(" after {}", step.after.join(", ")));
        }
        out.push_str(&format!(
            "{indent}{:<7} {}  {}{}\n",
            step.state.word(),
            step.id,
            step.text,
            refs
        ));
        if let Some(hold) = holds.get(&step.id) {
            out.push_str(&format!("{indent}  {}\n", hold.message()));
        }
    }
    Ok(out)
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
    let task = crate::task::load(project, job)?;
    let (_, errors) = crate::task::list_with_errors(project);
    let evidence = crate::task::EvidenceSnapshot::load(project);
    project_states_with_evidence(project, &mut plan, &evidence);
    let readable = errors.is_empty() && evidence.readable();
    for dependent in all_steps(&plan).filter(|step| {
        step.tasks.iter().any(|id| id == job) || task.plan_step.as_deref() == Some(&step.id)
    }) {
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
                if let Some(check) = failed_check(project, prerequisite, &evidence) {
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
        }
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
    let project = Project::load(&ctx.root, slug)?;
    let _lock = plan_lock(&project)?;
    let Some(mut plan) = load(&project)? else {
        return Ok(SyncOutcome::Missing);
    };
    let evidence = crate::task::EvidenceSnapshot::load(&project);
    if !project_states_with_evidence(&project, &mut plan, &evidence) {
        crate::project::refresh_page(&project)?;
        return Ok(SyncOutcome::Unchanged {
            revision: plan.revision,
            holds: failed_check_holds(&project, &plan, &evidence),
        });
    }
    plan.revision += 1;
    write(&project, &plan)?;
    crate::project::refresh_page(&project)?;
    Ok(SyncOutcome::Changed {
        revision: plan.revision,
    })
}

/// The shared projection refresh (SPEC-talk §6.5). Reads the current card and
/// work records and writes only on change; a missing card is a no-op. Callers
/// treat a failure as a separate refresh failure, never a merge failure.
pub(crate) fn refresh(_ctx: &Ctx, project: &Project) -> Result<bool> {
    let _lock = plan_lock(project)?;
    let Some(mut plan) = load(project)? else {
        crate::project::refresh_page(project)?;
        return Ok(false);
    };
    if !project_states(project, &mut plan) {
        crate::project::refresh_page(project)?;
        return Ok(false);
    }
    plan.revision += 1;
    write(project, &plan)?;
    crate::project::refresh_page(project)?;
    Ok(true)
}

/// Flips every step's persisted state to the state its bindings derive.
pub(crate) fn project_states(project: &Project, plan: &mut Plan) -> bool {
    project_states_with_evidence(project, plan, &crate::task::EvidenceSnapshot::load(project))
}

pub(crate) fn project_states_with_evidence(
    project: &Project,
    plan: &mut Plan,
    evidence: &crate::task::EvidenceSnapshot,
) -> bool {
    let mut changed = false;
    // Task enumeration parses every record. Reuse one snapshot across the
    // whole plan rather than repeating it for every step and subtask.
    let tasks = crate::task::list_with_errors(project).0;
    for step in &mut plan.steps {
        for sub in &mut step.subtasks {
            let state = derive_state_from_tasks(project, sub, evidence, &tasks);
            if sub.state != state {
                sub.state = state;
                changed = true;
            }
        }
        let mut state = derive_state_from_tasks(project, step, evidence, &tasks);
        if !step.subtasks.is_empty() {
            state = with_subtasks_from_tasks(step, state, &tasks);
        }
        if step.state != state {
            step.state = state;
            changed = true;
        }
    }
    changed
}

/// A step with subtasks, on top of its own leaf state `own`: done when every
/// subtask is done and its own linked work (if any) is done too; running when
/// any subtask or its own work is running or done; else left.
#[cfg(test)]
fn with_subtasks(project: &Project, step: &PlanStep, own: StepState) -> StepState {
    with_subtasks_from_tasks(step, own, &crate::task::list_with_errors(project).0)
}

fn with_subtasks_from_tasks(
    step: &PlanStep,
    own: StepState,
    tasks: &[crate::task::Task],
) -> StepState {
    let children = step.subtasks.iter().map(|s| s.state);
    let own_blocks = own != StepState::Done && has_own_work(step, tasks);
    if !own_blocks && children.clone().all(|s| s == StepState::Done) {
        StepState::Done
    } else if own != StepState::Left || children.into_iter().any(|s| s != StepState::Left) {
        StepState::Running
    } else {
        StepState::Left
    }
}

/// Whether the step itself carries work the leaf rule counts: a live linked
/// task or a thread. Its leaf state reads `left` both without work
/// and with work not yet started.
fn has_own_work(step: &PlanStep, tasks: &[crate::task::Task]) -> bool {
    !step.threads.is_empty()
        // A dropped binding is deliberately excluded, but a missing explicit
        // binding still blocks a parent from completing through its children.
        || step.tasks.iter().any(|id| {
            !tasks.iter().any(|task| task.id == *id && !task.dropped.is_empty())
        })
        || tasks.iter().any(|task| {
            task.dropped.is_empty()
                && (task.plan_step.as_deref() == Some(step.id.as_str())
                    || step.tasks.contains(&task.id))
        })
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

fn add_hold_json(value: &mut serde_json::Value, holds: &BTreeMap<String, FailedCheckHold>) {
    if let Some(hold) = value["id"].as_str().and_then(|id| holds.get(id)) {
        value["failed_check_hold"] = serde_json::json!({
            "checks": hold.checks,
            "next": hold.next,
            "message": hold.message(),
        });
    }
}

/// Explain only steps whose work is otherwise satisfied. Reuse the same
/// terminal evidence as derivation, including missing bindings and children.
pub(crate) fn failed_check_holds(
    project: &Project,
    plan: &Plan,
    evidence: &crate::task::EvidenceSnapshot,
) -> BTreeMap<String, FailedCheckHold> {
    let (tasks, errors) = crate::task::list_with_errors(project);
    if !errors.is_empty() || !evidence.readable() {
        return BTreeMap::new();
    }
    all_steps(plan).filter(|step| step.state == StepState::Running).filter_map(|step| {
        let checks = terminal_checks(project, step, evidence, &tasks)?;
        if checks.is_empty() {
            return None;
        }
        let next = "get a fresh critic verdict (re-check, sealed PASS), or unlink the check task with a reason".into();
        Some((step.id.clone(), FailedCheckHold { checks, next }))
    }).collect()
}

fn terminal_checks(
    project: &Project,
    step: &PlanStep,
    evidence: &crate::task::EvidenceSnapshot,
    tasks: &[crate::task::Task],
) -> Option<Vec<HoldingCheck>> {
    if step
        .tasks
        .iter()
        .any(|id| !tasks.iter().any(|task| &task.id == id))
    {
        return None;
    }
    if !has_own_work(step, tasks) && step.subtasks.is_empty() {
        return None;
    }
    let mut checks = Vec::new();
    for task in tasks.iter().filter(|task| {
        task.dropped.is_empty()
            && (step.tasks.contains(&task.id) || task.plan_step.as_deref() == Some(&step.id))
    }) {
        let view = crate::task::view_with_evidence(project, task.clone(), evidence);
        if !view.terminal_with_evidence(project, evidence) {
            return None;
        }
        if let Some(check) = task_failed_check(project, task, evidence) {
            checks.push(check);
        }
    }
    for id in &step.threads {
        let lane = thread::load(project, id).ok()?;
        if !crate::review::lane_done(project, &lane, evidence.events()) {
            return None;
        }
        if let Some(mut check) = critic_check(project, &lane, evidence)
            && !checks.iter().any(|check| &check.lane_id == id)
        {
            check.task_id = tasks
                .iter()
                .find(|task| task.attempts.last() == Some(id))
                .map(|task| task.id.clone());
            checks.push(check);
        }
    }
    for sub in &step.subtasks {
        checks.extend(terminal_checks(project, sub, evidence, tasks)?);
    }
    let mut seen = BTreeSet::new();
    checks.retain(|check| seen.insert((check.lane_id.clone(), check.task_id.clone())));
    Some(checks)
}

fn critic_check(
    project: &Project,
    lane: &thread::Thread,
    evidence: &crate::task::EvidenceSnapshot,
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

fn task_failed_check(
    project: &Project,
    task: &crate::task::Task,
    evidence: &crate::task::EvidenceSnapshot,
) -> Option<HoldingCheck> {
    let id = task.attempts.last()?;
    let lane = thread::load(project, id).ok()?;
    let mut check = critic_check(project, &lane, evidence)?;
    check.task_id = Some(task.id.clone());
    Some(check)
}

fn failed_check(
    project: &Project,
    step: &PlanStep,
    evidence: &crate::task::EvidenceSnapshot,
) -> Option<HoldingCheck> {
    let tasks = crate::task::list_with_errors(project).0;
    for task in tasks.iter().filter(|task| {
        task.dropped.is_empty()
            && (step.tasks.contains(&task.id) || task.plan_step.as_deref() == Some(&step.id))
    }) {
        if let Some(id) = task_failed_check(project, task, evidence) {
            return Some(id);
        }
    }
    for id in &step.threads {
        let Ok(lane) = thread::load(project, id) else {
            continue;
        };
        if let Some(check) = critic_check(project, &lane, evidence) {
            return Some(check);
        }
    }
    for sub in &step.subtasks {
        if let Some(id) = failed_check(project, sub, evidence) {
            return Some(id);
        }
    }
    None
}

/// Derive each state in the fixed order (SPEC-talk §6.5): `done` when at least
/// one binding exists and every binding is positively satisfied, else
/// `running` when any required work has started or partially landed, else
/// `left`.
#[cfg(test)]
fn derive_state(
    project: &Project,
    step: &PlanStep,
    evidence: &crate::task::EvidenceSnapshot,
) -> StepState {
    derive_state_from_tasks(
        project,
        step,
        evidence,
        &crate::task::list_with_errors(project).0,
    )
}

fn derive_state_from_tasks(
    project: &Project,
    step: &PlanStep,
    evidence: &crate::task::EvidenceSnapshot,
    tasks: &[crate::task::Task],
) -> StepState {
    let linked_tasks: Vec<_> = tasks
        .iter()
        .filter(|task| {
            task.plan_step.as_deref() == Some(step.id.as_str()) || step.tasks.contains(&task.id)
        })
        .cloned()
        .map(|task| crate::task::view_with_evidence(project, task, evidence))
        .filter(|view| view.record.dropped.is_empty())
        .collect();
    if linked_tasks.is_empty() && step.threads.is_empty() {
        return StepState::Left;
    }
    // Explicit dropped tasks exist but are excluded from the live bindings.
    // Only a genuinely missing or unreadable explicit task blocks completion.
    let mut all_satisfied = step
        .tasks
        .iter()
        .all(|id| tasks.iter().any(|task| task.id == *id));
    let mut any_started = false;
    for view in linked_tasks {
        if !view.terminal_with_evidence(project, evidence)
            || task_failed_check(project, &view.record, evidence).is_some()
        {
            all_satisfied = false;
        }
        if view.state != crate::task::State::Open {
            any_started = true;
        }
    }
    for id in &step.threads {
        match thread::load(project, id) {
            Ok(lane) => {
                any_started = true;
                all_satisfied &= crate::review::lane_done(project, &lane, evidence.events())
                    && critic_check(project, &lane, evidence).is_none();
            }
            Err(_) => all_satisfied = false,
        }
    }
    if all_satisfied {
        StepState::Done
    } else if any_started {
        StepState::Running
    } else {
        StepState::Left
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{Fx, fixture};

    fn add(fx: &Fx, text: &str, expect: u64) -> Plan {
        step_add(&fx.world.ctx(), "demo", text, vec![], vec![], expect).unwrap()
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
            serde_json::from_str(&show(&fx.world.ctx(), "demo", true).unwrap()).unwrap();
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
        with_plan(&fx.project, expect, |plan| {
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
    fn authored_outcomes_replace_historical_prose_without_changing_steps() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let (lane, _) = fx.lane(1);
        add(&fx, "Compare red.md and blue.md", 0);
        let old = link_historical_threads(&fx, vec![lane], 1);
        let old = with_plan(&fx.project, old.revision, |plan| {
            plan.kind = "unlisted historical kind".into();
            plan.what_you_get = "Compare files (Rolf, 2026-09-25).".into();
            Ok(())
        })
        .unwrap()
        .0;
        assert!(
            show(&ctx, "demo", false)
                .unwrap()
                .contains(&old.what_you_get)
        );
        let outcome =
            "Compare red.md and blue.md: show differences; keep names (Rolf, 2026-09-25).";
        let changed = set(&ctx, "demo", outcome, old.revision).unwrap();
        assert_eq!(changed.steps, old.steps);
        assert_eq!(changed.does, outcome);
        assert!(changed.kind.is_empty() && changed.what_you_get.is_empty());
        assert!(show(&ctx, "demo", false).unwrap().contains(outcome));
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
        let evidence = crate::task::EvidenceSnapshot::load(&fx.project);
        let step = |states: &[StepState]| PlanStep {
            id: "s-1".into(),
            subtasks: states
                .iter()
                .enumerate()
                .map(|(n, state)| PlanStep {
                    id: format!("s-{}", n + 2),
                    state: *state,
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
            let parent = step(states);
            let own = derive_state(&fx.project, &parent, &evidence);
            assert_eq!(with_subtasks(&fx.project, &parent, own), want, "{states:?}");
        }
        // Work linked to the step itself also counts: an open task keeps it
        // from being done, and nothing started keeps it left.
        write_task(&fx, "job-0001");
        let mut parent = step(&[Done, Done]);
        parent.tasks = vec!["job-0001".into()];
        let own = derive_state(&fx.project, &parent, &evidence);
        assert_eq!(own, Left);
        assert_eq!(with_subtasks(&fx.project, &parent, own), Running);
        // A missing explicit task is still required work, even if the
        // children are done. A stale card must not open a dependent step.
        parent.tasks = vec!["job-9999".into()];
        assert_eq!(derive_state(&fx.project, &parent, &evidence), Left);
        assert_eq!(with_subtasks(&fx.project, &parent, Left), Running);
        let mut parent = step(&[Left]);
        parent.tasks = vec!["job-0001".into()];
        assert_eq!(with_subtasks(&fx.project, &parent, own), Left);
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
        let shown: serde_json::Value =
            serde_json::from_str(&show(&ctx, "demo", true).unwrap()).unwrap();
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
                    serde_json::from_str(&show(&ctx, "demo", true).unwrap()).unwrap();
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
                    serde_json::from_str(&show(&ctx, "demo", true).unwrap()).unwrap();
                assert_eq!(shown["steps"][0]["state"], "running", "{binding}: {fault}");
                let diagnostic = match fault {
                    "missing" => "brief_artifact_missing",
                    "mismatch" => "brief_artifact_mismatch",
                    _ => "invalid UTF-8",
                };
                assert!(show(&ctx, "demo", false).unwrap().contains(diagnostic));
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
    fn unlink_journals_only_changes_with_reason_and_revision() {
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
        let journal = fx.project.state_dir().join("dispatch.jsonl");
        let before = std::fs::read_to_string(&journal)
            .unwrap_or_default()
            .lines()
            .count();
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
        let records: Vec<serde_json::Value> = std::fs::read_to_string(&journal)
            .unwrap()
            .lines()
            .skip(before)
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 1);
        let row = &records[0];
        assert_eq!(row["kind"], "plan-unlink");
        assert_eq!(row["step"], "s-2");
        assert_eq!(row["after"], serde_json::json!(["s-1"]));
        assert_eq!(row["tasks"], serde_json::json!(["job-0001"]));
        assert_eq!(row["why"], "Rolf released the hold");
        assert_eq!(row["revision"], plan.revision);
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
        assert_eq!(
            std::fs::read_to_string(&journal).unwrap().lines().count(),
            before + 1
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
        crate::task::drop_task(&fx.project, "job-0003", "No longer needed").unwrap();
        // Both the display and the launch gate must use the same projection,
        // even when the persisted card has not yet been synced.
        let shown: serde_json::Value =
            serde_json::from_str(&show(&ctx, "demo", true).unwrap()).unwrap();
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
        let plan = with_plan(&fx.project, 4, |plan| {
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
