use super::*;
use crate::testkit::{Fx, commit_file, fixture, git};

mod holds;
mod starts;
use std::path::Path;

fn configured() -> Fx {
    let fx = fixture();
    std::fs::write(
        fx.repo.join(".git/info/exclude"),
        ".worktrees/\n.herdr-project/\n",
    )
    .unwrap();
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    settings.repos[0].gates = Some(Vec::new());
    settings.repos[0].branch = Some("main".into());
    std::fs::write(
        fx.project.project_md(),
        format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
    )
    .unwrap();
    fx
}
fn pending(project: &Project, repo: &str, events: &[crate::contracts::Event]) -> Vec<Thread> {
    super::pending(project, repo, events, &reviewer_ids(project).unwrap())
}
fn lane_unsealed(fx: &Fx, n: u32) -> (String, String) {
    let (id, sha) = fx.lane(n);
    let branch = format!("hp/demo/{id}-work");
    git(&fx.repo, &["branch", "-m", &format!("lane/{n}"), &branch]);
    let base = git(&fx.repo, &["rev-parse", "main"]);
    thread::update(&fx.project, &id, |t| {
        t.base = base;
        t.branch = branch;
        t.tab_id.clear();
        t.pane_id.clear();
        t.thread_dir = thread::thread_dir(&t.worktree_path, &fx.project.slug, &id);
    })
    .unwrap();
    (id, sha)
}
fn lane(fx: &Fx, n: u32) -> (String, String) {
    let (id, sha) = lane_unsealed(fx, n);
    fx.seal_done(&id, 1, 1, &sha, "finished\n");
    (id, sha)
}
fn prepared(fx: &Fx) -> Review {
    // Recover the allocation-before-binding crash, without any live process.
    let id = fx.thread("pile reviewer");
    thread::update(&fx.project, &id, |t| {
        t.role = "reviewer".into();
        t.review_id = "review-1".into();
        t.pane_id.clear();
    })
    .unwrap();
    let review = start(&fx.world.ctx(), "demo", None).unwrap().unwrap();
    assert_eq!(review.reviewer.as_deref(), Some(id.as_str()));
    let checkout = fx.repo.join(".worktrees/reviewer");
    let branch = format!("hp/demo/{id}-review");
    git(
        &fx.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &branch,
            checkout.to_str().unwrap(),
            &review.candidate_branch,
        ],
    );
    thread::update(&fx.project, &id, |t| {
        t.kind = thread::Kind::Worktree;
        t.repo = fx.repo.to_string_lossy().into_owned();
        t.worktree_path = checkout.to_string_lossy().into_owned();
        t.cwd = t.worktree_path.clone();
        t.thread_dir = thread::thread_dir(&t.worktree_path, &fx.project.slug, &id);
        t.branch = branch;
        t.base = review.base.clone();
    })
    .unwrap();
    review
}
fn seal_verdict(
    fx: &Fx,
    review: &Review,
    candidate: &str,
    word: &str,
    without: BTreeMap<String, String>,
    gates: Vec<GateRun>,
    n: u32,
) {
    let verdict = Verdict {
        verdict: word.into(),
        review: review.id.clone(),
        candidate: candidate.into(),
        without,
        gates,
        gates_note: String::new(),
    };
    let report = format!(
        "+++\n{}+++\n\nChecked the complete pile.\n",
        toml::to_string(&verdict).unwrap()
    );
    fx.seal_done(
        review.reviewer.as_deref().unwrap(),
        1,
        n,
        candidate,
        &report,
    );
}
#[test]
fn packet_carries_original_acceptance_and_durable_evidence_not_rewritten_intent() {
    let fx = configured();
    let (id, _) = lane(&fx, 1);
    let original = "# Original request\nRead every source.\nAcceptance: exhaustive coverage, preserve existing behavior.\n";
    let hash = thread::store_artifact(&fx.project, original.as_bytes()).unwrap();
    thread::update(&fx.project, &id, |lane| {
        lane.launch.brief_hash = hash.clone()
    })
    .unwrap();
    let mut record = job(&fx, &id);
    record.acceptance = vec!["Later rewritten intent".into()];
    record.installed.push(crate::task::Evidence {
        at: "then".into(),
        command: "ha harness install".into(),
        acceptance: vec![],
        machine: Some("oci".into()),
        build: Some("build-sha".into()),
    });
    std::fs::write(
        fx.project
            .record_dir_for_write("tasks")
            .unwrap()
            .join(format!("{}.toml", record.id)),
        toml::to_string(&record).unwrap(),
    )
    .unwrap();
    let review = prepared(&fx);
    let packet = task(&fx.project, &review);
    assert!(packet.contains(original));
    assert!(packet.contains(&hash));
    assert!(packet.contains(&review.members[0].event));
    assert!(packet.contains(&review.members[0].artifact));
    assert!(packet.contains("Installation (not acceptance)"));
    assert!(packet.contains("build-sha"));
    assert!(!packet.contains("Later rewritten intent"));
    thread::update(&fx.project, &id, |lane| lane.launch.brief_hash.clear()).unwrap();
    assert!(task(&fx.project, &review).contains("Original brief/acceptance: not established"));
}

#[test]
fn idle_reviewer_warns_once_and_a_new_verdict_lands() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let id = review.reviewer.as_deref().unwrap().to_string();
    thread::update(&fx.project, &id, |t| {
        t.status = Status::Open;
        t.workspace_id = "w1".into();
        t.tab_id = "w1:t2".into();
        t.pane_id = "w1:p2".into();
        t.agent_name = "reviewer-test".into();
    })
    .unwrap();
    let reviewer = thread::load(&fx.project, &id).unwrap();
    review.no_verdict_since = "2020-01-01T00:00:00Z".into();
    // No matching process: the lane recovery owns this, not the idle warning.
    watch_no_verdict(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert!(review.notices.is_empty());
    assert!(review.no_verdict_since.is_empty());
    *fx.world.agents.borrow_mut() = format!(
        "[{{\"pane_id\":\"w1:p2\",\"tab_id\":\"w1:t2\",\"workspace_id\":\"w1\",\"name\":\"reviewer-test\",\"cwd\":{:?},\"agent_status\":\"idle\"}}]",
        reviewer.cwd
    );
    watch_no_verdict(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert!(review.notices.is_empty());
    review.no_verdict_since = "2020-01-01T00:00:00Z".into();
    watch_no_verdict(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    let count = review.notices.len();
    assert_eq!(count, 1);
    assert!(review.notices[0].line.contains("no merge attempted"));
    watch_no_verdict(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.notices.len(), count);
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        1,
    );
    thread::update(&fx.project, &id, |t| t.pane_id.clear()).unwrap();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert_ne!(
        load(&fx.project, &review.id).unwrap().phase,
        Phase::Reviewing
    );
}

#[test]
fn sleep_does_not_age_the_stuck_review_notice() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    let now = jiff::Timestamp::now().as_second();
    review.no_verdict_since = jiff::Timestamp::from_second(now - 2400)
        .unwrap()
        .to_string();
    crate::awake::set_sample(Some((now - 2400, 100)));
    drop(crate::awake::enter(&fx.world.root, true).unwrap());
    crate::awake::set_sample(Some((now, 140)));
    let (_clock, slept) = crate::awake::enter(&fx.world.root, true).unwrap();
    assert!(slept);
    watch_no_verdict_state(&fx.world.ctx(), &fx.project, &mut review, &reviewer, "idle").unwrap();
    assert!(review.notices.is_empty());
    assert!(!review.no_verdict_since.is_empty());
    crate::awake::set_sample(None);
}

#[test]
fn follow_up_after_seal_checks_verdict_now_but_waits_to_land() {
    let fx = configured();
    lane(&fx, 1);
    let review = prepared(&fx);
    let reviewer = review.reviewer.as_deref().unwrap();
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "REJECT",
        BTreeMap::new(),
        vec![],
        1,
    );
    let seal = crate::events::latest_done_event(&crate::events::list(&fx.project), reviewer, 1)
        .unwrap()
        .id
        .clone();
    thread::update(&fx.project, reviewer, |lane| {
        lane.review_after = seal.clone();
        lane.follow_ups.push(thread::FollowUp {
            attempt: 1,
            state: thread::FollowUpState::Delivered,
            after_seal: seal.clone(),
            ..Default::default()
        });
    })
    .unwrap();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    let checked = load(&fx.project, &review.id).unwrap();
    assert_eq!(checked.phase, Phase::Reviewing);
    assert_eq!(checked.checked_event, seal);
    assert_eq!(checked.verdict.as_ref().unwrap().verdict, "REJECT");
    thread::update(&fx.project, reviewer, |lane| {
        lane.review_after.clear();
        lane.follow_ups[0].state = thread::FollowUpState::Closed;
    })
    .unwrap();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert_eq!(
        load(&fx.project, &review.id).unwrap().phase,
        Phase::Rejected
    );
}

#[test]
fn sealed_reviewer_verdict_is_checked_in_the_arrival_pass_without_a_local_session() {
    let fx = configured();
    lane(&fx, 1);
    let review = prepared(&fx);
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "REJECT",
        BTreeMap::new(),
        vec![],
        1,
    );
    // No coordinator socket: the slow project phase cannot run. The review
    // phase must still consume the seal in this pass, not wait for a later
    // local session recovery or a second courier poll.
    fx.project
        .update_coordinator(|record| record.socket.clear())
        .unwrap();
    let ctx = fx.world.ctx();
    let mut memory = crate::steps::Memory::new(&ctx);
    crate::ticker::tick_for_test(&ctx, &mut memory);
    let checked = load(&fx.project, &review.id).unwrap();
    assert_eq!(checked.phase, Phase::Rejected);
    assert_eq!(
        checked.verdict_event,
        format!("{}-1-1", review.reviewer.unwrap())
    );
}

#[test]
fn idle_unchanged_follow_up_restores_reviewer_seal_and_verdict_in_same_pass() {
    let fx = configured();
    lane(&fx, 1);
    let review = prepared(&fx);
    let reviewer = review.reviewer.as_deref().unwrap();
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "REJECT",
        BTreeMap::new(),
        vec![],
        1,
    );
    let sealed_event =
        crate::events::latest_done_event(&crate::events::list(&fx.project), reviewer, 1)
            .unwrap()
            .clone();
    let checkout = thread::load(&fx.project, reviewer).unwrap().worktree_path;
    let report = thread::artifact(
        &fx.project,
        &sealed_event.payload.done.as_ref().unwrap().artifact,
    )
    .unwrap();
    let path = Path::new(&checkout).join(&sealed_event.payload.done.as_ref().unwrap().report_path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, report).unwrap();
    std::fs::write(
        fx.repo.join(".git/info/exclude"),
        ".worktrees/\n.herdr-project/\n.reports/\n",
    )
    .unwrap();
    thread::update(&fx.project, reviewer, |lane| {
        lane.review_after = sealed_event.id.clone();
        lane.follow_ups.push(thread::FollowUp {
            attempt: 1,
            state: thread::FollowUpState::Delivered,
            after_seal: sealed_event.id.clone(),
            delivered_at: "2026-09-18T00:00:00Z".into(),
            ..Default::default()
        })
    })
    .unwrap();
    assert!(
        sealed(
            &crate::events::list(&fx.project),
            &thread::load(&fx.project, reviewer).unwrap()
        )
        .is_none()
    );
    crate::ticker::restore_unchanged_seal(
        &fx.world.ctx(),
        &fx.project,
        &thread::load(&fx.project, reviewer).unwrap(),
    )
    .unwrap();
    assert!(
        sealed(
            &crate::events::list(&fx.project),
            &thread::load(&fx.project, reviewer).unwrap()
        )
        .is_some()
    );
    tick(&fx.world.ctx(), &fx.project).unwrap();
    let advanced = load(&fx.project, &review.id).unwrap();
    assert_eq!(advanced.verdict_event, sealed_event.id);
    assert_eq!(advanced.verdict.unwrap().verdict, "REJECT");
}

#[test]
fn box_follow_up_restores_only_after_box_checkout_and_report_match() {
    let fx = configured();
    let (id, sha) = lane(&fx, 1);
    let event_id = fx.seal_done(&id, 1, 1, &sha, "report\n");
    let event = crate::events::latest_done_event(&crate::events::list(&fx.project), &id, 1)
        .unwrap()
        .clone();
    thread::update(&fx.project, &id, |lane| {
        lane.machine = "box".into();
        lane.machine_id = "box".into();
        lane.review_after = event_id.clone();
        lane.follow_ups.push(thread::FollowUp {
            attempt: 1,
            state: thread::FollowUpState::Delivered,
            after_seal: event_id.clone(),
            delivered_at: "2026-09-18T00:00:00Z".into(),
            ..Default::default()
        });
    })
    .unwrap();
    fx.world
        .runner
        .on("machine list --json", crate::runner::fake::ok("[]"));
    fx.world.runner.on(
        "ssh",
        crate::runner::fake::ok(&format!(
            "{sha}\n{}  report.md\n",
            event.payload.done.unwrap().artifact
        )),
    );
    crate::ticker::restore_unchanged_seal(
        &fx.world.ctx(),
        &fx.project,
        &thread::load(&fx.project, &id).unwrap(),
    )
    .unwrap();
    let restored = thread::load(&fx.project, &id).unwrap();
    assert!(restored.review_after.is_empty());
    assert_eq!(restored.follow_ups[0].state, thread::FollowUpState::Closed);
}

#[test]
fn committed_follow_up_keeps_old_reviewer_seal_void() {
    let fx = configured();
    lane(&fx, 1);
    let review = prepared(&fx);
    let reviewer = review.reviewer.as_deref().unwrap();
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "REJECT",
        BTreeMap::new(),
        vec![],
        1,
    );
    let sealed_event =
        crate::events::latest_done_event(&crate::events::list(&fx.project), reviewer, 1)
            .unwrap()
            .clone();
    let checkout = thread::load(&fx.project, reviewer).unwrap().worktree_path;
    let report = thread::artifact(
        &fx.project,
        &sealed_event.payload.done.as_ref().unwrap().artifact,
    )
    .unwrap();
    let path = Path::new(&checkout).join(&sealed_event.payload.done.as_ref().unwrap().report_path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, report).unwrap();
    std::fs::write(
        fx.repo.join(".git/info/exclude"),
        ".worktrees/\n.herdr-project/\n.reports/\n",
    )
    .unwrap();
    thread::update(&fx.project, reviewer, |lane| {
        lane.review_after = sealed_event.id.clone();
        lane.follow_ups.push(thread::FollowUp {
            attempt: 1,
            state: thread::FollowUpState::Delivered,
            after_seal: sealed_event.id.clone(),
            delivered_at: "2026-09-18T00:00:00Z".into(),
            ..Default::default()
        })
    })
    .unwrap();
    std::fs::write(&path, "changed report").unwrap();
    crate::ticker::restore_unchanged_seal(
        &fx.world.ctx(),
        &fx.project,
        &thread::load(&fx.project, reviewer).unwrap(),
    )
    .unwrap();
    assert_eq!(
        thread::load(&fx.project, reviewer).unwrap().review_after,
        sealed_event.id
    );
    std::fs::write(
        &path,
        thread::artifact(
            &fx.project,
            &sealed_event.payload.done.as_ref().unwrap().artifact,
        )
        .unwrap(),
    )
    .unwrap();
    commit_file(
        Path::new(&checkout),
        "new.txt",
        "change",
        "follow-up changed HEAD",
    );
    crate::ticker::restore_unchanged_seal(
        &fx.world.ctx(),
        &fx.project,
        &thread::load(&fx.project, reviewer).unwrap(),
    )
    .unwrap();
    assert!(
        sealed(
            &crate::events::list(&fx.project),
            &thread::load(&fx.project, reviewer).unwrap()
        )
        .is_none()
    );
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert!(load(&fx.project, &review.id).unwrap().verdict.is_none());
}

fn job(fx: &Fx, id: &str) -> crate::task::Task {
    let task = crate::task::Task {
        id: "job-0001".into(),
        title: "Deliver the change".into(),
        authority: vec!["request:q-1".into()],
        acceptance: vec!["The change works".into()],
        attempts: vec![id.into()],
        repo: Some(fx.repo.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let dir = fx.project.record_dir_for_write("tasks").unwrap();
    std::fs::write(dir.join("job-0001.toml"), toml::to_string(&task).unwrap()).unwrap();
    task
}

#[test]
fn resolved_historical_seals_are_classified_once_without_old_rounds() {
    let fx = configured();
    let (id, sha) = lane(&fx, 1);
    let task = job(&fx, &id);
    git(&fx.repo, &["merge", "--ff-only", &sha]);
    thread::update(&fx.project, &id, |t| t.status = Status::Resolved).unwrap();
    classify_old_seals(&fx.world.ctx(), &fx.project, true).unwrap();
    let record = thread::load(&fx.project, &id).unwrap();
    assert_eq!(record.has_changes, Some(true));
    assert!(!record.merged_sha.is_empty());
    assert_eq!(
        crate::task::view(&fx.project, task).state,
        crate::task::State::Merged
    );
    let calls = fx.world.runner.calls.borrow().len();
    classify_old_seals(&fx.world.ctx(), &fx.project, true).unwrap();
    assert_eq!(calls, fx.world.runner.calls.borrow().len());
}

#[test]
fn old_no_change_seal_finishes_without_merge_even_when_resolved() {
    let fx = configured();
    let (id, _) = lane(&fx, 1);
    let record = thread::load(&fx.project, &id).unwrap();
    fx.seal_done(&id, 1, 2, &record.base, "Nothing to merge");
    thread::update(&fx.project, &id, |t| t.status = Status::Resolved).unwrap();
    let task = job(&fx, &id);
    classify_old_seals(&fx.world.ctx(), &fx.project, true).unwrap();
    let record = thread::load(&fx.project, &id).unwrap();
    assert_eq!(record.has_changes, Some(false));
    assert!(record.merged_sha.is_empty());
    assert!(crate::review::lane_done(
        &fx.project,
        &record,
        &crate::events::list(&fx.project)
    ));
    assert_eq!(
        crate::task::view(&fx.project, task).state,
        crate::task::State::Finished
    );
}

#[test]
fn empty_commit_seals_as_no_change_without_starting_a_pile() {
    let fx = configured();
    let (id, _) = lane_unsealed(&fx, 1);
    let lane = thread::load(&fx.project, &id).unwrap();
    let wt = Path::new(&lane.worktree_path);
    git(wt, &["reset", "--hard", &lane.base]);
    git(
        wt,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Verify release bundle",
        ],
    );
    let sha = git(wt, &["rev-parse", "HEAD"]);
    assert_ne!(sha, lane.base);
    let report_path = wt.join(".herdr-project/report.md");
    std::fs::create_dir_all(report_path.parent().unwrap()).unwrap();
    std::fs::write(&report_path, "Verified bundle\n").unwrap();
    let op = crate::ops::reserve_done(
        &fx.project,
        crate::ops::Reservation {
            thread: &id,
            pane: &lane.pane_id,
            attempt: 1,
            kind: crate::contracts::OpKind::Done,
            recipient: crate::contracts::Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 1,
            },
            requested: crate::contracts::Requested::Done {
                sha,
                report_path: ".herdr-project/report.md".into(),
            },
            helper_pid: 1,
        },
        wt,
    )
    .unwrap();
    let staged = crate::ops::stage_done(&fx.project, &op.op, wt, fx.world.ctx().runner).unwrap();
    assert_eq!(staged.has_changes, Some(false));
    let seal = crate::ops::seal(&fx.project, &op.op, None, |_| Ok(())).unwrap();
    assert_eq!(seal.payload.done.unwrap().has_changes, Some(false));
    assert!(start(&fx.world.ctx(), "demo", None).unwrap().is_none());
    assert!(list(&fx.project).unwrap().is_empty());
    let task = job(&fx, &id);
    assert_eq!(
        crate::task::view(&fx.project, task).state,
        crate::task::State::Finished
    );
}

#[test]
fn old_empty_commit_seal_is_classified_as_no_change() {
    let fx = configured();
    let (id, _) = lane_unsealed(&fx, 1);
    let lane = thread::load(&fx.project, &id).unwrap();
    let wt = Path::new(&lane.worktree_path);
    git(wt, &["reset", "--hard", &lane.base]);
    git(
        wt,
        &["commit", "-q", "--allow-empty", "-m", "Verify bundle"],
    );
    let sha = git(wt, &["rev-parse", "HEAD"]);
    fx.seal_done(&id, 1, 1, &sha, "Verified bundle");
    assert!(start(&fx.world.ctx(), "demo", None).unwrap().is_none());
    assert_eq!(
        thread::load(&fx.project, &id).unwrap().has_changes,
        Some(false)
    );
    // Reclassify an uncached, resolved historical seal through the other path.
    thread::update(&fx.project, &id, |t| {
        t.status = Status::Resolved;
        t.changes_seal.clear();
        t.has_changes = None;
    })
    .unwrap();
    classify_old_seals(&fx.world.ctx(), &fx.project, true).unwrap();
    assert_eq!(
        thread::load(&fx.project, &id).unwrap().has_changes,
        Some(false)
    );
}

#[test]
fn old_round_reviewer_with_missing_worktree_repo_does_not_break_plan() {
    let fx = configured();
    let (id, _) = lane(&fx, 1);
    let rounds = fx.project.state_dir().join("rounds");
    std::fs::create_dir_all(&rounds).unwrap();
    std::fs::write(
        rounds.join("r19.toml"),
        format!("round = \"r19\"\nreviewer = \"{id}\"\n[merge]\nphase = \"merged\"\n"),
    )
    .unwrap();
    thread::update(&fx.project, &id, |t| {
        t.role = "reviewer".into();
        t.status = Status::Resolved;
        t.repo = fx
            .repo
            .join(".worktrees/review-r19")
            .to_string_lossy()
            .into();
    })
    .unwrap();
    assert!(reviewer_ids(&fx.project).unwrap().contains(&id));
    classify_old_seals(&fx.world.ctx(), &fx.project, true).unwrap();
    assert!(
        thread::load(&fx.project, &id)
            .unwrap()
            .historical_seal
            .is_empty()
    );
    assert!(crate::plan::show(&fx.world.ctx(), "demo", false).is_ok());
    assert!(start(&fx.world.ctx(), "demo", None).unwrap().is_none());
}

#[test]
fn unconfigured_lane_repo_warns_once_per_seal_and_can_be_reclassified_later() {
    let fx = configured();
    let (id, sha) = lane(&fx, 1);
    thread::update(&fx.project, &id, |t| {
        t.status = Status::Resolved;
        t.repo = "/gone/unconfigured".into();
    })
    .unwrap();
    classify_old_seals(&fx.world.ctx(), &fx.project, true).unwrap();
    let skipped = thread::load(&fx.project, &id).unwrap();
    assert!(!skipped.unconfigured_repo_seal.is_empty());
    assert!(skipped.historical_seal.is_empty());
    classify_old_seals(&fx.world.ctx(), &fx.project, true).unwrap();
    assert_eq!(
        thread::load(&fx.project, &id)
            .unwrap()
            .unconfigured_repo_seal,
        skipped.unconfigured_repo_seal
    );
    thread::update(&fx.project, &id, |t| {
        t.repo = fx.repo.to_string_lossy().into()
    })
    .unwrap();
    git(&fx.repo, &["merge", "--ff-only", &sha]);
    classify_old_seals(&fx.world.ctx(), &fx.project, true).unwrap();
    assert!(
        !thread::load(&fx.project, &id)
            .unwrap()
            .historical_seal
            .is_empty()
    );
}

#[test]
fn hand_started_reviewer_role_lands_as_an_ordinary_member() {
    let fx = configured();
    let (id, sha) = lane(&fx, 1);
    let task = job(&fx, &id);
    thread::update(&fx.project, &id, |t| {
        t.role = "reviewer".into();
        t.has_changes = None;
        t.changes_seal.clear();
    })
    .unwrap();
    assert_eq!(reviewer_ids(&fx.project).unwrap().len(), 0);
    let mut review = prepared(&fx);
    assert_eq!(review.members.len(), 1);
    assert_eq!(review.members[0].thread, id);
    assert_eq!(review.members[0].sha, sha);
    assert_eq!(
        thread::load(&fx.project, &id).unwrap().has_changes,
        Some(true)
    );
    let reviewer = review.reviewer.clone().unwrap();
    assert_ne!(reviewer, id);
    assert!(reviewer_ids(&fx.project).unwrap().contains(&reviewer));
    assert!(!reviewer_ids(&fx.project).unwrap().contains(&id));
    assert!(
        pending(
            &fx.project,
            fx.repo.to_str().unwrap(),
            &crate::events::list(&fx.project)
        )
        .iter()
        .all(|lane| lane.id != reviewer)
    );
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        1,
    );
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Complete);
    assert_eq!(
        thread::load(&fx.project, &id).unwrap().merged_sha,
        candidate
    );
    assert_eq!(
        crate::task::view(&fx.project, task).state,
        crate::task::State::Merged
    );
}

#[test]
fn whole_pile_lands_pushes_closes_and_prunes_once() {
    let fx = configured();
    let (first, a) = lane(&fx, 1);
    let (_, b) = lane(&fx, 2);
    let task = job(&fx, &first);
    let mut review = prepared(&fx);
    assert_eq!(review.members.len(), 2);
    assert_eq!(
        thread::list(&fx.project)
            .iter()
            .filter(|t| t.role == "reviewer")
            .count(),
        1
    );
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    for sha in [a, b] {
        assert!(
            Git::new(fx.world.ctx().runner, &fx.repo)
                .is_ancestor(&sha, &candidate)
                .unwrap()
        );
    }
    let remote = fx.world.home.path().join("published.git");
    git(
        &fx.repo,
        &["init", "--bare", "-q", remote.to_str().unwrap()],
    );
    review.push_remote = Some(remote.to_string_lossy().into_owned());
    // A box reviewer exports source refs, including locally run members.
    for member in &review.members {
        git(
            &fx.repo,
            &["push", remote.to_str().unwrap(), &member.branch],
        );
        thread::update(&fx.project, &member.thread, |t| {
            t.review_sources
                .insert(remote.to_string_lossy().into_owned(), member.sha.clone());
        })
        .unwrap();
    }
    save(&fx.project, &review).unwrap();
    // A later completion cannot enter the frozen review.
    let (later, _) = lane(&fx, 3);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        1,
    );
    fx.world.runner.calls.borrow_mut().clear();
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    let remote_checks = fx
        .world
        .runner
        .calls
        .borrow()
        .iter()
        .filter(|cmd| {
            cmd.program == "git"
                && cmd.args.iter().any(|arg| arg == "ls-remote")
                && cmd.args.last().is_some_and(|arg| arg == "refs/heads/main")
        })
        .map(|cmd| cmd.args.clone())
        .collect::<Vec<_>>();
    assert_eq!(remote_checks.len(), 2, "{remote_checks:?}");
    assert_eq!(review.phase, Phase::Landing);
    assert!(!review.close);
    review = load(&fx.project, &review.id).unwrap();
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Complete);
    assert_eq!(review.notices.len(), 1);
    assert!(review.notices[0].line.contains("merged"));
    assert!(review.notices[0].line.contains("publication verified"));
    assert!(review.notices[0].line.contains("install not required"));
    assert!(review.fast_forward && review.push && review.install && review.close && review.prune);
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]), candidate);
    assert_eq!(git(&remote, &["rev-parse", "main"]), candidate);
    assert_eq!(
        crate::task::view(&fx.project, task).state,
        crate::task::State::Merged
    );
    assert_eq!(
        thread::load(&fx.project, &first).unwrap().status,
        Status::Resolved
    );
    assert_eq!(
        thread::load(&fx.project, &later).unwrap().status,
        Status::Open
    );
    assert_eq!(
        pending(
            &fx.project,
            fx.repo.to_str().unwrap(),
            &crate::events::list(&fx.project)
        )
        .len(),
        1
    );
    for member in &review.members {
        assert!(
            Git::new(fx.world.ctx().runner, &fx.repo)
                .branch_head(&member.branch)
                .unwrap()
                .is_none()
        );
        assert!(
            git(
                &remote,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    &format!("refs/heads/{}", member.branch)
                ]
            )
            .is_empty()
        );
    }
    let calls = fx.world.runner.calls.borrow().len();
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(fx.world.runner.calls.borrow().len(), calls);
    assert!(
        Git::new(fx.world.ctx().runner, &fx.repo)
            .branch_head(&review.candidate_branch)
            .unwrap()
            .is_none()
    );
}

#[test]
fn no_change_seal_finishes_task_and_plan_without_review_or_git_polling() {
    let fx = configured();
    let (id, _) = lane(&fx, 1);
    let lane = thread::load(&fx.project, &id).unwrap();
    git(
        Path::new(&lane.worktree_path),
        &["reset", "--hard", &lane.base],
    );
    let seal = fx.seal_done(
        &id,
        1,
        2,
        &lane.base,
        "Published elsewhere; output SHA is in this report",
    );
    // Exercise the historical unknown classifier at first explicit review.
    assert!(start(&fx.world.ctx(), "demo", None).unwrap().is_none());
    let record = thread::load(&fx.project, &id).unwrap();
    assert_eq!(record.changes_seal, seal);
    assert_eq!(record.has_changes, Some(false));
    assert!(record.merged_sha.is_empty());
    let task = job(&fx, &id);
    assert_eq!(
        crate::task::view(&fx.project, task).state,
        crate::task::State::Finished
    );
    let plan = crate::contracts::Plan {
        schema: 1,
        revision: 1,
        next_step: 2,
        goal: "Deliver".into(),
        kind: "command".into(),
        what_you_get: "Done".into(),
        does: "Delivers".into(),
        steps: vec![crate::contracts::PlanStep {
            id: "s1".into(),
            text: "Deliver".into(),
            tasks: vec!["job-0001".into()],
            ..Default::default()
        }],
    };
    project::write_atomic(
        &fx.project.state_dir().join("plan.toml"),
        toml::to_string(&plan).unwrap().as_bytes(),
    )
    .unwrap();
    let calls = fx.world.runner.calls.borrow().len();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert_eq!(
        fx.world.runner.calls.borrow().len(),
        calls,
        "idle ticker must not consult git"
    );
    let mut plan = crate::plan::load(&fx.project).unwrap().unwrap();
    crate::plan::project_states(&fx.project, &mut plan);
    assert_eq!(plan.steps[0].state, crate::contracts::StepState::Done);
}

#[test]
fn exclusion_requires_candidate_without_that_lane_and_new_seal_for_next_pile() {
    let fx = configured();
    let (_, a) = lane(&fx, 1);
    let (excluded_lane, _) = lane(&fx, 2);
    let mut review = prepared(&fx);
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    let without = BTreeMap::from([(excluded_lane.clone(), "Needs a follow-up".into())]);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        without.clone(),
        vec![],
        1,
    );
    assert!(
        advance(&fx.world.ctx(), &fx.project, &mut review)
            .unwrap_err()
            .to_string()
            .contains("ancestry")
    );
    let before = fx.world.runner.calls.borrow().len();
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(
        fx.world.runner.calls.borrow().len(),
        before,
        "do not revalidate an unchanged invalid verdict each tick"
    );
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    git(Path::new(&reviewer.worktree_path), &["reset", "--hard", &a]);
    seal_verdict(&fx, &review, &a, "MERGE", without, vec![], 2);
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    let held = thread::load(&fx.project, &excluded_lane).unwrap();
    assert_eq!(held.status, Status::Open);
    assert!(!held.review_after.is_empty());
    assert!(
        pending(
            &fx.project,
            fx.repo.to_str().unwrap(),
            &crate::events::list(&fx.project)
        )
        .is_empty()
    );
    let next = commit_file(
        Path::new(&held.worktree_path),
        "fix.txt",
        "fixed",
        "follow-up",
    );
    fx.seal_done(&held.id, 1, 2, &next, "fixed");
    assert_eq!(
        pending(
            &fx.project,
            fx.repo.to_str().unwrap(),
            &crate::events::list(&fx.project)
        )
        .len(),
        1
    );
}

#[test]
fn dead_reviewer_needs_coordinator_once_after_retries_end() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let reviewer = review.reviewer.clone().unwrap();
    thread::update(&fx.project, &reviewer, |t| {
        t.status = Status::Failed;
        t.recovery_pending = true;
        t.error = "process gone".into();
    })
    .unwrap();
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert!(review.notices.is_empty());
    thread::update(&fx.project, &reviewer, |t| t.recovery_pending = false).unwrap();
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.notices.len(), 1);
    assert!(review.notices[0].line.contains("needs attention: reviewer"));
    assert!(review.notices[0].line.contains(&format!(
        "next: ha review retry demo --repo {}",
        crate::remote::quote(review.repo.as_str())
    )));
}

#[test]
fn reject_does_not_land_and_cancel_releases_unchanged_members() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let base = review.base.clone();
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "REJECT",
        BTreeMap::new(),
        vec![],
        1,
    );
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Rejected);
    assert_eq!(review.notices.len(), 1);
    assert!(review.notices[0].line.contains("rejected"));
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]), base);
    assert!(
        pending(
            &fx.project,
            fx.repo.to_str().unwrap(),
            &crate::events::list(&fx.project)
        )
        .is_empty()
    );

    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    assert!(require_follow_up(&fx.project, &review.members[0].thread).is_err());
    cancel_record(&fx.world.ctx(), &fx.project, &mut review, "hold this pile").unwrap();
    assert_eq!(review.phase, Phase::Cancelled);
    assert_eq!(
        pending(
            &fx.project,
            fx.repo.to_str().unwrap(),
            &crate::events::list(&fx.project)
        )
        .len(),
        1
    );
}

#[test]
fn persisted_landing_without_remote_completes_on_ticker_pass() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    review.push_remote = Some(String::new());
    review.verdict = Some(Verdict {
        verdict: "MERGE".into(),
        review: review.id.clone(),
        candidate: candidate.clone(),
        without: BTreeMap::new(),
        gates: vec![],
        gates_note: String::new(),
    });
    review.phase = Phase::Landing;
    save(&fx.project, &review).unwrap();
    fx.world.runner.calls.borrow_mut().clear();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    let landed = load(&fx.project, &review.id).unwrap();
    assert_eq!(landed.phase, Phase::Complete);
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]), candidate);
    let calls = fx.world.runner.calls.borrow();
    assert!(!calls.iter().any(|cmd| {
        cmd.program == "git"
            && cmd
                .args
                .iter()
                .any(|arg| ["ls-remote", "fetch", "push"].contains(&arg.as_str()))
    }));
}

#[test]
fn large_landing_shares_every_ticker_pass_with_other_projects_due_work() {
    use crate::runner::fake::ok;
    use crate::scenarios::agent_json;

    let fx = configured();
    for n in 1..=58 {
        lane(&fx, n);
    }
    let mut review = prepared(&fx);
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    review.push_remote = Some(String::new());
    review.verdict = Some(Verdict {
        verdict: "MERGE".into(),
        review: review.id.clone(),
        candidate: candidate.clone(),
        without: BTreeMap::new(),
        gates: vec![],
        gates_note: String::new(),
    });
    review.phase = Phase::Landing;
    save(&fx.project, &review).unwrap();

    let other = fx.world.project("other", "b.sock");
    let coordinator = other.coordinator().unwrap();
    let agent = agent_json(
        "w1",
        "w1:t1",
        "w1:p1",
        &coordinator.cwd,
        &coordinator.agent_name,
        "idle",
    );
    *fx.world.agents.borrow_mut() = format!("[{agent}]");
    *fx.world.panes.borrow_mut() = format!("[{}]", fx.world.coordinator_pane(&other));
    fx.world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
    fx.world.runner.on_fn(
        |cmd| cmd.program != "ssh" && cmd.display().contains("pane read"),
        |_| Ok(ok("❯ ")),
    );

    // Keep another project's machine due on every pass as well as its local
    // prompt. The machine phase must still run while this review is landing.
    thread::allocate(&other, |t| {
        t.status = Status::Open;
        t.machine = "box".into();
        t.machine_id = "1".into();
    })
    .unwrap();
    fx.world.runner.on(
        "machine list --json",
        ok(r#"[{"id":"1","label":"box","target":"me@box","session":"default","enabled":true}]"#),
    );
    fx.world.runner.on_fn(
        |cmd| cmd.program == "ssh",
        |_| Ok(ok("boot\tboot-1\nagents\t{\"result\":{\"agents\":[]}}\npanes\t{\"result\":{\"panes\":[]}}\n")),
    );

    let ctx = fx.world.ctx();
    let mut memory = crate::steps::Memory::new(&ctx);
    let mut previous = 0;
    let mut passes = 0;
    loop {
        other
            .update_coordinator(|c| {
                c.prime_pending = true;
                c.prime_sent = false;
            })
            .unwrap();
        fx.world.runner.calls.borrow_mut().clear();
        assert!(crate::ticker::tick_for_test(&ctx, &mut memory));
        passes += 1;
        let landed = load(&fx.project, &review.id).unwrap();
        let resolved = thread::list(&fx.project)
            .into_iter()
            .filter(|t| t.status == Status::Resolved)
            .count();
        assert!(resolved > previous);
        assert!(resolved - previous <= crate::threads::CLEANUP_BATCH_SIZE);
        previous = resolved;
        // This is a real due prompt in the slow phase after reviews, not just
        // evidence that the ticker entered the next project's cheap phase.
        assert!(other.coordinator().unwrap().prime_sent);
        assert!(fx.world.runner.calls.borrow().iter().any(|cmd| {
            cmd.display().contains("agent list")
                && cmd
                    .env
                    .iter()
                    .any(|(k, v)| k == "HERDR_SOCKET_PATH" && v == &coordinator.socket)
        }));
        assert!(memory.machine_views.contains_key("1"));
        assert_eq!(crate::events::remote_state(&other, "1").boot_id, "boot-1");
        if landed.phase == Phase::Complete {
            assert!(landed.close && landed.prune);
            break;
        }
        assert_eq!(landed.phase, Phase::Landing);
        assert!(landed.fast_forward && landed.push && landed.install);
        assert!(!landed.close);
        assert!(passes < 59);
    }
    assert_eq!(previous, 59); // members plus the reviewer
    assert!(passes > 1);
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]), candidate);
    assert!(
        Git::new(ctx.runner, &fx.repo)
            .branch_head(&review.candidate_branch)
            .unwrap()
            .is_none()
    );
}

#[test]
fn landing_verifies_the_push_destination_not_the_box_reviewers_clone() {
    // review-99 shape: a project may omit the harness repo, and the box's
    // main can already contain the candidate while the publication does not.
    for harness in [true, false] {
        for box_reviewer in [true, false] {
            for recorded_push in [false, true] {
                let fx = configured();
                let published = fx.world.home.path().join("published.git");
                let box_clone = fx.world.home.path().join("box.git");
                for repo in [&published, &box_clone] {
                    git(
                        &fx.repo,
                        &[
                            "clone",
                            "--bare",
                            "-q",
                            fx.repo.to_str().unwrap(),
                            repo.to_str().unwrap(),
                        ],
                    );
                }
                git(
                    &fx.repo,
                    &["remote", "add", "origin", box_clone.to_str().unwrap()],
                );
                git(
                    &fx.repo,
                    &[
                        "remote",
                        "set-url",
                        "--push",
                        "origin",
                        published.to_str().unwrap(),
                    ],
                );
                if harness {
                    let (mut settings, body) = fx.project.read_project_md().unwrap();
                    let mut row = settings.repos.remove(0);
                    row.push_remote = Some("origin".into());
                    row.box_path = Some(box_clone.to_string_lossy().into_owned());
                    row.publish_url = Some(published.to_string_lossy().into_owned());
                    std::fs::write(
                        fx.project.project_md(),
                        format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
                    )
                    .unwrap();
                    std::fs::write(
                        fx.world.ctx().config_dir.join("config.toml"),
                        format!("[[harness.repos]]\n{}", toml::to_string(&row).unwrap()),
                    )
                    .unwrap();
                }
                lane(&fx, 1);
                lane(&fx, 2);
                let mut review = prepared(&fx);
                if box_reviewer {
                    thread::update(&fx.project, review.reviewer.as_deref().unwrap(), |t| {
                        t.machine = "oci".into();
                        t.machine_id = "oci-id".into();
                    })
                    .unwrap();
                }
                let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
                git(
                    &fx.repo,
                    &[
                        "push",
                        box_clone.to_str().unwrap(),
                        &format!("{candidate}:refs/heads/main"),
                    ],
                );
                assert_ne!(git(&published, &["rev-parse", "main"]), candidate);
                let remote = if !harness && !box_reviewer {
                    // Literal URLs can have separate push destinations too.
                    git(
                        &fx.repo,
                        &[
                            "config",
                            &format!("url.{}.pushInsteadOf", published.display()),
                            box_clone.to_str().unwrap(),
                        ],
                    );
                    box_clone.to_str().unwrap()
                } else {
                    "origin"
                };
                review.push_remote = Some(remote.into());
                review.push = recorded_push;
                review.install_required = true;
                review.phase = Phase::Landing;
                review.verdict = Some(Verdict {
                    verdict: "MERGE".into(),
                    review: review.id.clone(),
                    candidate: candidate.clone(),
                    without: BTreeMap::new(),
                    gates: vec![],
                    gates_note: String::new(),
                });
                save(&fx.project, &review).unwrap();
                fx.world.runner.calls.borrow_mut().clear();
                let error = land_with_install(&fx.world.ctx(), &fx.project, &mut review, || {
                    assert_eq!(
                        git(&published, &["rev-parse", "main"]),
                        candidate,
                        "installation started without publication"
                    );
                    bail!("stop after publication")
                })
                .unwrap_err();
                assert_eq!(error.to_string(), "stop after publication");
                assert!(load(&fx.project, &review.id).unwrap().push);
                assert_eq!(fx.world.runner.count(&format!("push {remote}")), 1);
                let calls = fx.world.runner.calls.borrow();
                assert!(
                    calls
                        .iter()
                        .filter(|cmd| cmd.args.iter().any(|arg| arg == "ls-remote"))
                        .all(|cmd| cmd.args.contains(&published.to_string_lossy().into_owned()))
                );
                assert!(
                    calls
                        .iter()
                        .filter(|cmd| cmd.program == "git")
                        .all(|cmd| cmd.args[1] == review.repo)
                );
            }
        }
    }
}

#[test]
fn unlisted_harness_publish_url_is_the_integration_destination() {
    let fx = configured();
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    let mut row = settings.repos.remove(0);
    row.publish_url = Some("https://example.test/harness.git".into());
    std::fs::write(
        fx.project.project_md(),
        format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
    )
    .unwrap();
    std::fs::write(
        fx.world.ctx().config_dir.join("config.toml"),
        format!("[[harness.repos]]\n{}", toml::to_string(&row).unwrap()),
    )
    .unwrap();
    lane(&fx, 1);
    let review = prepared(&fx);
    assert!(review.install_required);
    assert_eq!(review.push_remote, row.publish_url);
}

#[test]
fn successful_push_without_remote_candidate_does_not_mark_publication_done() {
    use crate::runner::{RealRunner, Runner};
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    let published = fx.world.home.path().join("published.git");
    git(
        &fx.repo,
        &[
            "clone",
            "--bare",
            "-q",
            fx.repo.to_str().unwrap(),
            published.to_str().unwrap(),
        ],
    );
    review.push_remote = Some(published.to_string_lossy().into_owned());
    review.verdict = Some(Verdict {
        verdict: "MERGE".into(),
        review: review.id.clone(),
        candidate,
        without: BTreeMap::new(),
        gates: vec![],
        gates_note: String::new(),
    });
    review.phase = Phase::Landing;
    let runner = crate::runner::fake::FakeRunner::new();
    runner.on_fn(
        |cmd| cmd.program == "git" && cmd.args.iter().any(|arg| arg == "push"),
        |_| Ok(crate::runner::fake::ok("")),
    );
    runner.on_fn(|_| true, |cmd| RealRunner.run(cmd));
    let ctx = Ctx {
        runner: &runner,
        ..fx.world.ctx()
    };
    let error = land_with_install(&ctx, &fx.project, &mut review, || {
        panic!("unpublished integration installed")
    })
    .unwrap_err();
    assert!(
        error.to_string().starts_with("integration not published:"),
        "{error:#}"
    );
    assert!(!review.push);
    assert!(!load(&fx.project, &review.id).unwrap().push);
    assert_eq!(runner.count("ls-remote"), 2);
}

#[test]
fn configured_remote_failure_is_not_treated_as_local_only() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    let remote = fx.world.home.path().join("missing-remote.git");
    review.push_remote = Some(remote.to_string_lossy().into_owned());
    review.verdict = Some(Verdict {
        verdict: "MERGE".into(),
        review: review.id.clone(),
        candidate,
        without: BTreeMap::new(),
        gates: vec![],
        gates_note: String::new(),
    });
    review.phase = Phase::Landing;
    fx.world.runner.calls.borrow_mut().clear();
    let error = land_with_install(&fx.world.ctx(), &fx.project, &mut review, || {
        Ok(String::new())
    })
    .unwrap_err()
    .to_string();
    assert!(
        error.starts_with(&format!(
            "`git ls-remote {} refs/heads/main` failed: ",
            remote.display()
        )),
        "{error}"
    );
    assert_eq!(fx.world.runner.count("ls-remote"), 1);
    assert_eq!(review.phase, Phase::Landing);
    assert!(!review.push);
}

#[test]
fn landing_recovers_ref_before_marker_and_install_failure_without_early_task_done() {
    let fx = configured();
    let (id, _) = lane(&fx, 1);
    let task = job(&fx, &id);
    let mut review = prepared(&fx);
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        1,
    );
    review.verdict = Some(Verdict {
        verdict: "MERGE".into(),
        review: review.id.clone(),
        candidate: candidate.clone(),
        without: BTreeMap::new(),
        gates: vec![],
        gates_note: String::new(),
    });
    review.phase = Phase::Landing;
    review.install_required = true;
    save(&fx.project, &review).unwrap();
    // Crash after git accepted the FF but before fast_forward was recorded.
    git(&fx.repo, &["merge", "--ff-only", &candidate]);
    let refusal = cancel_record(&fx.world.ctx(), &fx.project, &mut review, "too late").unwrap_err();
    assert_eq!(
        crate::refusal::next(&refusal),
        Some(review.command(&fx.project, "retry").as_str())
    );
    assert!(review.fast_forward);
    assert!(
        land_with_install(&fx.world.ctx(), &fx.project, &mut review, || bail!(
            "REGRESSION demo: done 31→29; rolled back on mac, boxes untouched"
        ))
        .is_err()
    );
    assert!(review.fast_forward && review.push && !review.install && !review.close);
    assert_eq!(
        review.landing_summary(),
        "merged; no remote configured; REGRESSION demo: done 31→29; rolled back on mac, boxes untouched"
    );
    let notice = &review.notices.last().unwrap().line;
    assert!(notice.contains(&review.landing_summary()));
    assert_eq!(notice.matches("REGRESSION").count(), 1);
    fx.world.runner.calls.borrow_mut().clear();
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(
        fx.world.runner.calls.borrow().len(),
        0,
        "a regressed build must not reinstall each ticker pass"
    );
    let mut old_value = toml::Value::try_from(&review).unwrap();
    old_value.as_table_mut().unwrap().remove("install_result");
    let historical: Review = toml::from_str(&toml::to_string(&old_value).unwrap()).unwrap();
    assert!(historical.install_result.is_empty());
    assert_eq!(
        crate::task::view(&fx.project, task.clone()).state,
        crate::task::State::Merged
    );
    assert!(
        !crate::task::view(&fx.project, task.clone()).terminal_with_evidence(
            &fx.project,
            &crate::task::EvidenceSnapshot::load(&fx.project)
        )
    );
    // An explicit retry acknowledges the regression. A different install
    // failure must not leave the old regression blocking later ticker passes.
    let error = retry(&fx.world.ctx(), "demo", None).unwrap_err();
    assert!(error.to_string().starts_with("harness_repos_missing"));
    let mut restored = load(&fx.project, &review.id).unwrap();
    assert!(restored.install_result.is_empty());
    assert!(restored.attention.is_empty());
    land_with_install(&fx.world.ctx(), &fx.project, &mut restored, || {
        Ok("installed on mac, oci, a2; plan counts unchanged; records load".into())
    })
    .unwrap();
    assert_eq!(restored.phase, Phase::Complete);
    assert_eq!(
        restored.landing_summary(),
        "merged; no remote configured; installed on mac, oci, a2; plan counts unchanged; records load"
    );
    assert_eq!(
        crate::task::view(&fx.project, task).state,
        crate::task::State::Installed
    );
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]), candidate);
    land_with_install(&fx.world.ctx(), &fx.project, &mut restored, || {
        panic!("install replayed")
    })
    .unwrap();
}

#[test]
fn landing_completes_while_a_merged_member_owes_cleanup() {
    let fx = configured();
    let (id, sha) = lane(&fx, 1);
    let record = thread::load(&fx.project, &id).unwrap();
    let dir = Path::new(&record.thread_dir);
    std::fs::create_dir_all(dir).unwrap();
    std::fs::File::create(dir.join("large.bin"))
        .unwrap()
        .set_len(201 * 1024 * 1024)
        .unwrap();
    fx.seal_done(&id, 1, 2, &sha, "[large](large.bin)\n");
    let mut review = prepared(&fx);
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    review.verdict = Some(Verdict {
        verdict: "MERGE".into(),
        review: review.id.clone(),
        candidate,
        without: BTreeMap::new(),
        gates: vec![],
        gates_note: String::new(),
    });
    review.phase = Phase::Landing;
    save(&fx.project, &review).unwrap();
    land_with_install(&fx.world.ctx(), &fx.project, &mut review, || {
        Ok(String::new())
    })
    .unwrap();
    assert_eq!(review.phase, Phase::Complete);
    let lane = thread::load(&fx.project, &id).unwrap();
    assert_eq!(lane.status, Status::Resolved);
    assert!(lane.cleanup_pending);
    assert!(!lane.merged_sha.is_empty());
}

#[test]
fn moved_tip_refreshes_same_reviewer_once_then_releases_for_fresh_review() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let reviewer = review.reviewer.clone();
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    let tip = commit_file(&fx.repo, "other.txt", "other", "integration moved");
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        1,
    );
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Reviewing);
    assert_eq!(review.reviewer, reviewer);
    assert_eq!(review.moved, 1);
    assert_eq!(review.refresh_tip.as_deref(), Some(tip.as_str()));
    assert!(!review.fast_forward);
    // The persisted follow-up is submitted once by advance. Simulate that
    // receipt, and let the same reviewer merge the new tip and reseal.
    review.refresh_tip = None;
    let lane = thread::load(&fx.project, reviewer.as_deref().unwrap()).unwrap();
    git(
        Path::new(&lane.worktree_path),
        &["merge", "--no-edit", &tip],
    );
    let refreshed = git(Path::new(&lane.worktree_path), &["rev-parse", "HEAD"]);
    seal_verdict(
        &fx,
        &review,
        &refreshed,
        "MERGE",
        BTreeMap::new(),
        vec![],
        2,
    );
    commit_file(&fx.repo, "again.txt", "again", "integration moved twice");
    save(&fx.project, &review).unwrap();
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Cancelled);
    assert!(!review.fast_forward);
}

#[test]
fn old_fork_with_docs_only_changes_does_not_select_main_code_gates() {
    let fx = configured();
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    settings.repos[0].gates = Some(vec![project::Gate {
        command: "code-gate".into(),
        paths: Some(vec!["src/**".into()]),
        ..Default::default()
    }]);
    std::fs::write(
        fx.project.project_md(),
        format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
    )
    .unwrap();
    let (id, _) = lane_unsealed(&fx, 1);
    let record = thread::load(&fx.project, &id).unwrap();
    let wt = Path::new(&record.worktree_path);
    git(wt, &["reset", "--hard", &record.base]);
    let sha = commit_file(wt, "docs/note.md", "docs", "docs only");
    commit_file(&fx.repo, "src/new.rs", "new code", "main changes code");
    fx.seal_done(&id, 1, 1, &sha, "finished\n");
    let review = prepared(&fx);
    assert!(review.selected_gates.is_empty());
}

#[test]
fn gates_are_selected_from_pile_and_reviewer_fix_paths_and_failures_refuse() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    review.gates = vec![
        project::Gate {
            command: "code-gate".into(),
            paths: Some(vec!["src/**".into()]),
            ..Default::default()
        },
        project::Gate {
            command: "docs-gate".into(),
            paths: Some(vec!["docs/**".into()]),
            ..Default::default()
        },
    ];
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    let candidate = commit_file(
        Path::new(&reviewer.worktree_path),
        "docs/fix.md",
        "clarify",
        "reviewer fix",
    );
    let only_code = vec![GateRun {
        command: "code-gate".into(),
        exit: 0,
    }];
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        only_code.clone(),
        1,
    );
    assert!(
        advance(&fx.world.ctx(), &fx.project, &mut review)
            .unwrap_err()
            .to_string()
            .contains("candidate-selected gate")
    );
    let mut gates = only_code;
    gates.push(GateRun {
        command: "docs-gate".into(),
        exit: 1,
    });
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        gates.clone(),
        2,
    );
    assert!(advance(&fx.world.ctx(), &fx.project, &mut review).is_err());
    gates[1].exit = 0;
    seal_verdict(&fx, &review, &candidate, "MERGE", BTreeMap::new(), gates, 3);
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Complete);
}

#[test]
fn historical_installed_tasks_load_without_old_review_or_hold_files() {
    let fx = configured();
    let text = r#"schema = 1
id = "job-0001"
title = "Previously installed"
authority = ["request:q-1"]
acceptance = ["Works"]
attempts = ["t-9999"]
rounds = ["r118"]
[wait]
kind = "round"
target = "r118"
snapshot = "anything"
since = "then"
[[installed]]
at = "2026-09-25"
command = "ha harness install"
[[verified]]
at = "2026-09-25"
command = "old evidence"
acceptance = [1]
"#;
    let dir = fx.project.record_dir_for_write("tasks").unwrap();
    std::fs::write(dir.join("job-0001.toml"), text).unwrap();
    let task = crate::task::load(&fx.project, "job-0001").unwrap();
    assert_eq!(
        crate::task::view(&fx.project, task).state,
        crate::task::State::Installed
    );
}
