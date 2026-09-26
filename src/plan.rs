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
        if plan.next_step == 0 {
            // An old card without `next_step`: never reuse a live id.
            plan.next_step = plan
                .steps
                .iter()
                .filter_map(|s| s.id.strip_prefix("s-")?.parse::<u64>().ok())
                .max()
                .unwrap_or(0)
                + 1;
        }
        let id = format!("s-{}", plan.next_step);
        plan.next_step += 1;
        plan.steps.push(PlanStep {
            id,
            text: text.clone(),
            state: StepState::Left,
            tasks: dedup(&tasks),
            threads: Vec::new(),
        });
        Ok(())
    })?;
    Ok(plan)
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
        let before = plan.steps.len();
        plan.steps.retain(|s| s.id != id);
        if plan.steps.len() == before {
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
    plan.steps
        .iter_mut()
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
    for step in &plan.steps {
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
            "  {:<7} {}  {}{}\n",
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
        let state = derive_state(project, step, evidence);
        if step.state != state {
            step.state = state;
            changed = true;
        }
    }
    changed
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
}
