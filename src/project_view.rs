//! One read-only projection of project records. Named sections are selected
//! before rendering; no consumer reads generated Markdown back as evidence.
use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::Result;
use serde_json::{Value, json};

use crate::contracts::{Event, StepState};
use crate::paths::Ctx;
use crate::project::{Project, Settings};
use crate::thread::{Group, Status};
use crate::threads::Row;

#[path = "rundown/activity.rs"]
mod activity;

#[derive(Clone)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) text: String,
}

pub(crate) struct Section {
    pub(crate) name: String,
    pub(crate) rows: Vec<Entry>,
}

impl Section {
    pub(crate) fn new(name: &str, rows: Vec<Entry>) -> Self {
        Self {
            name: name.into(),
            rows,
        }
    }

    pub(crate) fn chars(&self) -> usize {
        self.name.chars().count()
            + 6
            + if self.rows.is_empty() {
                6
            } else {
                self.rows
                    .iter()
                    .map(|row| {
                        row.text.chars().count()
                            + if row.id.is_empty() {
                                1
                            } else {
                                row.id.chars().count() + 6
                            }
                    })
                    .sum::<usize>()
            }
    }

    pub(crate) fn render(&self) -> String {
        let mut out = format!("\n## {}\n\n", self.name);
        if self.rows.is_empty() {
            out.push_str("None.\n");
        }
        for row in &self.rows {
            if row.id.is_empty() {
                let _ = writeln!(out, "{}", row.text);
            } else {
                let _ = writeln!(out, "- `{}` {}", row.id, row.text);
            }
        }
        out
    }
}

fn entry(id: impl Into<String>, text: impl Into<String>) -> Entry {
    Entry {
        id: id.into(),
        text: text.into(),
    }
}

pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn installation_pending(thread: &crate::thread::Thread, reviews: &[crate::review::Review]) -> bool {
    if let Some(review) = reviews.iter().find(|r| r.id == thread.merged_review) {
        review.fast_forward && review.install_required && !review.install
    } else {
        !thread.merged_sha.is_empty()
            && thread.historical_install_required
            && thread.installed_sha.is_empty()
    }
}

fn process_unknown(row: &Row) -> bool {
    // A first readiness check has no process observation yet by design.
    // Actual connection errors still remain unknown.
    if row.group == Group::Working
        && (row.thread.status == Status::Starting || !row.thread.startup_wait_started.is_empty())
        && row.thread.observation_error.is_empty()
        && !row.note.contains("session unreachable")
    {
        return false;
    }
    !row.thread.recovery_pending
        && (row.group == Group::Unknown
            || row.note.contains("agent state unknown")
            || row.note.contains("process not queried")
            || row.note.contains("session unreachable")
            || (row.thread.is_remote()
                && (row.thread.last_observed.is_empty()
                    || !row.thread.observation_error.is_empty())))
}

/// Seal and process evidence stay separate. A missing pane never invalidates
/// a wait, and a wait never licenses an automatic restart.
fn action_row(
    project: &Project,
    row: &mut Row,
    events: &[Event],
    reviews: &[crate::review::Review],
    tasks: &[crate::task::Task],
) {
    let t = &row.thread;
    if t.status == Status::Resolved {
        return;
    }
    let event = crate::events::latest_event(events, &t.id, t.attempt.max(1));
    if let Some(task) = tasks
        .iter()
        .find(|task| !task.dropped.is_empty() && task.attempts.contains(&t.id))
        && t.merged_sha.is_empty()
        && crate::review::lane_review_from(reviews, t).is_none()
    {
        if crate::task::sealed_unlanded(t, events, reviews) {
            row.group = Group::WaitingOnYou;
            row.note = format!(
                "task {} dropped; {} seal retained; not landed; retire with ha thread resolve {} {}; {}",
                task.id,
                if event.is_some_and(|e| e.payload.done.is_some()) {
                    "done"
                } else {
                    "waiting"
                },
                project.slug,
                t.id,
                row.note
            );
        } else {
            row.note = format!(
                "task {} dropped; lane not retired; cancel with ha thread cancel {} {} --reason \"task {} dropped\"; {}",
                task.id, project.slug, t.id, task.id, row.note
            );
        }
        return;
    }
    let unknown = process_unknown(row);
    let absent = row.note.starts_with("process gone:")
        || (!unknown && t.is_remote() && row.note.starts_with("no agent;"));
    if let Some(event) = event {
        if let Some(failed) = &event.payload.failed {
            row.group = Group::WaitingOnYou;
            row.note = format!(
                "failed seal retained: {}; {}",
                one_line(&failed.text),
                row.note
            );
            return;
        }
        if let Some(wait) = event
            .payload
            .waiting
            .as_ref()
            .filter(|_| event.id != t.answered_waiting_event)
        {
            row.group = Group::WaitingOnYou;
            row.note = format!(
                "waiting seal retained: {}; {}; when input is ready, {}",
                one_line(&wait.text),
                if absent {
                    "agent gone, waiting seal kept (process absent)"
                } else {
                    &row.note
                },
                if absent {
                    format!(
                        "retry with a continuation reason: ha thread retry {} {} --reason \"input is ready; continue preserved work\"",
                        project.slug, t.id
                    )
                } else if unknown {
                    "process unknown; check the connection before choosing prompt or retry".into()
                } else {
                    format!("ha thread prompt {} {} <continuation>", project.slug, t.id)
                }
            );
            return;
        }
        if event.payload.done.is_some()
            && !crate::threads::follow_up_pending_for_seal(t, Some(event))
        {
            row.group =
                if t.parked || row.group == Group::Parked || absent || !t.merged_sha.is_empty() {
                    Group::Parked
                } else {
                    Group::ReadyForReview
                };
            let review = reviews.iter().find(|r| r.id == t.merged_review);
            let landing = if t.merged_sha.is_empty() {
                "awaiting review"
            } else if installation_pending(t, reviews) {
                "merged; awaiting installation"
            } else if !t.installed_sha.is_empty() || review.is_some_and(|r| r.install) {
                "merged; installed"
            } else if review.is_some_and(|r| !r.install_required) {
                "merged; install not required"
            } else {
                "merged; install not recorded"
            };
            row.note = format!(
                "done seal retained; {landing}; {}; continue with ha thread prompt {} {} <follow-up>",
                row.note, project.slug, t.id
            );
            return;
        }
    }
    if absent && !t.recovery_pending {
        row.group = Group::WaitingOnYou;
        row.note = format!(
            "process absent; unsealed attempt — {}",
            crate::threads::retry_command(&project.slug, &t.id)
        );
    } else if unknown && t.status != Status::Failed {
        row.group = Group::Unknown;
    } else if row.group == Group::ReadyForReview
        && event
            .and_then(|event| event.payload.done.as_ref())
            .is_none()
    {
        row.group = Group::Idle;
        row.note = format!("report draft; completion not sealed; {}", row.note);
    }
}

/// Only explicit personal requests and browser authentication belong here;
/// a technical 'Needs attention' group is not evidence that Rolf is needed.
fn personal_wait(text: &str) -> bool {
    let text = text.to_lowercase();
    text.starts_with("rolf")
        || [
            "need rolf",
            "only rolf",
            "waiting for rolf",
            "waiting on rolf",
            "awaiting rolf",
            "rolf decides",
            "needs you",
            "waiting on you",
            "need approval",
            "approval needed",
            "approval required",
            "need your approval",
            "awaiting approval",
            "waiting for approval",
            "approval from",
            "please approve",
        ]
        .iter()
        .any(|phrase| text.contains(phrase))
        || text.contains("login")
        || text.contains("log in")
        || text.contains("sign in")
        || text.contains("signed in")
}

pub(crate) struct View {
    pub(crate) sections: Vec<Section>,
    pub(crate) lanes: Vec<Row>,
    pub(crate) messages: BTreeMap<String, String>,
    pub(crate) events: BTreeMap<String, String>,
    pub(crate) plan: Value,
    title: String,
    pub(crate) needs_you: Vec<String>,
    needs_you_items: Vec<String>,
    activity: Value,
    harness: Value,
    reviews: Vec<crate::review::Review>,
    unreadable_lanes: usize,
    read_errors: Vec<String>,
}

impl View {
    pub(crate) fn load(ctx: &Ctx, project: &Project, history: Option<usize>) -> Result<Self> {
        let (settings, _) = project.read_project_md()?;
        let mut view = Self::capture_with_runner(
            project,
            &settings,
            Some(crate::threads::rows(ctx, project)),
            history,
            Some(ctx.runner),
        );
        if !settings.repos.is_empty() {
            let default_review_machine = crate::launch::parse_launch_config(&ctx.config_dir)
                .map(|config| config.dispatch.machine)
                .unwrap_or_default();
            view.sections.push(Section::new(
                "Repositories",
                settings
                    .repos
                    .iter()
                    .map(|repo| {
                        let snapshot = if let Some(machine) = &repo.machine {
                            format!("{} on {machine} (local git status not queried)", repo.path)
                        } else {
                            crate::coordinator::repo_snapshot(ctx.runner, &repo.path)
                        };
                        let review_machine = match &repo.review_machine {
                            Some(machine) => format!("{machine} (explicit; no fallback)"),
                            None if default_review_machine.is_empty()
                                || default_review_machine == crate::contracts::MACHINE_LOCAL =>
                            {
                                "local (default)".into()
                            }
                            None => format!(
                                "{default_review_machine} first, local if unavailable (default)"
                            ),
                        };
                        entry("", format!("{snapshot}; review machine: {review_machine}"))
                    })
                    .collect(),
            ));
        }
        let recipes = match crate::launch::parse_launch_config(&ctx.config_dir) {
            Ok(config) => crate::launch::context_recipe_lines(&config),
            Err(error) => vec![format!("config-error: {error:#}")],
        };
        view.sections.push(Section::new(
            "Recipes",
            recipes.into_iter().map(|line| entry("", line)).collect(),
        ));
        Ok(view)
    }

    pub(crate) fn capture(
        project: &Project,
        settings: &Settings,
        observed: Option<Vec<Row>>,
        history: Option<usize>,
    ) -> Self {
        Self::capture_with_runner(project, settings, observed, history, None)
    }

    // Record-only page generation needs no subprocesses. Interactive reads can
    // also recover historical landing dates from the repository's reflog.
    fn capture_with_runner(
        project: &Project,
        settings: &Settings,
        observed: Option<Vec<Row>>,
        history: Option<usize>,
        runner: Option<&dyn crate::runner::Runner>,
    ) -> Self {
        let evidence = crate::task::EvidenceSnapshot::load(project);
        let events = evidence.events();
        let (reviews, review_error) = match crate::review::list(project) {
            Ok(rows) => (rows, None),
            Err(error) => (vec![], Some(format!("review read error: {error:#}"))),
        };
        let lane_errors = crate::thread::read_errors(project);
        let unreadable_lanes = lane_errors.len();
        let mut read_errors: Vec<String> = lane_errors
            .iter()
            .map(|error| {
                format!("Some work records could not be read; unreadable record: {error:#}")
            })
            .collect();
        let seal_errors: Vec<String> = if evidence.readable() {
            Vec::new()
        } else {
            crate::events::read_errors(project)
                .into_iter()
                .map(|error| format!("unreadable record (seal): {error:#}"))
                .collect()
        };
        read_errors.extend(seal_errors.iter().cloned());
        let mut lanes = observed.unwrap_or_else(|| {
            crate::thread::list(project)
                .into_iter()
                .map(|thread| Row {
                    group: crate::thread::recorded_group(&thread, jiff::Timestamp::now()),
                    note: if thread.recovery_pending {
                        "starting (placement queued)".into()
                    } else {
                        format!(
                            "recorded: {}; process not queried",
                            if thread.last_state.is_empty() {
                                "unknown"
                            } else {
                                &thread.last_state
                            }
                        )
                    },
                    thread,
                })
                .collect()
        });
        for row in &mut lanes {
            action_row(project, row, events, &reviews, &evidence.tasks);
        }
        lanes.sort_by_key(|row| Group::DISPLAY_ORDER.iter().position(|g| *g == row.group));
        let mut activity = json!({});
        let mut plan = match crate::plan::load(project) {
            Ok(Some(mut plan)) => {
                let holds = crate::plan::failed_check_holds(project, &mut plan, &evidence);
                activity = activity::activity(&plan, &evidence, &reviews, &lanes, runner);
                let mut value = serde_json::to_value(&plan).expect("serializable plan");
                for step in value
                    .get_mut("steps")
                    .and_then(serde_json::Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    crate::plan::add_hold_json(step, &holds);
                    for sub in step
                        .get_mut("subtasks")
                        .and_then(serde_json::Value::as_array_mut)
                        .into_iter()
                        .flatten()
                    {
                        crate::plan::add_hold_json(sub, &holds);
                    }
                }
                value
            }
            Ok(None) => json!({"schema":1, "revision":0, "goal":settings.goal, "steps":[]}),
            Err(error) => {
                json!({"schema":1, "revision":0, "goal":settings.goal, "steps":[], "error":format!("{error:#}")})
            }
        };
        // Authored text and the existing projected states/counts are unchanged.
        plan["present"] = json!(plan["revision"].as_u64().unwrap_or(0) != 0);
        let outcome = format!(
            "{} {}",
            plan["what_you_get"].as_str().unwrap_or(""),
            plan["does"].as_str().unwrap_or("")
        );
        let mut sections = vec![Section::new(
            "Goal and what Rolf gets",
            vec![
                entry(
                    "",
                    format!(
                        "Goal: {}",
                        if settings.goal.trim().is_empty() {
                            "not written down."
                        } else {
                            settings.goal.trim()
                        }
                    ),
                ),
                entry(
                    "",
                    format!(
                        "What Rolf gets: {}",
                        if outcome.trim().is_empty() {
                            "not written down."
                        } else {
                            outcome.trim()
                        }
                    ),
                ),
            ],
        )];
        let mut status = Vec::new();
        if project
            .coordinator()
            .is_some_and(|c| !c.closed_by_rolf_at.is_empty())
        {
            status.push(entry("", "Closed by Rolf. Run `ha open` to reopen it."));
        }
        if crate::prompt::long_input_hold(project) {
            status.push(entry("", "Automated prompts have waited over 30 minutes for text in the coordinator's input line. They remain pending; finish or clear the draft when ready."));
        }
        if !status.is_empty() {
            sections.push(Section::new("Coordinator status", status));
        }
        let mut seal_rows = BTreeMap::new();
        let mut needs_you = Vec::new();
        let mut needs_you_items = Vec::new();
        let open_waits = crate::steps::goal_check::open_waits_with_evidence(project, &evidence);
        if let Some(line) = crate::steps::goal_check::attention_with_waits(project, &open_waits) {
            needs_you.push(line);
        }
        let goal_check = crate::steps::goal_check::load(project);
        if goal_check.disposition == Some(crate::steps::goal_check::Disposition::NeedsRolf)
            && !goal_check.evidence.is_empty()
        {
            needs_you_items.push(goal_check.evidence.clone());
        }
        for (wait, _) in &open_waits {
            if let crate::steps::goal_check::Disposition::Wait {
                party, condition, ..
            } = wait
                && party.eq_ignore_ascii_case("Rolf")
            {
                needs_you_items.push(condition.clone());
            }
        }
        // Retired explicit waits must not fall back to guessing responsibility
        // from an old lane's text. Open waits override their historical entries.
        let explicit_parties: BTreeMap<_, _> = goal_check
            .waits.into_iter().map(|(wait, _)| (wait, false))
            .chain(open_waits.iter().cloned().map(|(wait, _)| {
                let personal = matches!(&wait, crate::steps::goal_check::Disposition::Wait { party, .. }
                    if party.eq_ignore_ascii_case("Rolf"));
                (wait, personal)
            }))
            .flat_map(|(wait, personal)| match wait {
                crate::steps::goal_check::Disposition::Wait { tasks, .. } => tasks
                    .into_iter()
                    .flat_map(move |id| {
                        crate::task::load(project, &id)
                            .ok()
                            .into_iter()
                            .flat_map(|task| task.attempts)
                            .map(|id| (id, personal))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect();
        let mut work = Vec::new();
        for row in lanes
            .iter()
            .filter(|row| history == Some(usize::MAX) || row.group != Group::Resolved)
        {
            let t = &row.thread;
            let personal_wait = |text: &str| {
                explicit_parties
                    .get(&t.id)
                    .map_or_else(|| personal_wait(text), |personal| *personal)
            };
            if (t.status == Status::Failed
                || !t.provider_wait_started.is_empty()
                || row.group == Group::WaitingOnYou)
                && personal_wait(&t.error)
            {
                needs_you_items.push(one_line(&t.error));
                needs_you.push(format!("{}: {}", t.id, one_line(&t.error)));
            }
            let mut detail = format!(
                "[{}] {} — {}",
                row.group.label(),
                one_line(&t.title),
                row.note
            );
            if t.follow_ups.iter().any(|f| {
                f.attempt == t.attempt.max(1) && f.state == crate::thread::FollowUpState::Queued
            }) {
                detail.push_str(" — follow-up queued");
            }
            if !t.error.is_empty() {
                let _ = write!(detail, "\n  {}: {}", t.failure_class.plain(), t.error);
            }
            if t.recovery_pending && t.attempt > 1 {
                detail.push_str("\n  automatic retry selected; wait for startup");
            } else if t.status == Status::Failed
                && let Some(notice) = t
                    .start_notices
                    .iter()
                    .rev()
                    .find(|n| n.line.contains(" — next: "))
            {
                let _ = write!(detail, "\n  {}", notice.line);
            }
            if !t.repo.is_empty() || !t.branch.is_empty() {
                let _ = write!(detail, "\n  repo={} branch={}", t.repo, t.branch);
            }
            if !t.pane_id.is_empty() || !t.machine.is_empty() {
                let _ = write!(detail, "\n  pane={} machine={}", t.pane_id, t.machine);
            }
            if !t.launch.recipe_basis.is_empty() {
                let _ = write!(
                    detail,
                    "\n  recipe={} via {}: {:?}",
                    t.launch.recipe_id, t.launch.recipe_request, t.launch.recipe_basis
                );
            }
            let completion = crate::events::latest_event(events, &t.id, t.attempt.max(1));
            if let Some(event) = completion.filter(|_| t.status != Status::Resolved) {
                let seal = if let Some(done) = &event.payload.done {
                    let report = crate::thread::sealed_report_reference(project, t)
                        .unwrap_or_else(|| format!(".state/artifacts/{} (missing)", done.artifact));
                    Some(format!("done: {} report={report}", done.sha))
                } else if let Some(wait) = event
                    .payload
                    .waiting
                    .as_ref()
                    .filter(|_| event.id != t.answered_waiting_event)
                {
                    if personal_wait(&wait.text) {
                        needs_you_items.push(one_line(&wait.text));
                        needs_you.push(format!("{}: {}", t.id, one_line(&wait.text)));
                    }
                    Some(format!(
                        "waiting — {}{}: {}",
                        wait.class.plain(),
                        wait.provider_kind
                            .as_ref()
                            .map(|k| format!(" ({k})"))
                            .unwrap_or_default(),
                        wait.text
                    ))
                } else {
                    event.payload.failed.as_ref().map(|failed| {
                        if personal_wait(&failed.text) {
                            needs_you_items.push(one_line(&failed.text));
                            needs_you.push(format!("{}: {}", t.id, one_line(&failed.text)));
                        }
                        format!(
                            "failed — {}{}: {}",
                            failed.class.plain(),
                            failed
                                .provider_kind
                                .as_ref()
                                .map(|k| format!(" ({k})"))
                                .unwrap_or_default(),
                            failed.text
                        )
                    })
                };
                if let Some(seal) = seal {
                    seal_rows.insert(
                        event.id.clone(),
                        format!("{} {seal} event={}", t.id, event.id),
                    );
                }
            }
            let sealed = completion
                .and_then(|e| e.payload.done.as_ref())
                .is_some_and(|d| d.artifact == t.report_hash)
                && crate::thread::sealed_report_path(project, t).is_some();
            if !t.report_hash.is_empty()
                && !sealed
                && let Some(report) = crate::thread::report_reference(project, t)
            {
                let _ = write!(detail, "\n  report draft: {report} (not completion)");
            }
            if t.lineage_mismatch {
                detail.push_str(
                    "\n  lineage-mismatch: live process identity differs; parent not repaired",
                );
            }
            for note in &t.copy_notes {
                let _ = write!(detail, "\n  copy incomplete: {note}");
            }
            work.push(entry(&t.id, detail));
        }
        // Superseded/answered seals are history, not current actions. Omission
        // never receipts them; --full can still show their immutable evidence.
        if history == Some(usize::MAX) {
            for event in events {
                seal_rows.entry(event.id.clone()).or_insert_with(|| {
                    let text = crate::events::typed_line(project, event)
                        .unwrap_or_else(|error| format!("seal read error: {error:#}"));
                    format!("Historical seal: {text}")
                });
            }
        }
        work.extend(lane_errors.into_iter().enumerate().map(|(i, error)| {
            entry(
                format!("lane-error:{i}"),
                format!(
                    "[Unknown] Unreadable lane: {}",
                    one_line(&format!("{error:#}"))
                ),
            )
        }));
        if let Some(error) = plan["error"].as_str() {
            read_errors.push(error.into());
            work.push(entry("plan-error", error));
        }
        work.extend(
            seal_errors
                .iter()
                .enumerate()
                .map(|(i, error)| entry(format!("seal-error:{i}"), error)),
        );
        sections.push(Section::new("Current work", work));
        sections.push(Section::new(
            "Seals",
            seal_rows.iter().map(|(id, text)| entry(id, text)).collect(),
        ));
        let mut review_rows: Vec<_> = reviews
            .iter()
            .filter(|r| history == Some(usize::MAX) || !r.phase.closed())
            .map(|r| {
                entry(
                    r.id.clone(),
                    format!(
                        "[{:?}] {} lanes — reviewer {}; {}{}{}",
                        r.phase,
                        r.members.len(),
                        r.reviewer.as_deref().unwrap_or("pending"),
                        r.landing_summary(),
                        r.gates_summary(),
                        if r.attention.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", r.attention)
                        }
                    ),
                )
            })
            .collect();
        match crate::review::current_holds(project) {
            Ok(holds) => review_rows.extend(holds.into_iter().map(|(repo, reason)| {
                entry(format!("hold:{repo}"), format!("PILE hold: {reason}"))
            })),
            Err(error) => review_rows.push(entry(
                "pile-holds:error",
                format!("pile-holds read error: {error:#}"),
            )),
        }
        if let Some(error) = review_error {
            review_rows.push(entry("review-error", error));
        }
        sections.push(Section::new("Pile reviews", review_rows));
        let (tasks, errors) = crate::task::views_with_evidence(project, &evidence);
        let open: Vec<_> = tasks
            .iter()
            .filter(|t| !t.terminal_with_evidence(project, &evidence))
            .collect();
        let mut task_rows: Vec<_> = open
            .iter()
            .map(|t| {
                entry(
                    &t.record.id,
                    format!(
                        "[{}] {} — next: {}",
                        t.state.word(),
                        one_line(&t.record.title),
                        one_line(&t.next)
                    ),
                )
            })
            .collect();
        read_errors.extend(
            errors
                .iter()
                .map(|error| format!("unreadable record (task): {error:#}")),
        );
        task_rows.extend(errors.into_iter().enumerate().map(|(i, error)| {
            entry(
                format!("task-error:{i}"),
                format!("Unreadable task: {error:#}"),
            )
        }));
        sections.push(Section::new("Open tasks", task_rows));
        // Keep recurrence visible even when the ordinary delivery task is done.
        let repairs: Vec<_> = tasks
            .iter()
            .flat_map(|task| {
                task.repairs.iter().map(|repair| {
                    entry(
                        format!("{}:{}", task.record.id, repair.cause),
                        crate::task::repair_summary(repair),
                    )
                })
            })
            .collect();
        if !repairs.is_empty() {
            sections.push(Section::new("Repair outcomes", repairs));
        }
        let mut notes = crate::note::active_rows(project);
        crate::note::sort_newest_first(&mut notes);
        for (kind, name) in [
            ("task note", "Task notes in force"),
            ("standing instruction", "Standing instructions in force"),
            ("memory", "Facts in force"),
        ] {
            sections.push(Section::new(
                name,
                notes
                    .iter()
                    .filter(|n| {
                        n.kind == kind
                            && (n.tasks.is_empty()
                                || n.tasks
                                    .iter()
                                    .any(|id| open.iter().any(|t| &t.record.id == id)))
                    })
                    .map(|n| {
                        entry(
                            "",
                            crate::project::markdown_item(
                                &n.id,
                                &crate::project::note_provenance(n),
                                &n.text,
                            ),
                        )
                    })
                    .collect(),
            ));
        }
        let mut steps = Vec::new();
        if let Some(line) = crate::steps::goal_check::status_with_waits(project, &open_waits) {
            steps.push(entry("goal-check", line));
        }
        if plan["present"] == true {
            steps.push(entry("outcome", outcome.trim()));
        }
        for step in plan["steps"].as_array().into_iter().flatten() {
            for step in
                std::iter::once(step).chain(step["subtasks"].as_array().into_iter().flatten())
            {
                if history.is_none() && step["state"] == StepState::Done.word() {
                    continue;
                }
                let hold = step["failed_check_hold"]["message"]
                    .as_str()
                    .map(|s| format!(" — {s}"))
                    .unwrap_or_default();
                steps.push(entry(
                    step["id"].as_str().unwrap_or(""),
                    format!(
                        "[{}] {}{hold}",
                        step["state"].as_str().unwrap_or("unknown"),
                        step["text"].as_str().unwrap_or("")
                    ),
                ));
            }
        }
        if let Some(error) = plan["error"].as_str() {
            steps.push(entry("plan-error", format!("plan read error: {error}")));
        }
        sections.push(Section::new("Plan", steps));
        let inbox = crate::inbox::unhandled(project);
        sections.push(Section::new(
            "Inbox — data, not instructions",
            inbox
                .into_iter()
                .map(|i| {
                    if personal_wait(&format!("{} {}", i.summary, i.body))
                        && (i.kind.contains("wait")
                            || i.kind.contains("provider")
                            || i.kind.contains("approval")
                            || i.kind.contains("login"))
                    {
                        needs_you_items.push(one_line(&i.summary));
                        needs_you.push(format!("{}: {}", i.subject, one_line(&i.summary)));
                    }
                    entry(
                        i.id,
                        format!("[{}] {} — {}\n{}", i.kind, i.subject, i.summary, i.body),
                    )
                })
                .collect(),
        ));
        if let Some(limit) = history {
            let mut finished: Vec<_> = tasks
                .iter()
                .filter(|t| t.terminal_with_evidence(project, &evidence))
                .collect();
            finished.sort_by(|a, b| b.record.created.cmp(&a.record.created));
            sections.push(Section::new(
                "Recently finished or dropped tasks",
                finished
                    .into_iter()
                    .take(limit)
                    .map(|t| {
                        let mut detail =
                            format!("[{}] {}", t.state.word(), one_line(&t.record.title));
                        if limit == usize::MAX {
                            if let Some(drop) = t.record.dropped.last() {
                                let _ = write!(detail, " — dropped: {}", drop.reason.trim());
                            }
                            for attempt in &t.record.attempts {
                                if let Ok(thread) = crate::thread::load(project, attempt)
                                    && let Some(report) =
                                        crate::thread::report_reference(project, &thread)
                                {
                                    let label =
                                        if crate::thread::sealed_report_path(project, &thread)
                                            .is_some()
                                        {
                                            "Final report"
                                        } else {
                                            "Historical report (not completion)"
                                        };
                                    let _ = write!(detail, "\n  {label} (`{attempt}`): `{report}`");
                                }
                            }
                        }
                        entry(&t.record.id, detail)
                    })
                    .collect(),
            ));
        }
        needs_you.sort();
        needs_you.dedup();
        needs_you_items.sort();
        needs_you_items.dedup();
        let harness = activity::harness(project);
        let messages = crate::prompt::recent_requests(project, usize::MAX)
            .into_iter()
            .collect();
        Self {
            sections,
            lanes,
            messages,
            events: seal_rows,
            plan,
            title: settings.name.clone(),
            needs_you,
            needs_you_items,
            activity,
            harness,
            reviews,
            unreadable_lanes,
            read_errors,
        }
    }

    pub(crate) fn rows(&self, names: &[&str]) -> BTreeMap<String, String> {
        self.sections
            .iter()
            .filter(|s| names.contains(&s.name.as_str()))
            .flat_map(|s| s.rows.iter())
            .filter(|r| !r.id.is_empty())
            .map(|r| (r.id.clone(), r.text.clone()))
            .collect()
    }

    pub(crate) fn render(&self, names: &[&str]) -> String {
        self.sections
            .iter()
            .filter(|s| names.is_empty() || names.contains(&s.name.as_str()))
            .map(Section::render)
            .collect()
    }

    pub(crate) fn work_summary(&self) -> String {
        let (mut running, mut starting, mut waiting, mut review, mut install, mut unknown) =
            (0, 0, 0, 0, 0, 0);
        for row in self.lanes.iter().filter(|row| row.group != Group::Resolved) {
            let t = &row.thread;
            let reachable = !process_unknown(row);
            let provider = !t.provider_wait_started.is_empty();
            if row.group == Group::Working && reachable && !provider {
                if t.status == Status::Starting
                    || t.recovery_pending
                    || t.prompt_pending
                    || !t.startup_wait_started.is_empty()
                {
                    starting += 1;
                } else if t.status == Status::Open {
                    running += 1;
                }
            }
            if matches!(row.group, Group::WaitingOnYou | Group::Idle) || provider {
                waiting += 1;
            }
            if matches!(row.group, Group::ReadyForReview | Group::Parked) && t.merged_sha.is_empty()
            {
                review += 1;
            }
            if installation_pending(t, &self.reviews) {
                install += 1;
            }
            if row.group == Group::Unknown || !reachable {
                unknown += 1;
            }
        }
        let mut work = vec![format!("{running} running"), format!("{waiting} waiting")];
        for (count, label) in [
            (starting, "starting"),
            (review, "awaiting review"),
            (install, "awaiting installation"),
            (unknown, "unknown"),
            (self.unreadable_lanes, "unreadable"),
        ] {
            if count > 0 {
                work.push(format!("{count} {label}"));
            }
        }
        work.join(" · ")
    }

    pub(crate) fn rundown(&self) -> Value {
        json!({"title":self.title, "plan":self.plan, "work":self.work_summary(), "needs_you":self.needs_you.join("; "),
            "read_error":self.read_errors.join("; "),
            "needs_you_items":self.needs_you_items, "activity":self.activity, "harness":self.harness,
            "actions":self.sections.iter().filter(|s| matches!(s.name.as_str(), "Current work" | "Pile reviews" | "Open tasks")).flat_map(|s| &s.rows).map(|r| format!("{}: {}", r.id, r.text.lines().next().unwrap_or(""))).collect::<Vec<_>>()})
    }
}
