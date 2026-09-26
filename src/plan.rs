//! The plan card (SPEC-talk §2.7 and §6.5): the file, its commands, checked
//! writes, revision guards and the shared state projection.
//!
//! A step's `state` is a persisted projection of the bound work, never a
//! coordinator-supplied status. The shared `refresh` reads the plan, the goal
//! and the durable work records under the plan writer lock and writes only on
//! change. It never rewrites the goal into the coordinator's wording.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::contracts::{Plan, PlanStep, StepState, plan_kind_sentence};
use crate::paths::Ctx;
use crate::project::{Project, write_atomic};
use crate::thread;

pub(crate) fn plan_path(project: &Project) -> PathBuf {
    project.record_file("plan.toml")
}

fn lock_path(project: &Project) -> PathBuf {
    project.state_dir().join("plan.lock")
}

struct PlanLock {
    _file: File,
}

/// The plan writer lock, `<project>/.plan.lock`. Separate from the project
/// lock so a refresh called from a thread or review mutation cannot deadlock.
fn plan_lock(project: &Project) -> Result<PlanLock> {
    let path = lock_path(project);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    file.lock()?;
    Ok(PlanLock { _file: file })
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
    let path = project.record_file_for_write("plan.toml")?;
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

/// The whole candidate card, including preserved text and generated
/// sentences. Refuses invalid language and duplicate identifiers before writing.
fn plan_kind_error(kind: &str) -> String {
    format!(
        "plan_kind: `{kind}` is not a result kind (possible values: {})",
        crate::contracts::PLAN_KINDS
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn validate(_project: &Project, plan: &Plan) -> Result<()> {
    if plan.schema != 1 {
        bail!("plan_schema: expected schema 1, got {}", plan.schema);
    }
    if !plan.kind.is_empty() {
        let sentence =
            plan_kind_sentence(&plan.kind).with_context(|| plan_kind_error(&plan.kind))?;
        if plan.what_you_get != sentence {
            bail!("plan_result: what_you_get must be \"{sentence}\"");
        }
    } else if !plan.what_you_get.is_empty() {
        bail!("plan_result: what_you_get is set without a result kind");
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
    validate_subtasks(plan, ids)
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
    validate(project, &plan)?;
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

/// `ha plan set --kind <kind> --does "<sentence>" [--expect <revision>]`.
/// Preserves the steps and refreshes the goal from `PROJECT.md`.
pub(crate) fn set(
    ctx: &Ctx,
    slug: &str,
    kind: &str,
    does: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    let sentence = plan_kind_sentence(kind).with_context(|| plan_kind_error(kind))?;
    let does = does.trim().to_string();
    let goal = project_goal(&project);
    let (plan, ()) = with_plan(&project, expect, |plan| {
        plan.kind = kind.to_string();
        plan.what_you_get = sentence.to_string();
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
    expect: impl Into<Option<u64>>,
) -> Result<(Plan, String)> {
    let project = Project::load(&ctx.root, slug)?;
    let text = text.trim().to_string();
    check_task_refs(&project, &tasks)?;
    with_plan(&project, expect, |plan| {
        let Some(at) = plan.steps.iter().position(|s| s.id == under) else {
            if all_steps(plan).any(|s| s.id == under) {
                bail!("plan_step_depth: `{under}` is a subtask; a subtask cannot have subtasks");
            }
            bail!("plan_step_unknown: `{under}` is not a step of this plan");
        };
        let id = next_id(plan);
        plan.steps[at].subtasks.push(PlanStep {
            id: id.clone(),
            text: text.clone(),
            state: StepState::Left,
            tasks: dedup(&tasks),
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
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    if tasks.is_empty() {
        bail!("plan_link: at least one --task is required");
    }
    check_task_refs(&project, &tasks)?;
    let (plan, ()) = with_plan(&project, expect, |plan| {
        let step = find_step(plan, id)?;
        for task in &tasks {
            if !step.tasks.contains(task) {
                step.tasks.push(task.clone());
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
    _why: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    if tasks.is_empty() {
        bail!("plan_unlink: at least one --task is required");
    }
    let (plan, ()) = with_plan(&project, expect, |plan| {
        let step = find_step(plan, id)?;
        step.tasks.retain(|task| !tasks.contains(task));
        Ok(())
    })?;
    Ok(plan)
}

pub(crate) fn step_remove(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    _why: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    let (plan, ()) = with_plan(&project, expect, |plan| {
        let before = all_steps(plan).count();
        plan.steps.retain(|s| s.id != id);
        for step in &mut plan.steps {
            step.subtasks.retain(|s| s.id != id);
        }
        if all_steps(plan).count() == before {
            bail!("plan_step_unknown: `{id}` is not a step of this plan");
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

/// `ha plan show [--json]`. Missing returns revision zero and `present:
/// false`; a normal call reports a goal that drifted from `PROJECT.md`.
pub(crate) fn show(ctx: &Ctx, slug: &str, json: bool) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let mut plan = load(&project)?;
    if let Some(card) = &mut plan {
        project_states(&project, card);
    }
    if json {
        let view = match &plan {
            Some(plan) => {
                let mut value = serde_json::to_value(plan)?;
                value["present"] = serde_json::json!(true);
                value
            }
            None => serde_json::json!({
                "present": false,
                "schema": 1,
                "revision": 0,
                "next_step": 0,
                "goal": project_goal(&project),
                "kind": "",
                "what_you_get": "",
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
    if plan.kind.is_empty() {
        out.push_str("what you get at the end: not written down yet\n");
    } else {
        out.push_str(&format!(
            "what you get at the end: {} {}\n",
            plan.what_you_get, plan.does
        ));
    }
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
        out.push_str(&format!(
            "{indent}{:<7} {}  {}{}\n",
            step.state.word(),
            step.id,
            step.text,
            refs
        ));
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncOutcome {
    Missing,
    Unchanged { revision: u64 },
    Changed { revision: u64 },
}

/// `ha plan sync`: derive states from the bound work records and write only
/// on change.
pub(crate) fn sync(ctx: &Ctx, slug: &str) -> Result<SyncOutcome> {
    let project = Project::load(&ctx.root, slug)?;
    let _lock = plan_lock(&project)?;
    let Some(mut plan) = load(&project)? else {
        return Ok(SyncOutcome::Missing);
    };
    if !project_states(&project, &mut plan) {
        crate::project::refresh_page(&project)?;
        return Ok(SyncOutcome::Unchanged {
            revision: plan.revision,
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
    for step in &mut plan.steps {
        for sub in &mut step.subtasks {
            let state = derive_state(project, sub, evidence);
            if sub.state != state {
                sub.state = state;
                changed = true;
            }
        }
        let mut state = derive_state(project, step, evidence);
        if !step.subtasks.is_empty() {
            state = with_subtasks(project, step, state, evidence);
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
fn with_subtasks(
    project: &Project,
    step: &PlanStep,
    own: StepState,
    evidence: &crate::task::EvidenceSnapshot,
) -> StepState {
    let children = step.subtasks.iter().map(|s| s.state);
    let own_blocks = own != StepState::Done && has_own_work(project, step, evidence);
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
fn has_own_work(
    project: &Project,
    step: &PlanStep,
    evidence: &crate::task::EvidenceSnapshot,
) -> bool {
    !step.threads.is_empty()
        || crate::task::list_with_errors(project)
            .0
            .into_iter()
            .filter(|task| {
                task.plan_step.as_deref() == Some(step.id.as_str()) || step.tasks.contains(&task.id)
            })
            .any(|task| {
                crate::task::view_with_evidence(project, task, evidence).state
                    != crate::task::State::Dropped
            })
}

/// Derive each state in the fixed order (SPEC-talk §6.5): `done` when at least
/// one binding exists and every binding is positively satisfied, else
/// `running` when any required work has started or partially landed, else
/// `left`.
fn derive_state(
    project: &Project,
    step: &PlanStep,
    evidence: &crate::task::EvidenceSnapshot,
) -> StepState {
    let linked_tasks: Vec<_> = crate::task::list_with_errors(project)
        .0
        .into_iter()
        .filter(|task| {
            task.plan_step.as_deref() == Some(step.id.as_str()) || step.tasks.contains(&task.id)
        })
        .map(|task| crate::task::view_with_evidence(project, task, evidence))
        .filter(|view| view.record.dropped.is_empty())
        .collect();
    if linked_tasks.is_empty() && step.threads.is_empty() {
        return StepState::Left;
    }
    let mut all_satisfied = true;
    let mut any_started = false;
    for view in linked_tasks {
        if !view.terminal_with_evidence(project, evidence) {
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
                all_satisfied &= crate::review::lane_done(project, &lane, evidence.events());
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

    fn goal(fx: &Fx, text: &str) {
        let path = fx.project.project_md();
        let md = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            md.replacen("goal = \"\"", &format!("goal = \"{text}\""), 1),
        )
        .unwrap();
        // Prove the replacement landed.
        assert_eq!(fx.project.read_project_md().unwrap().0.goal, text, "{text}");
    }

    fn add(fx: &Fx, text: &str, expect: u64) -> Plan {
        step_add(&fx.world.ctx(), "demo", text, vec![], expect).unwrap()
    }

    #[test]
    fn set_copies_the_exact_goal_and_generates_the_result_sentence() {
        let fx = fixture();
        goal(&fx, "I want to build a trading bot with Jeff.");
        let plan = set(
            &fx.world.ctx(),
            "demo",
            "screen",
            "It shows pretend trades and lets you stop them.",
            0,
        )
        .unwrap();
        assert_eq!(plan.revision, 1);
        assert_eq!(plan.schema, 1);
        assert_eq!(plan.goal, "I want to build a trading bot with Jeff.");
        assert_eq!(plan.kind, "screen");
        assert_eq!(plan.what_you_get, "A screen you open.");
        // A normal set preserves the steps.
        add(&fx, "Choose what the screen will show.", 1);
        let plan = set(
            &fx.world.ctx(),
            "demo",
            "command",
            "It prints the pretend trades.",
            2,
        )
        .unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.what_you_get, "A command you run.");
        assert_eq!(plan.revision, 3);
    }

    #[test]
    fn omitted_expect_uses_locked_revision_and_json_shows_it() {
        let fx = fixture();
        set(
            &fx.world.ctx(),
            "demo",
            "screen",
            "It shows the result.",
            None,
        )
        .unwrap();
        let plan = step_add(&fx.world.ctx(), "demo", "Build the screen.", vec![], None).unwrap();
        assert_eq!(plan.revision, 2);
        let json: serde_json::Value =
            serde_json::from_str(&show(&fx.world.ctx(), "demo", true).unwrap()).unwrap();
        assert_eq!(json["revision"], 2);
        assert!(
            set(
                &fx.world.ctx(),
                "demo",
                "command",
                "It runs the task.",
                Some(0)
            )
            .is_err()
        );
    }

    #[test]
    fn a_stale_expect_fails_unchanged_and_cannot_break_the_old_file() {
        let fx = fixture();
        set(
            &fx.world.ctx(),
            "demo",
            "screen",
            "It shows pretend trades.",
            0,
        )
        .unwrap();
        let before = std::fs::read_to_string(plan_path(&fx.project)).unwrap();
        let e = format!(
            "{:#}",
            set(
                &fx.world.ctx(),
                "demo",
                "command",
                "It prints the lines.",
                9
            )
            .unwrap_err()
        );
        assert!(e.starts_with("plan_revision_stale"), "{e}");
        assert_eq!(
            std::fs::read_to_string(plan_path(&fx.project)).unwrap(),
            before
        );
    }

    #[test]
    fn an_unchanged_mutation_does_not_advance_the_revision() {
        let fx = fixture();
        goal(&fx, "I want to build a trading bot with Jeff.");
        let first = set(
            &fx.world.ctx(),
            "demo",
            "screen",
            "It shows pretend trades.",
            0,
        )
        .unwrap();
        let same = set(
            &fx.world.ctx(),
            "demo",
            "screen",
            "It shows pretend trades.",
            first.revision,
        )
        .unwrap();
        assert_eq!(same.revision, first.revision);
    }

    #[test]
    fn steps_have_no_cap_and_removal_never_reuses_an_identifier() {
        let fx = fixture();
        set(
            &fx.world.ctx(),
            "demo",
            "screen",
            "It shows pretend trades.",
            0,
        )
        .unwrap();
        let mut expect = 1;
        for n in 0..7 {
            add(&fx, &format!("Step number {n}."), expect);
            expect += 1;
        }
        let plan = add(&fx, "An eighth step.", expect);
        let plan = step_remove(
            &fx.world.ctx(),
            "demo",
            "s-3",
            "It is not needed.",
            plan.revision,
        )
        .unwrap();
        assert_eq!(plan.steps.len(), 7);
        let plan = add(&fx, "A replacement step.", plan.revision);
        assert!(plan.steps.iter().any(|s| s.id == "s-9"), "{:?}", plan.steps);
        assert!(!plan.steps.iter().any(|s| s.id == "s-3"));
    }

    #[test]
    fn a_missing_card_creates_on_expect_zero() {
        let fx = fixture();
        let plan = add(&fx, "Take the first step.", 0);
        assert_eq!(plan.revision, 1);
        assert_eq!(plan.steps[0].id, "s-1");
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
        set(&ctx, "demo", "screen", "It shows pretend trades.", 0).unwrap();
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
    fn subtasks_nest_one_level_share_ids_and_go_with_their_step() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        write_task(&fx, "job-0001");
        set(&ctx, "demo", "screen", "It shows pretend trades.", 0).unwrap();
        add(&fx, "Build the screen", 1);
        add(&fx, "Try it out", 2);
        let (plan, id) = subtask_add(&ctx, "demo", "s-1", "Draw the list", vec![], 3).unwrap();
        assert_eq!(id, "s-3");
        assert_eq!(plan.steps[0].subtasks[0].state, StepState::Left);
        let (_, id) = subtask_add(&ctx, "demo", "s-1", "Colour the marks", vec![], None).unwrap();
        assert_eq!(id, "s-4");
        let e = format!(
            "{:#}",
            subtask_add(&ctx, "demo", "s-3", "Too deep", vec![], None).unwrap_err()
        );
        assert!(e.starts_with("plan_step_depth"), "{e}");
        let e = format!(
            "{:#}",
            subtask_add(&ctx, "demo", "s-9", "Nowhere", vec![], None).unwrap_err()
        );
        assert!(e.starts_with("plan_step_unknown"), "{e}");

        // Edit, link, unlink and remove take a subtask id like a step id.
        let plan = step_edit(&ctx, "demo", "s-4", "Colour the boxes", None).unwrap();
        assert_eq!(plan.steps[0].subtasks[1].text, "Colour the boxes");
        let plan = step_link(&ctx, "demo", "s-3", vec!["job-0001".into()], None).unwrap();
        assert_eq!(plan.steps[0].subtasks[0].tasks, ["job-0001"]);
        assert!(plan.steps[0].tasks.is_empty());
        let plan =
            step_unlink(&ctx, "demo", "s-3", vec!["job-0001".into()], "moved", None).unwrap();
        assert!(plan.steps[0].subtasks[0].tasks.is_empty());

        // The card on disk carries them nested, and loads back the same.
        let text = std::fs::read_to_string(plan_path(&fx.project)).unwrap();
        assert!(text.contains("[[steps.subtasks]]"), "{text}");
        assert_eq!(load(&fx.project).unwrap().unwrap(), plan);

        let shown = show(&ctx, "demo", false).unwrap();
        assert!(
            shown.contains(
                "  left    s-1  Build the screen\n      left    s-3  Draw the list\n      left    s-4  Colour the boxes\n  left    s-2  Try it out\n"
            ),
            "{shown}"
        );
        let json: serde_json::Value =
            serde_json::from_str(&show(&ctx, "demo", true).unwrap()).unwrap();
        assert_eq!(json["steps"][0]["subtasks"][1]["id"], "s-4");
        assert!(json["steps"][1].get("subtasks").is_none(), "{json}");

        let plan = step_remove(&ctx, "demo", "s-4", "not needed", None).unwrap();
        assert_eq!(plan.steps[0].subtasks.len(), 1);
        assert_eq!(plan.steps.len(), 2);
        // Removing a step removes its subtasks; no id is reused.
        let plan = step_remove(&ctx, "demo", "s-1", "not needed", None).unwrap();
        assert_eq!(all_steps(&plan).count(), 1);
        let (_, id) = subtask_add(&ctx, "demo", "s-2", "Open it once", vec![], None).unwrap();
        assert_eq!(id, "s-5");
        assert!(step_edit(&ctx, "demo", "s-3", "Gone", None).is_err());

        // Subtasks do not count toward the step cap.
        for n in 0..6 {
            add(
                &fx,
                &format!("Step number {n}."),
                load(&fx.project).unwrap().unwrap().revision,
            );
        }
        for n in 0..4 {
            subtask_add(&ctx, "demo", "s-2", &format!("Part {n}"), vec![], None).unwrap();
        }
        let plan = load(&fx.project).unwrap().unwrap();
        assert_eq!(plan.steps.len(), 7);
        assert_eq!(plan.steps[0].subtasks.len(), 5);
    }

    #[test]
    fn a_plan_without_subtasks_reads_exactly_as_before() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        set(&ctx, "demo", "screen", "It shows pretend trades.", 0).unwrap();
        add(&fx, "Build the screen", 1);
        add(&fx, "Try it out", 2);
        assert_eq!(
            std::fs::read_to_string(plan_path(&fx.project)).unwrap(),
            "schema = 1\nrevision = 3\nnext_step = 3\ngoal = \"\"\nkind = \"screen\"\nwhat_you_get = \"A screen you open.\"\ndoes = \"It shows pretend trades.\"\n\n[[steps]]\nid = \"s-1\"\ntext = \"Build the screen\"\nstate = \"left\"\ntasks = []\nthreads = []\n\n[[steps]]\nid = \"s-2\"\ntext = \"Try it out\"\nstate = \"left\"\ntasks = []\nthreads = []\n"
        );
        assert_eq!(
            show(&ctx, "demo", false).unwrap(),
            "revision 3\ngoal: \nwhat you get at the end: A screen you open. It shows pretend trades.\nsteps:\n  left    s-1  Build the screen\n  left    s-2  Try it out\n"
        );
        let json: serde_json::Value =
            serde_json::from_str(&show(&ctx, "demo", true).unwrap()).unwrap();
        assert_eq!(
            json["steps"][0],
            serde_json::json!({
                "id": "s-1", "text": "Build the screen", "state": "left",
                "tasks": [], "threads": [],
            })
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
            assert_eq!(
                with_subtasks(&fx.project, &parent, own, &evidence),
                want,
                "{states:?}"
            );
        }
        // Work linked to the step itself also counts: an open task keeps it
        // from being done, and nothing started keeps it left.
        write_task(&fx, "job-0001");
        let mut parent = step(&[Done, Done]);
        parent.tasks = vec!["job-0001".into()];
        let own = derive_state(&fx.project, &parent, &evidence);
        assert_eq!(own, Left);
        assert_eq!(with_subtasks(&fx.project, &parent, own, &evidence), Running);
        let mut parent = step(&[Left]);
        parent.tasks = vec!["job-0001".into()];
        assert_eq!(with_subtasks(&fx.project, &parent, own, &evidence), Left);
    }

    #[test]
    fn subtask_states_come_from_their_own_work() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        set(&ctx, "demo", "screen", "It shows pretend trades.", 0).unwrap();
        let (lane, sha) = fx.lane(1);
        add(&fx, "Land the lane", 1);
        subtask_add(&ctx, "demo", "s-1", "The lane's part", vec![], 2).unwrap();
        subtask_add(&ctx, "demo", "s-1", "The rest", vec![], 3).unwrap();
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

        let plan = step_remove(&ctx, "demo", "s-3", "not needed", None).unwrap();
        assert_eq!(plan.steps[0].state, StepState::Done);
    }
}
