//! Read-only activity alongside (never instead of) the plan's derived marks.
use crate::contracts::{Plan, PlanStep, StepState};
use crate::project::Project;
use crate::review::Review;
use crate::runner::Runner;
use crate::task::EvidenceSnapshot;
use crate::thread::{Group, Status, Thread};
use crate::threads::Row;
use jiff::Timestamp;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

fn time(text: &str) -> Option<Timestamp> {
    text.parse().ok()
}

/// Per-read cache only: one reflog read per repository/integration, regardless
/// of how many historical reviews or steps share it. Nothing is written back.
struct Reflogs<'a> {
    runner: Option<&'a dyn Runner>,
    refs: BTreeMap<(String, String), BTreeMap<String, Timestamp>>,
}

impl Reflogs<'_> {
    fn merged_at(&mut self, review: &Review, lane: &Thread) -> Option<Timestamp> {
        if let Some(at) = time(&review.merged_at) {
            return Some(at);
        }
        let runner = self.runner?;
        let dates = self
            .refs
            .entry((review.repo.clone(), review.integration.clone()))
            .or_insert_with(|| {
                let reference = format!("refs/heads/{}", review.integration);
                let output = crate::repo::Git::new(runner, &review.repo)
                    .with_timeout(Duration::from_secs(5))
                    .run(&[
                        "reflog",
                        "show",
                        "--date=iso-strict",
                        "--format=%H%x09%gD",
                        &reference,
                        "--",
                    ])
                    .unwrap_or_default();
                let mut dates: BTreeMap<String, Timestamp> = BTreeMap::new();
                for line in output.lines() {
                    let Some((sha, selector)) = line.split_once('\t') else {
                        continue;
                    };
                    let Some((_, date)) = selector.rsplit_once("@{") else {
                        continue;
                    };
                    let Some(at) = date.strip_suffix('}').and_then(time) else {
                        continue;
                    };
                    // A later reset back to the same commit does not make an
                    // older landing recent. This is the ref-update time, not %ct.
                    dates
                        .entry(sha.to_string())
                        .and_modify(|old| *old = (*old).min(at))
                        .or_insert(at);
                }
                dates
            });
        let merged = (lane.merged_review == review.id).then_some(lane.merged_sha.as_str());
        merged.and_then(|sha| dates.get(sha).copied()).or_else(|| {
            review
                .verdict
                .as_ref()
                .and_then(|verdict| dates.get(&verdict.candidate).copied())
        })
    }
}

fn landed(
    lane: &Thread,
    evidence: &EvidenceSnapshot,
    reviews: &[Review],
    reflogs: &mut Reflogs<'_>,
) -> Option<Timestamp> {
    if !evidence.lane_done(lane) {
        return None;
    }
    if let Some(review) = crate::review::lane_review_from(reviews, lane) {
        // Recent work is dated by the merge. Installation remains a separate
        // fact; never turn a recovered merge time into an installed_at value.
        return reflogs.merged_at(review, lane);
    }
    // A historical merge without dated landing evidence is not today's news.
    if !lane.merged_sha.is_empty() {
        return None;
    }
    crate::events::latest_done_event(evidence.events(), &lane.id, lane.attempt.max(1))
        .and_then(|event| time(&event.created))
}

#[derive(Default)]
struct Dates {
    done: Option<Timestamp>,
    running: Option<Timestamp>,
    active: bool,
}

fn step_dates(
    step: &PlanStep,
    evidence: &EvidenceSnapshot,
    reviews: &[Review],
    rows: &[Row],
    recent: &mut Vec<Value>,
    running: &mut Vec<Value>,
    reflogs: &mut Reflogs<'_>,
) -> Dates {
    let (tasks, threads) = crate::plan::bindings(step, evidence);
    let mut finished = Vec::new();
    let mut started = Vec::new();
    let mut active = false;
    let mut lanes = threads;
    for id in tasks {
        if let Some(task) = evidence.tasks.iter().find(|task| task.id == id)
            && task.dropped.is_empty()
        {
            let installed = task
                .installed
                .iter()
                .filter_map(|fact| time(&fact.at))
                .min();
            let reviewed = task
                .attempts
                .last()
                .and_then(|id| evidence.lanes.get(id))
                .filter(|lane| crate::review::lane_review_from(reviews, lane).is_some());
            if let Some(lane) = reviewed {
                lanes.push(lane.id.clone());
            } else if let Some(at) = installed {
                finished.push(Some(at));
            } else if let Some(id) = task.attempts.last() {
                lanes.push(id.clone());
            } else {
                finished.push(None);
            }
        }
    }
    lanes.sort();
    lanes.dedup();
    for id in lanes {
        finished.push(
            evidence
                .lanes
                .get(&id)
                .and_then(|lane| landed(lane, evidence, reviews, reflogs)),
        );
        if let Some(row) = rows.iter().find(|row| row.thread.id == id)
            && row.group == Group::Working
            && row.thread.status == Status::Open
            && !super::process_unknown(row)
            && row.thread.provider_wait_started.is_empty()
            && !row.thread.recovery_pending
            && !row.thread.prompt_pending
            && row.thread.startup_wait_started.is_empty()
        {
            active = true;
            // The current working stretch, not the lane's birth (which survives retries).
            if let Some(at) = time(&row.thread.last_state_change) {
                started.push(at);
            }
        }
    }
    for sub in &step.subtasks {
        let dates = step_dates(sub, evidence, reviews, rows, recent, running, reflogs);
        finished.push(dates.done);
        started.extend(dates.running);
        active |= dates.active;
    }
    let done = if step.state == StepState::Done && !finished.is_empty() {
        finished
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .and_then(|dates| dates.into_iter().max())
    } else {
        None
    };
    let running_at = if step.state != StepState::Done {
        started.into_iter().min()
    } else {
        None
    };
    if let Some(at) = done {
        recent.push(json!({"text":step.text, "at":at.to_string()}));
    }
    active &= step.state != StepState::Done;
    if active {
        running.push(
            json!({"text":step.text, "at":running_at.map(|at| at.to_string()).unwrap_or_default()}),
        );
    }
    Dates {
        done,
        running: running_at,
        active,
    }
}

pub(super) fn activity(
    plan: &Plan,
    evidence: &EvidenceSnapshot,
    reviews: &[Review],
    rows: &[Row],
    runner: Option<&dyn Runner>,
) -> Value {
    let mut reflogs = Reflogs {
        runner,
        refs: BTreeMap::new(),
    };
    let mut recent = Vec::new();
    let mut running = Vec::new();
    for step in &plan.steps {
        step_dates(
            step,
            evidence,
            reviews,
            rows,
            &mut recent,
            &mut running,
            &mut reflogs,
        );
    }
    json!({"recent":recent, "running":running})
}

/// The harness review, not this project's most recent application review.
/// Missing historical timestamps remain unknown; file mtimes are not landing facts.
pub(super) fn harness(project: &Project) -> Value {
    let dir = project.dir();
    let root = dir.parent().unwrap_or(&dir);
    let Ok(harness) = Project::load(root, "adeherdr") else {
        return json!({});
    };
    let Ok(reviews) = crate::review::list(&harness) else {
        return json!({"check":"self-check unavailable"});
    };
    let latest = reviews
        .iter()
        .filter(|review| review.install_required && review.install && review.fast_forward)
        .max_by_key(|review| time(&review.installed_at));
    let Some(review) = latest else {
        return json!({});
    };
    // A repeated after-install check appends a new REVIEW notice. Its result,
    // not a failure from an earlier run in the same record, is the current one.
    let result = review
        .install_result
        .rfind("REVIEW ")
        .map(|start| &review.install_result[start..])
        .unwrap_or(&review.install_result);
    let check = if result.contains("FAIL") || review.install_result.starts_with("REGRESSION") {
        // Do not expose journey ids, build hashes, paths or recipes in the tab.
        "self-check failed: after-install checks did not pass"
    } else if result.contains("JOURNEY ") && review.install_result.contains("Rundown renders: OK") {
        "self-check passed"
    } else {
        "not run yet"
    };
    json!({"updated_at":review.installed_at, "check":check})
}
