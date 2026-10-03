use super::*;
use crate::{
    contracts::{Plan, PlanStep},
    task::Task,
    testkit::{Fx, fixture},
};

fn task(f: &Fx, id: &str, attempts: Vec<String>) {
    crate::prompt::record_test_request(&f.project, "q-1", "Deliver a usable result").unwrap();
    let task = Task {
        id: id.into(),
        title: "Resolve the uncovered outcome".into(),
        authority: vec!["request:q-1".into()],
        acceptance: vec!["The result is usable".into()],
        attempts,
        ..Task::default()
    };
    let dir = f.project.record_dir_for_write("tasks").unwrap();
    std::fs::write(
        dir.join(format!("{id}.toml")),
        toml::to_string(&task).unwrap(),
    )
    .unwrap();
}

fn done_plan(f: &Fx) {
    let plan = Plan {
        schema: 1,
        revision: 4,
        does: "A usable result".into(),
        steps: vec![PlanStep {
            id: "s-1".into(),
            text: "Initial work".into(),
            state: StepState::Done,
            ..PlanStep::default()
        }],
        ..Plan::default()
    };
    std::fs::write(
        crate::plan::plan_path(&f.project),
        toml::to_string(&plan).unwrap(),
    )
    .unwrap();
}

fn idle() -> Agent {
    Agent {
        pane_id: "w1:p1".into(),
        agent_status: "idle".into(),
        ..Agent::default()
    }
}

#[test]
fn exhausted_unproved_plan_owes_once_and_justified_work_consumes_without_count_changes() {
    let f = fixture();
    done_plan(&f);
    let before = crate::plan::load(&f.project).unwrap().unwrap();
    reconcile(&f.project, None, 10).unwrap();
    let first = load(&f.project);
    for _ in 0..4 {
        reconcile(&f.project, None, 20).unwrap();
    }
    assert_eq!(first, load(&f.project), "restart/pass dedup");
    assert!(first.disposition.is_none());
    assert!(
        record(
            &f.project,
            Disposition::Closed {
                tasks: vec![],
                outcome: before.does.clone()
            },
            "checklist done"
        )
        .is_err()
    );
    assert_eq!(before, crate::plan::load(&f.project).unwrap().unwrap());
    task(&f, "job-0001", vec![]);
    crate::plan::step_add(
        &f.world.ctx(),
        "demo",
        "Cover missing outcome",
        vec!["job-0001".into()],
        vec![],
        None,
    )
    .unwrap();
    reconcile(&f.project, None, 30).unwrap();
    assert!(
        matches!(load(&f.project).disposition, Some(Disposition::Action { task }) if task == "job-0001")
    );
    assert!(notice(&f.project).is_none());
    let plan = crate::plan::load(&f.project).unwrap().unwrap();
    // Plan mutations refresh actual evidence, but goal checks do not touch the counts.
    let done = plan
        .steps
        .iter()
        .filter(|s| s.state == StepState::Done)
        .count();
    for _ in 0..3 {
        reconcile(&f.project, None, 40).unwrap();
    }
    assert_eq!(
        done,
        crate::plan::load(&f.project)
            .unwrap()
            .unwrap()
            .steps
            .iter()
            .filter(|s| s.state == StepState::Done)
            .count()
    );
}

#[test]
fn absent_plan_articulates_then_links_request_backed_work() {
    let f = fixture();
    reconcile(&f.project, None, 10).unwrap();
    assert!(notice(&f.project).is_some());
    crate::plan::set(
        &f.world.ctx(),
        "demo",
        "A useful decision, with assumptions separated",
        None,
    )
    .unwrap();
    reconcile(&f.project, None, 20).unwrap();
    assert!(
        load(&f.project).disposition.is_none(),
        "wording alone is not progress"
    );
    task(&f, "job-0001", vec![]);
    crate::plan::step_add(
        &f.world.ctx(),
        "demo",
        "Challenge risky assumption",
        vec!["job-0001".into()],
        vec![],
        None,
    )
    .unwrap();
    reconcile(&f.project, None, 30).unwrap();
    assert!(matches!(
        load(&f.project).disposition,
        Some(Disposition::Action { .. })
    ));
}

#[test]
fn unchanged_turn_reowes_diagnosis_once_then_escalates_and_empty_turn_is_bounded() {
    for observe_working in [true, false] {
        let f = fixture();
        let idle = idle();
        reconcile(&f.project, Some(&idle), 10).unwrap();
        let (token, original) = notice(&f.project).unwrap();
        queued(&f.project, &token).unwrap();
        assert!(
            load(&f.project).disposition.is_none(),
            "outbox acceptance is not consumption"
        );
        delivered(&f.project, &token, &idle, 20).unwrap();
        reconcile(&f.project, Some(&idle), 21).unwrap();
        assert!(
            notice(&f.project).is_none(),
            "idle polling is not another turn"
        );
        if observe_working {
            let mut working = idle.clone();
            working.agent_status = "working".into();
            reconcile(&f.project, Some(&working), 30).unwrap();
        }
        reconcile(&f.project, Some(&idle), 620).unwrap();
        let (retry, diagnosis) = notice(&f.project).unwrap();
        assert_ne!(retry, token);
        assert_ne!(diagnosis, original);
        delivered(&f.project, &retry, &idle, 630).unwrap();
        // Rewording a plan without an action/proof/wait is not a disposition
        // and cannot buy another identical wake.
        crate::plan::set(&f.world.ctx(), "demo", "Same outcome, restated", None).unwrap();
        reconcile(&f.project, Some(&idle), 640).unwrap();
        reconcile(&f.project, Some(&idle), 1230).unwrap();
        assert_eq!(load(&f.project).disposition, Some(Disposition::NeedsRolf));
        assert!(attention(&f.project).unwrap().contains("needs Rolf"));
        for _ in 0..10 {
            reconcile(&f.project, Some(&idle), 2000).unwrap();
            assert!(notice(&f.project).is_none());
        }
    }
}

fn write_review(f: &Fx, phase: &str, merged: bool) {
    let review: crate::review::Review = serde_json::from_value(serde_json::json!({
        "id":"review-1", "repo":"", "integration":"", "base":"", "candidate_branch":"", "members":[], "gates":[], "selected_gates":[], "reviewer":null,
        "phase":phase, "verdict":null, "verdict_event":phase, "reviewer_after":"", "checked_event":"", "retry_attempt":null, "retry_generation":0,
        "moved":0, "refresh_tip":null, "push_remote":null, "install_required":false, "fast_forward":merged, "push":false, "install":false,
        "close":false, "prune":false, "attention":""
    })).unwrap();
    std::fs::create_dir_all(crate::review::dir(&f.project)).unwrap();
    std::fs::write(
        crate::review::path(&f.project, "review-1"),
        toml::to_string(&review).unwrap(),
    )
    .unwrap();
}

#[test]
fn seal_landing_and_reject_each_owe_deduplicated_checks_across_restarts() {
    let f = fixture();
    reconcile(&f.project, None, 10).unwrap();
    record(
        &f.project,
        Disposition::Wait {
            tasks: vec![],
            party: "result".into(),
            condition: "new evidence".into(),
        },
        "await result",
    )
    .unwrap();
    let lane = f.thread("Research");
    let event = f.seal_done(&lane, 1, 1, "", "The preferred approach is invalid");
    reconcile(&f.project, None, 20).unwrap();
    let first = load(&f.project);
    assert!(first.effects.contains(&event));
    assert!(first.disposition.is_none());
    for _ in 0..3 {
        reconcile(&f.project, None, 20).unwrap();
        assert_eq!(load(&f.project), first);
    }
    write_review(&f, "landing", true);
    reconcile(&f.project, None, 30).unwrap();
    let landed = load(&f.project);
    assert_eq!(landed.generation, first.generation + 1);
    assert!(landed.disposition.is_none());
    reconcile(&f.project, None, 30).unwrap();
    assert_eq!(load(&f.project), landed);
    write_review(&f, "rejected", false);
    reconcile(&f.project, None, 40).unwrap();
    assert_eq!(load(&f.project).generation, landed.generation + 1);
    reconcile(&f.project, None, 40).unwrap();
    assert_eq!(load(&f.project).generation, landed.generation + 1);
}

#[test]
fn evidence_backed_closure_stops_wakes_and_does_not_change_done_counts() {
    let f = fixture();
    let lane = f.thread("Outcome proof");
    task(&f, "job-0001", vec![lane.clone()]);
    crate::plan::set(&f.world.ctx(), "demo", "A usable result", None).unwrap();
    crate::plan::step_add(
        &f.world.ctx(),
        "demo",
        "Verify result",
        vec!["job-0001".into()],
        vec![],
        None,
    )
    .unwrap();
    reconcile(&f.project, None, 10).unwrap();
    let close = Disposition::Closed {
        tasks: vec!["job-0001".into()],
        outcome: "A usable result".into(),
    };
    assert!(record(&f.project, close.clone(), "no proof yet").is_err());
    let event = f.seal_done(&lane, 1, 1, "", "Acceptance exercised; result usable");
    crate::thread::update(&f.project, &lane, |t| {
        t.changes_seal = event.clone();
        t.has_changes = Some(false);
    })
    .unwrap();
    crate::plan::sync(&f.world.ctx(), "demo").unwrap();
    reconcile(&f.project, None, 20).unwrap();
    let before = crate::plan::load(&f.project).unwrap().unwrap();
    assert_eq!(before.steps[0].state, StepState::Done);
    record(
        &f.project,
        close,
        &format!("{event} report proves Plan.does and request:q-1 acceptance"),
    )
    .unwrap();
    for _ in 0..5 {
        reconcile(&f.project, Some(&idle()), 1000).unwrap();
        assert!(notice(&f.project).is_none());
    }
    assert_eq!(before, crate::plan::load(&f.project).unwrap().unwrap());
}

#[test]
fn scoped_wait_survives_independent_work_and_replacement_then_answer_rechecks_once() {
    let f = fixture();
    let a = f.thread("Research A");
    task(&f, "job-0001", vec![a.clone()]);
    f.seal_waiting(&a, 1, 1, "Need Rolf's consequential choice");
    reconcile(&f.project, None, 10).unwrap();
    let wait = Disposition::Wait {
        tasks: vec!["job-0001".into()],
        party: "Rolf".into(),
        condition: "request-backed answer to choice".into(),
    };
    record(
        &f.project,
        wait.clone(),
        "A awaits a choice; B depends on A",
    )
    .unwrap();
    reconcile(&f.project, Some(&idle()), 20).unwrap();
    assert!(notice(&f.project).is_none());
    let c = f.thread("Independent C");
    task(&f, "job-0002", vec![c]);
    reconcile(&f.project, Some(&idle()), 30).unwrap();
    record(
        &f.project,
        Disposition::Action {
            task: "job-0002".into(),
        },
        "C is independent of A/B",
    )
    .unwrap();
    let mut replacement = idle();
    replacement.pane_id = "w2:p9".into();
    reconcile(&f.project, Some(&replacement), 40).unwrap();
    assert!(notice(&f.project).is_none());
    assert_eq!(load(&f.project).waits[0].0, wait);
    crate::prompt::record_test_request(&f.project, "q-answer", "Choose the reversible route")
        .unwrap();
    reconcile(&f.project, Some(&replacement), 50).unwrap();
    let answer_check = load(&f.project);
    assert!(answer_check.disposition.is_none());
    for _ in 0..3 {
        reconcile(&f.project, Some(&replacement), 60).unwrap();
        assert_eq!(load(&f.project), answer_check);
    }
    // Continuation effects belong to the existing lane follow-up path, not this check.
    crate::plan::step_add(
        &f.world.ctx(),
        "demo",
        "Continue A",
        vec!["job-0001".into()],
        vec![],
        None,
    )
    .unwrap();
    reconcile(&f.project, None, 70).unwrap();
    assert!(
        matches!(load(&f.project).disposition, Some(Disposition::Action { task }) if task == "job-0001")
    );
    assert!(load(&f.project).waits.is_empty());
    reconcile(&f.project, None, 80).unwrap();
    assert!(notice(&f.project).is_none());
}

#[test]
fn explicit_wait_party_replaces_phrase_inferred_responsibility() {
    let f = fixture();
    let lane = f.thread("Research");
    task(&f, "job-0001", vec![lane.clone()]);
    f.seal_waiting(
        &lane,
        1,
        1,
        "Rolf will need this after the upstream service returns",
    );
    reconcile(&f.project, None, 10).unwrap();
    record(
        &f.project,
        Disposition::Wait {
            tasks: vec!["job-0001".into()],
            party: "upstream service".into(),
            condition: "data is available".into(),
        },
        "Not waiting on Rolf",
    )
    .unwrap();
    let (settings, _) = f.project.read_project_md().unwrap();
    let view = crate::project_view::View::capture(&f.project, &settings, None, None);
    assert!(
        !view
            .needs_you
            .iter()
            .any(|line| line.starts_with(&format!("{lane}:")))
    );
}

#[test]
fn historical_ticker_and_plan_load_without_changing_completion() {
    let f = fixture();
    std::fs::write(f.project.state_dir().join("ticker.json"), r#"{"plan_nudged":true,"plan_revision":4,"plan_lane_ids":["t-0001"],"plan_request":"q-1","announced":"old"}"#).unwrap();
    assert_eq!(crate::steps::load_state(&f.project).announced, "old");
    done_plan(&f);
    let before = std::fs::read(crate::plan::plan_path(&f.project)).unwrap();
    reconcile(&f.project, None, 10).unwrap();
    assert!(notice(&f.project).is_some());
    assert_eq!(
        before,
        std::fs::read(crate::plan::plan_path(&f.project)).unwrap()
    );
    assert_eq!(
        load(&f.project),
        project::read_json::<Check>(&path(&f.project)).unwrap()
    );
}
