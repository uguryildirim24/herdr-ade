use super::*;
use crate::contracts::{
    DonePayload, Event, EventPayload, Plan, PlanStep, Recipient, WaitingPayload,
};
use crate::{events, thread};

const CAUSE: &str = "D13: stale retry identity kills its new attempt";
const BOUNDARY: &str = "automatic retry launches and survives its attempt-owned ready window";
const INSTALL: &str = "2026-10-03T07:42:00Z";
const BUILD: &str = "dd498b3";

struct Case {
    world: crate::scenarios::World,
    project: Project,
    lane: thread::Thread,
}

impl Case {
    fn new() -> Self {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |lane| {
            // Named historical merged-record consumer, not a producer acceptance claim.
            lane.merged_sha = BUILD.into();
            lane.status = thread::Status::Resolved;
        });
        write(
            &project,
            &Task {
                id: "job-0001".into(),
                title: "Repair D13".into(),
                authority: vec!["request:historical".into()],
                acceptance: vec![BOUNDARY.into()],
                attempts: vec![lane.id.clone()],
                ..Default::default()
            },
        )
        .unwrap();
        Self {
            world,
            project,
            lane,
        }
    }

    fn event(&self, id: &str, lane: &str, attempt: u32, at: &str, success: bool) -> Event {
        let report =
            b"Observed automatic retry start, live new attempt, and ready-window survival.\n";
        let artifact = thread::sha256_hex(report);
        let path = events::artifact_path(&self.project, &artifact);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, report).unwrap();
        let payload = if success {
            EventPayload {
                done: Some(DonePayload {
                    artifact,
                    sha: BUILD.into(),
                    ..Default::default()
                }),
                ..Default::default()
            }
        } else {
            EventPayload {
                failed: Some(WaitingPayload {
                    text:
                        "recovery exhausted before retried agent launched; stale process identity"
                            .into(),
                    class: FailureClass::ProcessGone,
                    ..Default::default()
                }),
                ..Default::default()
            }
        };
        let event = Event {
            id: id.into(),
            op: id.into(),
            thread: lane.into(),
            attempt,
            recipient: Recipient::default(),
            created: at.into(),
            payload,
            usage: None,
        };
        events::seal_create_if_absent(&self.project, &event).unwrap();
        event
    }

    fn link(&self, ids: &[&str]) -> Result<Task> {
        record_repair(&self.project, "job-0001", RepairCommand::Link {
            cause: CAUSE.into(), boundary: BOUNDARY.into(), limits: Some("original two same-recipe retries; spend unknown".into()),
            event: ids.iter().map(|id| (*id).into()).collect(), reason: "diagnosis: retry selected a new identity but ready-window cleanup retained its predecessor".into()
        })
    }

    fn install(&self) -> Result<Task> {
        record_repair(
            &self.project,
            "job-0001",
            RepairCommand::Install {
                cause: CAUSE.into(),
                machine: "oci".into(),
                build: BUILD.into(),
                at: INSTALL.into(),
                evidence: "review-59 installation receipt: oci binary dd498b3".into(),
            },
        )
    }

    fn exercise(&self, event: &str, machine: &str, build: &str, boundary: &str) -> Result<Task> {
        record_repair(&self.project, "job-0001", RepairCommand::Exercise {
            cause: CAUSE.into(), boundary: boundary.into(), machine: machine.into(), build: build.into(), event: event.into(),
            evidence: "t-0719 attempt 3 sealed report: observed new process alive past automatic-retry ready window, on installed dd498b3".into()
        })
    }

    fn repair(&self) -> RepairView {
        view(&self.project, load(&self.project, "job-0001").unwrap())
            .repairs
            .remove(0)
    }

    fn installed_lane(&self) {
        thread::update(&self.project, &self.lane.id, |lane| {
            lane.installed_sha = BUILD.into()
        })
        .unwrap();
    }
}

#[test]
fn d13_two_attempts_and_lanes_share_one_confirmed_repair_and_original_limits() {
    let case = Case::new();
    case.event("r176", "t-0468", 1, "2026-09-25T20:00:00Z", false);
    case.event("tonight-1", "t-0712", 1, "2026-10-03T07:00:00Z", false);
    case.event("tonight-2", "t-0712", 2, "2026-10-03T07:10:00Z", false);
    assert!(
        load(&case.project, "job-0001")
            .unwrap()
            .incidents
            .is_empty()
    );
    case.link(&["r176", "tonight-1"]).unwrap();
    let task = case.link(&["tonight-2", "tonight-1"]).unwrap();
    assert_eq!(task.incidents.len(), 1);
    assert_eq!(task.incidents[0].confirmations.len(), 3);
    assert_eq!(
        task.incidents[0].limits,
        "original two same-recipe retries; spend unknown"
    );
    assert_eq!(list_with_errors(&case.project).0.len(), 1);
    assert_eq!(case.repair().cost.unknown_attempts, 3);
    assert_eq!(case.repair().cost.known, None);
    let changed = record_repair(
        &case.project,
        "job-0001",
        RepairCommand::Link {
            cause: CAUSE.into(),
            boundary: BOUNDARY.into(),
            limits: Some("renewed budget".into()),
            event: vec!["tonight-2".into()],
            reason: "same cause".into(),
        },
    );
    assert!(
        changed
            .unwrap_err()
            .to_string()
            .contains("original boundary or limits")
    );
    let mut other = task;
    other.id = "job-0002".into();
    other.incidents.clear();
    write(&case.project, &other).unwrap();
    assert!(
        record_repair(
            &case.project,
            "job-0002",
            RepairCommand::Link {
                cause: CAUSE.into(),
                boundary: BOUNDARY.into(),
                limits: Some("unknown".into()),
                event: vec!["tonight-2".into()],
                reason: "confirmed".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("already belongs to job-0001")
    );
}

#[test]
fn d13_merge_install_and_unrelated_success_are_not_effectiveness() {
    let case = Case::new();
    case.event("failed", "t-0719", 2, "2026-10-03T07:10:00Z", false);
    case.link(&["failed"]).unwrap();
    assert_eq!(case.repair().outcome, "repair not established");
    assert!(case.install().is_err()); // merged, not installed
    case.event("early-success", "t-0719", 3, "2026-10-03T07:40:00Z", true);
    assert!(
        case.exercise("early-success", "oci", BUILD, BOUNDARY)
            .is_err()
    );
    case.installed_lane();
    case.install().unwrap();
    assert_eq!(case.repair().outcome, "installed; boundary unexercised");
    assert!(
        case.exercise("early-success", "oci", BUILD, BOUNDARY)
            .is_err()
    );
    case.event("live-success", "t-0719", 3, "2026-10-03T07:50:00Z", true);
    assert!(
        case.exercise("live-success", "mac", BUILD, BOUNDARY)
            .is_err()
    );
    assert!(
        case.exercise("live-success", "oci", "another-build", BOUNDARY)
            .is_err()
    );
    assert!(
        case.exercise("live-success", "oci", BUILD, "cargo test passed")
            .is_err()
    );
    assert!(case.exercise("failed", "oci", BUILD, BOUNDARY).is_err());
    assert_eq!(case.repair().outcome, "installed; boundary unexercised");
    case.exercise("live-success", "oci", BUILD, BOUNDARY)
        .unwrap();
    case.exercise("live-success", "oci", BUILD, BOUNDARY)
        .unwrap();
    assert_eq!(case.repair().outcome, "effective at exercised boundary");
    assert_eq!(
        load(&case.project, "job-0001").unwrap().incidents[0]
            .exercises
            .len(),
        1
    );
}

#[test]
fn d13_post_install_recurrence_remains_visible_without_changing_plan_done_counts() {
    let case = Case::new();
    case.event("failed", "t-0719", 2, "2026-10-03T07:10:00Z", false);
    case.link(&["failed"]).unwrap();
    case.installed_lane();
    case.install().unwrap();
    case.event("live-success", "t-0719", 3, "2026-10-03T07:50:00Z", true);
    case.exercise("live-success", "oci", BUILD, BOUNDARY)
        .unwrap();
    let mut plan = Plan {
        schema: 1,
        steps: vec![PlanStep {
            id: "s-1".into(),
            tasks: vec!["job-0001".into()],
            ..Default::default()
        }],
        ..Default::default()
    };
    crate::plan::project_states(&case.project, &mut plan);
    let before = plan.clone();
    assert_eq!(plan.steps[0].state, crate::contracts::StepState::Done);
    case.event("recurred", "t-0719", 4, "2026-10-03T08:00:00Z", false);
    case.link(&["recurred"]).unwrap();
    assert_eq!(case.repair().outcome, "recurred after install");
    assert!(
        case.repair()
            .timeline
            .iter()
            .any(|row| row.contains("recurred") && row.contains("recurrence after install"))
    );
    crate::plan::project_states(&case.project, &mut plan);
    assert_eq!(plan, before);
    let (settings, _) = case.project.read_project_md().unwrap();
    let projection = crate::project_view::View::capture(&case.project, &settings, None, None);
    let section = projection
        .sections
        .iter()
        .find(|section| section.name == "Repair outcomes")
        .unwrap();
    assert!(section.render().contains("recurred after install"));
    assert_eq!(list_with_errors(&case.project).0.len(), 1);
    // Later successful exercise can establish the boundary again, without erasing recurrence.
    case.event("success-again", "t-0719", 5, "2026-10-03T08:10:00Z", true);
    case.exercise("success-again", "oci", BUILD, BOUNDARY)
        .unwrap();
    assert_eq!(case.repair().outcome, "effective at exercised boundary");
    assert!(
        case.repair()
            .timeline
            .iter()
            .any(|row| row.contains("recurrence after install"))
    );
}

#[test]
fn dependency_waits_need_confirmation_and_corrupt_recovery_input_cannot_block_delivery() {
    let case = Case::new();
    let mut event = case.event(
        "process-failure",
        "t-0712",
        1,
        "2026-10-03T07:00:00Z",
        false,
    );
    event.id = "dependency-wait".into();
    event.payload.waiting = event.payload.failed.take();
    events::seal_create_if_absent(&case.project, &event).unwrap();
    assert!(
        load(&case.project, "job-0001")
            .unwrap()
            .incidents
            .is_empty()
    );
    let mut command = RepairCommand::Link {
        cause: "confirmed provider dependency".into(),
        boundary: "provider readiness".into(),
        limits: Some("original deadline; usage unknown".into()),
        event: vec![event.id.clone()],
        reason: "".into(),
    };
    assert!(record_repair(&case.project, "job-0001", command).is_err());
    command = RepairCommand::Link {
        cause: "confirmed provider dependency".into(),
        boundary: "provider readiness".into(),
        limits: Some("original deadline; usage unknown".into()),
        event: vec![event.id.clone()],
        reason: "shared provider outage confirmed from exact adapter dependency evidence".into(),
    };
    record_repair(&case.project, "job-0001", command).unwrap();
    crate::launch::dispatch(&case.project, serde_json::json!({"kind":"recovery-exhausted", "event":event.id, "error":"original limit exhausted"})).unwrap();
    assert!(
        case.repair()
            .timeline
            .iter()
            .any(|row| row.contains("original limit exhausted"))
    );
    std::fs::write(
        case.project.state_dir().join("dispatch.jsonl"),
        "not json\n",
    )
    .unwrap();
    assert!(
        case.repair()
            .timeline
            .iter()
            .any(|row| row.contains("journal evidence unknown"))
    );
    events::append_delivery(
        &case.project,
        &event.id,
        crate::contracts::DeliveryState::Handled,
    )
    .unwrap();
    let sealed = case.event("new-seal", "t-0712", 2, "2026-10-03T07:10:00Z", false);
    assert_eq!(events::load(&case.project, &sealed.id).unwrap(), sealed);
}

#[test]
fn missing_success_report_downgrades_effectiveness_and_old_tasks_still_load() {
    let case = Case::new();
    let old = std::fs::read_to_string(path(&case.project, "job-0001")).unwrap();
    assert!(!old.contains("incidents"));
    assert!(
        load(&case.project, "job-0001")
            .unwrap()
            .incidents
            .is_empty()
    );
    case.event("failed", "t-0719", 2, "2026-10-03T07:10:00Z", false);
    case.link(&["failed"]).unwrap();
    case.installed_lane();
    case.install().unwrap();
    let success = case.event("live-success", "t-0719", 3, "2026-10-03T07:50:00Z", true);
    case.exercise(&success.id, "oci", BUILD, BOUNDARY).unwrap();
    std::fs::remove_file(events::artifact_path(
        &case.project,
        &success.payload.done.unwrap().artifact,
    ))
    .unwrap();
    assert_ne!(case.repair().outcome, "effective at exercised boundary");
    assert!(case.exercise(&success.id, "oci", BUILD, BOUNDARY).is_err());
    // Fixture retains the world (and its records) for the whole test.
    assert!(case.world.home.path().exists());
}

#[test]
fn current_pile_acceptance_and_install_receipts_are_required_before_exercise() {
    let case = Case::new();
    thread::update(&case.project, &case.lane.id, |lane| {
        lane.merged_sha.clear();
        lane.merged_review = "review-59".into();
    })
    .unwrap();
    case.event("failed", "t-0719", 2, "2026-10-03T07:10:00Z", false);
    case.link(&["failed"]).unwrap();
    let source = case.event(
        "repair-seal",
        &case.lane.id,
        1,
        "2026-10-03T07:20:00Z",
        true,
    );
    // Producer success with no independent acceptance cannot establish repair.
    assert!(case.install().is_err());
    let criteria = crate::contracts::CriterionEvidence {
        thread: case.lane.id.clone(),
        event: source.id.clone(),
        criterion: 1,
        condition: BOUNDARY.into(),
        established: true,
        evidence: "reviewer inspected retry-identity change and its reproduction regression".into(),
    };
    let front = toml::to_string(&serde_json::json!({"acceptance": [criteria]})).unwrap();
    let report = format!("+++\n{front}+++\nIndependent pile review.\n");
    let artifact = thread::sha256_hex(report.as_bytes());
    std::fs::write(events::artifact_path(&case.project, &artifact), report).unwrap();
    let judged = Event {
        id: "reviewed".into(),
        op: "reviewed".into(),
        thread: "reviewer".into(),
        attempt: 1,
        recipient: Recipient::default(),
        created: "2026-10-03T07:35:00Z".into(),
        usage: None,
        payload: EventPayload {
            done: Some(DonePayload {
                artifact,
                sha: BUILD.into(),
                ..Default::default()
            }),
            ..Default::default()
        },
    };
    events::seal_create_if_absent(&case.project, &judged).unwrap();
    let mut review: crate::review::Review = serde_json::from_value(serde_json::json!({
        "id":"review-59", "repo":"/repo", "integration":"main", "base":"base", "candidate_branch":"candidate",
        "members":[{"thread":case.lane.id, "attempt":1, "event":source.id, "sha":BUILD, "branch":"repair", "artifact":source.payload.done.unwrap().artifact}],
        "gates":[], "selected_gates":[], "reviewer":"reviewer", "phase":"landing",
        "verdict":{"verdict":"APPROVE", "review":"review-59", "candidate":BUILD}, "verdict_event":"reviewed",
        "reviewer_after":"", "checked_event":"reviewed", "retry_attempt":null, "retry_generation":0, "moved":0,
        "refresh_tip":null, "push_remote":null, "install_required":true, "fast_forward":true, "push":true,
        "install":false, "close":false, "prune":false, "attention":""
    })).unwrap();
    crate::review::save(&case.project, &review).unwrap();
    assert!(
        require_accepted(
            &case.project,
            &load(&case.project, "job-0001").unwrap(),
            &EvidenceSnapshot::load(&case.project)
        )
        .is_ok()
    );
    assert!(case.install().is_err()); // accepted and merged still is not installed
    review.install = true;
    crate::review::save(&case.project, &review).unwrap();
    assert!(
        record_repair(
            &case.project,
            "job-0001",
            RepairCommand::Install {
                cause: CAUSE.into(),
                machine: "oci".into(),
                build: BUILD.into(),
                at: "2026-10-03T07:30:00Z".into(),
                evidence: "premature installation claim".into()
            }
        )
        .is_err()
    );
    case.install().unwrap();
    assert_eq!(case.repair().outcome, "installed; boundary unexercised");
    case.event("live-success", "t-0719", 3, "2026-10-03T07:50:00Z", true);
    case.exercise("live-success", "oci", BUILD, BOUNDARY)
        .unwrap();
    assert_eq!(case.repair().outcome, "effective at exercised boundary");
    std::fs::remove_file(events::artifact_path(
        &case.project,
        &judged.payload.done.unwrap().artifact,
    ))
    .unwrap();
    assert_eq!(case.repair().outcome, "unknown"); // missing independent review does not survive as effective
}

#[test]
fn known_usage_is_attempt_deduplicated_and_partial_usage_is_not_zero() {
    let case = Case::new();
    let mut known = case.event("known", "t-0712", 1, "2026-10-03T07:00:00Z", false);
    known.usage = Some(crate::usage::Usage {
        input: 80,
        output: 20,
        total: 100,
        ..Default::default()
    });
    known.id = "known-usage".into();
    events::seal_create_if_absent(&case.project, &known).unwrap();
    let unknown = case.event("unknown", "t-0719", 2, "2026-10-03T07:10:00Z", false);
    let cost = crate::usage::cost(&[&known, &known, &unknown]);
    assert_eq!(cost.known.unwrap().total, 100);
    assert_eq!(cost.measured_attempts, 1);
    assert_eq!(cost.unknown_attempts, 1);
    // A later seal can lack counters without erasing an earlier known lower bound.
    let mut later_unknown = known.clone();
    later_unknown.id = "later-unknown".into();
    later_unknown.created = "2026-10-03T07:05:00Z".into();
    later_unknown.usage = None;
    let partial = crate::usage::cost(&[&known, &later_unknown]);
    assert_eq!(partial.known.unwrap().total, 100);
    assert_eq!(partial.measured_attempts, 0); // the latest snapshot is unmeasured
    assert_eq!(partial.unknown_attempts, 1);
    let mut resumed = known.clone();
    resumed.attempt = 2;
    resumed.usage.as_mut().unwrap().total = 150;
    let cumulative = crate::usage::cost(&[&known, &resumed, &unknown]);
    assert_eq!(cumulative.known.unwrap().total, 150); // not 100 + 150 from the same transcript
    assert_eq!(cumulative.measured_attempts, 2);
    assert_eq!(cumulative.unknown_attempts, 1);
    case.link(&["known-usage", "unknown"]).unwrap();
    let summary = repair_summary(&case.repair());
    assert!(summary.contains("100 tokens"));
    assert!(summary.contains("1 usage unknown"));
    assert!(summary.contains("other intervention cost unknown"));
}
