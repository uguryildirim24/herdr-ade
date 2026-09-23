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
use crate::glossary;
use crate::paths::Ctx;
use crate::project::{Project, write_atomic};
use crate::round;
use crate::thread;
use crate::threads;

/// At most seven active steps (SPEC-talk §6.5).
const MAX_STEPS: usize = 7;

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
/// lock so a refresh called from a thread or round mutation cannot deadlock.
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
/// sentences. Refuses invalid language, duplicate identifiers and excess
/// steps before anything is written (SPEC-talk §6.5).
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

fn validate(project: &Project, plan: &Plan) -> Result<()> {
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
    if !plan.does.is_empty() {
        glossary::check_sentence(project, "does", &plan.does)?;
    }
    if plan.steps.len() > MAX_STEPS {
        bail!(
            "plan_steps: at most {MAX_STEPS} steps, got {}",
            plan.steps.len()
        );
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
        glossary::check_sentence(project, &format!("step {}", step.id), &step.text)?;
        for t in &step.threads {
            thread::validate_id(t)?;
        }
        for r in &step.rounds {
            round::validate_round_id(r)?;
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
    let does = glossary::check_sentence(&project, "does", does)?;
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
    let text = glossary::check_sentence(&project, "step", text)?;
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
            rounds: Vec::new(),
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
    let text = glossary::check_sentence(&project, "step", text)?;
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
    why: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    if tasks.is_empty() {
        bail!("plan_unlink: at least one --task is required");
    }
    glossary::check_sentence(&project, "why", why)?;
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
    why: &str,
    expect: impl Into<Option<u64>>,
) -> Result<Plan> {
    let project = Project::load(&ctx.root, slug)?;
    glossary::check_sentence(&project, "why", why)?;
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
                "goal": "",
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
        if !step.rounds.is_empty() {
            refs.push_str(&format!(" rounds {}", step.rounds.join(", ")));
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
    let mut changed = false;
    for step in &mut plan.steps {
        let state = derive_state(project, step);
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
fn derive_state(project: &Project, step: &PlanStep) -> StepState {
    let linked_tasks: Vec<_> = crate::task::list_with_errors(project)
        .0
        .into_iter()
        .filter(|task| {
            task.plan_step.as_deref() == Some(step.id.as_str()) || step.tasks.contains(&task.id)
        })
        .map(|task| crate::task::view(project, task))
        .filter(|view| view.state != crate::task::State::Dropped)
        .collect();
    if linked_tasks.is_empty() && step.threads.is_empty() && step.rounds.is_empty() {
        return StepState::Left;
    }
    let mut all_satisfied = true;
    let mut any_started = false;
    for view in linked_tasks {
        if !view.terminal(project) {
            all_satisfied = false;
        }
        if view.state != crate::task::State::Open {
            any_started = true;
        }
    }
    for id in &step.threads {
        let carrying = threads::carrying_rounds(project, id);
        let exists = thread::load(project, id).is_ok();
        let satisfied =
            !carrying.is_empty() && carrying.iter().all(|r| threads::round_landed(project, r));
        if !satisfied {
            all_satisfied = false;
        }
        if exists || !carrying.is_empty() || satisfied {
            any_started = true;
        }
    }
    for id in &step.rounds {
        let exists = round::load(project, id).is_ok();
        let landed = threads::round_landed(project, id);
        if !landed {
            all_satisfied = false;
        }
        if exists || landed {
            any_started = true;
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
    use crate::contracts::MergePhase;
    use crate::round::testkit::{Fx, fixture, git};

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

    fn open_r1(fx: &Fx, plain: &str) {
        crate::round::open(
            &fx.world.ctx(),
            "demo",
            crate::round::OpenArgs {
                round: "r1".into(),
                branch: "main".into(),
                plain: Some(plain.to_string()),
                repo: Some(fx.repo.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
    }

    /// Drive a real review and merge, so the plan sees a landed carrying round
    /// rather than a hand-written merge record.
    fn land_round(fx: &Fx, round: &str, lanes: &[(String, String)]) {
        let ctx = fx.world.ctx();
        let o = crate::round::review(&ctx, "demo", round).unwrap();
        let wt = o.worktree;
        for (_, sha) in lanes {
            git(&wt, &["merge", "-q", "--no-edit", sha]);
        }
        let c = git(&wt, &["rev-parse", "HEAD"]);
        let record = crate::round::load(&fx.project, round).unwrap();
        let front = format!(
            "+++\nverdict = \"MERGE\"\nround = \"{round}\"\ncandidate = \"{c}\"\nmanifest_hash = \"{}\"\npolicy_hash = \"{}\"\ngates = []\n+++\n\nAll gates pass.\n",
            record.manifest_hash.clone().unwrap(),
            record.policy_hash
        );
        let reviewer = fx.thread("Reviewer");
        fx.seal_done(&reviewer, 1, 1, &c, &front);
        crate::round::bind_reviewer(&ctx, "demo", round, &reviewer).unwrap();
        let out = crate::round::merge(&ctx, "demo", round, None).unwrap();
        assert!(matches!(
            out,
            crate::round::MergeOutcome::Checkpointed { .. }
        ));
    }

    #[test]
    fn the_seven_result_sentences_pass_an_empty_registry_check() {
        let g = crate::plain::Glossary::default();
        for (kind, sentence) in crate::contracts::PLAN_KINDS {
            let r = crate::plain::check(sentence, &g);
            assert!(r.passed(), "{kind}: {:?}", r.violations);
            assert_eq!(plan_kind_sentence(kind), Some(*sentence));
        }
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
    fn a_normal_set_keeps_zero_steps_valid_but_refuses_bad_values() {
        let fx = fixture();
        let e = format!(
            "{:#}",
            set(&fx.world.ctx(), "demo", "gadget", "It does a thing.", 0).unwrap_err()
        );
        assert!(e.starts_with("plan_kind"), "{e}");
        let e = format!(
            "{:#}",
            set(
                &fx.world.ctx(),
                "demo",
                "screen",
                "Run the zorbulate gate now.",
                0
            )
            .unwrap_err()
        );
        assert!(e.starts_with("plain_refused"), "{e}");
        assert!(load(&fx.project).unwrap().is_none());
    }

    #[test]
    fn at_most_seven_steps_and_removal_never_reuses_an_identifier() {
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
        let e = format!(
            "{:#}",
            step_add(&fx.world.ctx(), "demo", "One too many.", vec![], expect).unwrap_err()
        );
        assert!(e.starts_with("plan_steps"), "{e}");
        let plan =
            step_remove(&fx.world.ctx(), "demo", "s-3", "It is not needed.", expect).unwrap();
        assert_eq!(plan.steps.len(), 6);
        let plan = add(&fx, "A replacement step.", plan.revision);
        assert!(plan.steps.iter().any(|s| s.id == "s-8"), "{:?}", plan.steps);
        assert!(!plan.steps.iter().any(|s| s.id == "s-3"));
    }

    #[test]
    fn removing_or_unlinking_requires_a_checked_why() {
        let fx = fixture();
        set(
            &fx.world.ctx(),
            "demo",
            "screen",
            "It shows pretend trades.",
            0,
        )
        .unwrap();
        add(&fx, "A step to drop.", 1);
        let e = format!(
            "{:#}",
            step_remove(
                &fx.world.ctx(),
                "demo",
                "s-1",
                "Because the zorbulate is gone.",
                2
            )
            .unwrap_err()
        );
        assert!(e.starts_with("plain_refused"), "{e}");
        // The step is still there; nothing was removed.
        assert_eq!(load(&fx.project).unwrap().unwrap().steps.len(), 1);
    }

    #[test]
    fn a_missing_card_creates_on_expect_zero() {
        let fx = fixture();
        let plan = add(&fx, "Take the first step.", 0);
        assert_eq!(plan.revision, 1);
        assert_eq!(plan.steps[0].id, "s-1");
    }

    fn state(fx: &Fx) -> StepState {
        load(&fx.project).unwrap().unwrap().steps[0].state
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
    fn new_links_use_tasks_and_one_task_can_belong_to_multiple_steps() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let task = crate::task::Task {
            id: "job-0001".into(),
            title: "Ship the checked change.".into(),
            authority: vec!["request:q-1".into()],
            acceptance: vec!["The change lands.".into()],
            created: "2026-09-22T00:00:00Z".into(),
            ..crate::task::Task::default()
        };
        let task_dir = fx.project.state_dir().join("tasks");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(
            task_dir.join("job-0001.toml"),
            toml::to_string(&task).unwrap(),
        )
        .unwrap();

        set(&ctx, "demo", "screen", "It shows pretend trades.", 0).unwrap();
        let first = step_add(
            &ctx,
            "demo",
            "Build the first part.",
            vec![task.id.clone()],
            1,
        )
        .unwrap();
        let second = step_add(&ctx, "demo", "Build the second part.", vec![], 2).unwrap();
        let linked = step_link(&ctx, "demo", "s-2", vec![task.id.clone()], 3).unwrap();
        assert_eq!(first.steps[0].tasks[0], task.id);
        assert_eq!(second.steps[1].tasks, Vec::<String>::new());
        assert_eq!(linked.steps[0].tasks[0], task.id);
        assert_eq!(linked.steps[1].tasks[0], task.id);
        assert!(linked.steps.iter().all(|step| step.threads.is_empty()));
        assert!(linked.steps.iter().all(|step| step.rounds.is_empty()));

        let unlinked = step_unlink(
            &ctx,
            "demo",
            "s-1",
            vec![task.id.clone()],
            "The second step owns it.",
            4,
        )
        .unwrap();
        assert!(unlinked.steps[0].tasks.is_empty());
        assert_eq!(unlinked.steps[1].tasks, [task.id]);
    }

    #[test]
    fn states_follow_required_work_and_reopen_on_rework() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        set(&ctx, "demo", "screen", "It shows pretend trades.", 0).unwrap();
        let (lane, sha) = fx.lane(1);
        add(&fx, "Landed by the lane.", 1);
        let linked = link_historical_threads(&fx, vec![lane.clone()], 2);
        // Binding changes refresh the projection in the same committed plan.
        assert_eq!(linked.revision, 3);
        assert_eq!(linked.steps[0].state, StepState::Running);
        assert_eq!(
            sync(&ctx, "demo").unwrap(),
            SyncOutcome::Unchanged { revision: 3 }
        );
        assert_eq!(state(&fx), StepState::Running);

        // A plain done is not enough; only the merged round completes it.
        open_r1(&fx, "The first round lands the shared types.");
        crate::round::admit(&ctx, "demo", "r1", &lane).unwrap();
        fx.seal_done(&lane, 1, 1, &sha, "# report\n");
        let _ = sync(&ctx, "demo").unwrap();
        assert_eq!(state(&fx), StepState::Running);

        land_round(&fx, "r1", &[(lane.clone(), sha.clone())]);
        assert!(
            MergePhase::Checkpointed
                == crate::round::read_merge(&fx.project, "r1")
                    .unwrap()
                    .unwrap()
                    .phase
        );
        let _ = sync(&ctx, "demo").unwrap();
        assert_eq!(state(&fx), StepState::Done);

        // Adding required rework (a second carrying round) reopens it.
        crate::round::open(
            &ctx,
            "demo",
            crate::round::OpenArgs {
                round: "r2".into(),
                branch: "main".into(),
                plain: Some("The second round lands the shared types too.".into()),
                repo: Some(fx.repo.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        // Already-landed work is refused: rework needs a newer attempt, so
        // restart the lane before admitting it to the carrying round.
        fx.set_attempt(&lane, 2);
        crate::round::admit(&ctx, "demo", "r2", &lane).unwrap();
        let _ = sync(&ctx, "demo").unwrap();
        assert_eq!(state(&fx), StepState::Running);
    }

    #[test]
    fn an_abandoned_round_releases_a_lane_that_lands_in_a_later_round() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        set(&ctx, "demo", "screen", "It shows pretend trades.", 0).unwrap();
        let (lane, sha) = fx.lane(1);
        add(&fx, "Land the lane once.", 1);
        link_historical_threads(&fx, vec![lane.clone()], 2);

        open_r1(&fx, "The first round tries to land the lane.");
        crate::round::admit(&ctx, "demo", "r1", &lane).unwrap();
        fx.seal_done(&lane, 1, 1, &sha, "# report\n");
        crate::round::cancel(&ctx, "demo", "r1", "the review cannot proceed").unwrap();

        crate::round::open(
            &ctx,
            "demo",
            crate::round::OpenArgs {
                round: "r2".into(),
                branch: "main".into(),
                plain: Some("The later round lands the lane.".into()),
                repo: Some(fx.repo.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        crate::round::admit(&ctx, "demo", "r2", &lane).unwrap();
        assert_eq!(threads::carrying_rounds(&fx.project, &lane), ["r2"]);

        land_round(&fx, "r2", &[(lane, sha)]);
        let _ = sync(&ctx, "demo").unwrap();
        assert_eq!(state(&fx), StepState::Done);
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
