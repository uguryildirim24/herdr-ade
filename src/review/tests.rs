use super::*;
use crate::testkit::{Fx, commit_file, fixture, git};

mod attachments;
mod holds;
mod starts;
mod wall_gate;
mod withdrawals;
use std::path::Path;

fn configured() -> Fx {
    let fx = fixture();
    std::fs::write(
        fx.repo.join(".git/info/exclude"),
        ".worktrees/\n.herdr-project/\n.reports/\n",
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
            &review.base,
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
    // The reviewer, not review preparation, integrates the frozen member set.
    for member in &review.members {
        let out = std::process::Command::new("git")
            .args(["merge", "--no-edit", &member.sha])
            .current_dir(&checkout)
            .output()
            .unwrap();
        if !out.status.success() {
            git(&checkout, &["merge", "--abort"]);
            break;
        }
    }
    review
}
fn landing_verdict(review: &mut Review, candidate: &str) {
    review.verdict = Some(Verdict {
        verdict: "MERGE".into(),
        review: review.id.clone(),
        candidate: candidate.into(),
        without: BTreeMap::new(),
        gates: vec![],
        gates_note: String::new(),
        evidence_only: false,
        withdrawn_only: false,
    });
    review.phase = Phase::Landing;
}
fn post_seal_follow_up(fx: &Fx, id: &str, event: &crate::contracts::Event) -> PathBuf {
    let checkout = thread::load(&fx.project, id).unwrap().worktree_path;
    let done = event.payload.done.as_ref().unwrap();
    let path = Path::new(&checkout).join(&done.report_path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        thread::artifact(&fx.project, &done.artifact).unwrap(),
    )
    .unwrap();
    thread::update(&fx.project, id, |lane| {
        lane.review_after = event.id.clone();
        lane.follow_ups.push(thread::FollowUp {
            attempt: 1,
            state: thread::FollowUpState::Delivered,
            after_seal: event.id.clone(),
            delivered_at: "2026-09-18T00:00:00Z".into(),
            ..Default::default()
        });
    })
    .unwrap();
    path
}
fn candidate(fx: &Fx, review: &Review) -> String {
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    git(Path::new(&reviewer.worktree_path), &["rev-parse", "HEAD"])
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
        evidence_only: false,
        withdrawn_only: false,
        without,
        gates,
        gates_note: String::new(),
    };
    #[derive(Serialize)]
    struct AcceptanceRows {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        acceptance: Vec<crate::contracts::CriterionEvidence>,
    }
    let tasks = crate::task::list_with_errors(&fx.project).0;
    let mut acceptance = Vec::new();
    for member in &review.members {
        for task in tasks
            .iter()
            .filter(|task| task.attempts.last() == Some(&member.thread))
        {
            for criterion in 1..=task.acceptance.len() {
                acceptance.push(crate::contracts::CriterionEvidence {
                    thread: member.thread.clone(),
                    event: member.event.clone(),
                    criterion,
                    condition: task.acceptance[criterion - 1].clone(),
                    established: true,
                    evidence: format!("artifact {}: observed behavior", member.artifact),
                });
            }
        }
    }
    let report = format!(
        "+++\n{}{}+++\n\nChecked the complete pile.\n",
        toml::to_string(&verdict).unwrap(),
        toml::to_string(&AcceptanceRows { acceptance }).unwrap()
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
fn retry_rebuilds_pre_contract_packet_and_delivers_it_to_a_parked_reviewer() {
    for parked in [false, true] {
        let fx = configured();
        let (id, _) = lane(&fx, 1);
        let member_task = job(&fx, &id);
        let original = format!(
            "# Original brief\nAcceptance: {}\n",
            member_task.acceptance[0]
        );
        let member_hash = thread::store_artifact(&fx.project, original.as_bytes()).unwrap();
        thread::update(&fx.project, &id, |t| {
            t.launch.brief_hash = member_hash.clone()
        })
        .unwrap();
        let mut review = prepared(&fx);
        let reviewer = review.reviewer.clone().unwrap();
        let old_packet = "Pre-contract pile: SHAs and reports only\n";
        let old_hash = thread::store_artifact(&fx.project, old_packet.as_bytes()).unwrap();
        project::write_atomic(
            &thread::task_path_for_write(&fx.project, &reviewer).unwrap(),
            old_packet.as_bytes(),
        )
        .unwrap();
        thread::update(&fx.project, &reviewer, |t| {
            t.launch.brief_hash = old_hash.clone();
            t.parked = parked;
        })
        .unwrap();
        let candidate = candidate(&fx, &review);
        fx.seal_done(&reviewer, 1, 1, &candidate, &format!(
            "+++\nreview = {:?}\nverdict = \"MERGE\"\ncandidate = {candidate:?}\n+++\nOld review contract\n", review.id
        ));
        assert!(
            advance(&fx.world.ctx(), &fx.project, &mut review)
                .unwrap_err()
                .to_string()
                .contains("acceptance not established")
        );
        let retried = retry(&fx.world.ctx(), "demo", None).unwrap().unwrap();
        assert_eq!(retried.phase, Phase::Reviewing);
        assert!(retried.verdict.is_none());
        let lane = thread::load(&fx.project, &reviewer).unwrap();
        assert_ne!(lane.launch.brief_hash, old_hash);
        let packet = std::fs::read_to_string(thread::task_path(&fx.project, &reviewer)).unwrap();
        let brief =
            String::from_utf8(thread::artifact(&fx.project, &lane.launch.brief_hash).unwrap())
                .unwrap();
        for text in [&original, "[[acceptance]]", &review.members[0].event] {
            assert!(packet.contains(text));
            assert!(brief.contains(text));
            if parked {
                assert!(
                    lane.follow_ups
                        .iter()
                        .any(|follow_up| follow_up.text.contains(text))
                );
            }
        }
        assert_eq!(
            thread::artifact(&fx.project, &old_hash).unwrap(),
            old_packet.as_bytes()
        );
        assert_eq!(
            thread::load(&fx.project, &id).unwrap().launch.brief_hash,
            member_hash
        );
        assert!(
            thread::load(&fx.project, &id)
                .unwrap()
                .review_after
                .is_empty()
        );
        assert_eq!(git(&fx.repo, &["rev-parse", "main"]), review.base);
    }
}

#[test]
fn evidence_only_rejection_allows_retry_or_automatic_review_without_member_follow_ups() {
    for explicit_retry in [false, true] {
        let fx = configured();
        let (id, _) = lane(&fx, 1);
        let mut review = prepared(&fx);
        let candidate = candidate(&fx, &review);
        // The reviewer can read neither the member's original brief nor its
        // task record. This is not a verdict about the implementation.
        std::fs::write(
            fx.project
                .record_dir_for_write("tasks")
                .unwrap()
                .join("job-unreadable.toml"),
            "invalid = [",
        )
        .unwrap();
        fx.seal_done(review.reviewer.as_deref().unwrap(), 1, 1, &candidate, &format!(
            "+++\nreview = {:?}\nverdict = \"REJECT\"\ncandidate = {candidate:?}\nevidence_only = true\n+++\nIncomplete review evidence, not failed implementation\n", review.id
        ));
        advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
        assert_eq!(review.phase, Phase::Rejected);
        assert!(review.verdict.as_ref().unwrap().evidence_only);
        let member = thread::load(&fx.project, &id).unwrap();
        assert!(member.review_after.is_empty());
        assert!(member.review_reason.is_empty());
        assert_eq!(
            pending(
                &fx.project,
                fx.repo.to_str().unwrap(),
                &crate::events::list(&fx.project)
            )
            .len(),
            1
        );
        assert_eq!(git(&fx.repo, &["rev-parse", "main"]), review.base);
        // Recover a new allocation-before-binding intent without a live agent.
        let next_reviewer = fx.thread("fresh reviewer");
        thread::update(&fx.project, &next_reviewer, |t| {
            t.role = "reviewer".into();
            t.review_id = "review-2".into();
            t.pane_id.clear();
        })
        .unwrap();
        let fresh = if explicit_retry {
            retry(&fx.world.ctx(), "demo", None).unwrap().unwrap()
        } else {
            tick(&fx.world.ctx(), &fx.project).unwrap();
            load(&fx.project, "review-2").unwrap()
        };
        assert_eq!(fresh.phase, Phase::Reviewing);
        assert_ne!(fresh.reviewer, review.reviewer);
        assert_eq!(fresh.members[0].event, review.members[0].event);
        assert!(task(&fx.project, &fresh).contains("[[acceptance]]"));
        assert_eq!(
            load(&fx.project, &review.id).unwrap().phase,
            Phase::Rejected
        );
    }
}

#[test]
fn evidence_only_cannot_be_used_for_merge_or_member_exclusions() {
    let fx = configured();
    lane(&fx, 1);
    let review = prepared(&fx);
    let candidate = candidate(&fx, &review);
    let git = Git::new(&fx.world.runner, fx.repo.to_str().unwrap());
    for (n, word, without) in [
        (1, "MERGE", String::new()),
        (
            2,
            "REJECT",
            format!(
                "without = {{ {} = \"broken work\" }}\n",
                review.members[0].thread
            ),
        ),
    ] {
        let reviewer = review.reviewer.as_deref().unwrap();
        fx.seal_done(reviewer, 1, n, &candidate, &format!(
            "+++\nreview = {:?}\nverdict = {word:?}\ncandidate = {candidate:?}\nevidence_only = true\n{without}+++\n", review.id
        ));
        let events = crate::events::checked(&fx.project).unwrap();
        let lane = thread::load(&fx.project, reviewer).unwrap();
        assert!(
            verdict(
                &fx.world.ctx(),
                &fx.project,
                &review,
                sealed(&events, &lane).unwrap(),
                &git
            )
            .unwrap_err()
            .to_string()
            .contains("evidence_only requires")
        );
    }
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
    let candidate = candidate(&fx, &review);
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
fn offline_box_does_not_hold_local_task_or_local_pile_in_the_same_project() {
    let fx = configured();
    lane(&fx, 1);
    let review = prepared(&fx);
    let candidate = candidate(&fx, &review);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        1,
    );
    let remote = thread::allocate(&fx.project, |t| {
        t.status = Status::Open;
        t.machine = "box".into();
        t.repo = "/unrelated-box-repo".into();
    })
    .unwrap();
    let local = thread::allocate(&fx.project, |t| {
        t.status = Status::Open;
        t.launch.kind = "claude".into();
        t.provider_wait_started = project::now();
    })
    .unwrap();
    fx.world.runner.on(
        "machine list --json",
        crate::runner::fake::ok(
            r#"[{"id":"box","label":"box","target":"box","session":"default","enabled":true}]"#,
        ),
    );
    fx.world.runner.on_fn(
        |cmd| cmd.program == "ssh",
        |_| {
            Ok(crate::runner::fake::fail(
                255,
                "ssh: connect to host box: Operation timed out",
            ))
        },
    );
    let before = thread::load(&fx.project, &remote.id).unwrap();
    let ctx = fx.world.ctx();
    crate::ticker::tick_for_test(&ctx, &mut crate::steps::Memory::new(&ctx));
    assert!(
        thread::load(&fx.project, &local.id)
            .unwrap()
            .provider_wait_started
            .is_empty()
    );
    crate::testkit::assert_failed_observation_only(
        &before,
        &thread::load(&fx.project, &remote.id).unwrap(),
    );
    assert!(load(&fx.project, &review.id).unwrap().fast_forward);
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]), candidate);
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
    for word in ["REJECT", "MERGE"] {
        let fx = configured();
        lane(&fx, 1);
        let mut review = prepared(&fx);
        review.gates = vec![project::Gate {
            command: "printf proof".into(),
            ..Default::default()
        }];
        save(&fx.project, &review).unwrap();
        fx.world.runner.on_fn(
            |cmd| cmd.program == "sh",
            |cmd| crate::runner::Runner::run(&crate::runner::RealRunner, cmd),
        );
        let reviewer = review.reviewer.as_deref().unwrap();
        let candidate = candidate(&fx, &review);
        seal_verdict(&fx, &review, &candidate, word, BTreeMap::new(), vec![], 1);
        let event =
            crate::events::latest_done_event(&crate::events::list(&fx.project), reviewer, 1)
                .unwrap()
                .clone();
        post_seal_follow_up(&fx, reviewer, &event);
        let seal = event.id;
        tick(&fx.world.ctx(), &fx.project).unwrap();
        let checked = load(&fx.project, &review.id).unwrap();
        assert_eq!(checked.phase, Phase::Reviewing);
        assert_eq!(checked.checked_event, seal);
        assert_eq!(checked.verdict.as_ref().unwrap().verdict, word);
        let runs = fx.world.runner.count("printf proof");
        tick(&fx.world.ctx(), &fx.project).unwrap();
        assert_eq!(fx.world.runner.count("printf proof"), runs);
        thread::update(&fx.project, reviewer, |lane| {
            lane.review_after.clear();
            lane.follow_ups[0].state = thread::FollowUpState::Closed;
        })
        .unwrap();
        tick(&fx.world.ctx(), &fx.project).unwrap();
        assert_eq!(
            load(&fx.project, &review.id).unwrap().phase,
            if word == "REJECT" {
                Phase::Rejected
            } else {
                Phase::Complete
            }
        );
        assert_eq!(
            fx.world.runner.count("printf proof"),
            runs,
            "release reuses the checked seal without executing gates twice"
        );
    }
}

#[test]
fn sealed_reviewer_verdict_is_checked_in_the_arrival_pass_without_a_local_session() {
    let fx = configured();
    lane(&fx, 1);
    let review = prepared(&fx);
    let candidate = candidate(&fx, &review);
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
    let candidate = candidate(&fx, &review);
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
    post_seal_follow_up(&fx, reviewer, &sealed_event);
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
fn box_follow_up_restores_only_after_box_checkout_is_unchanged_and_clean() {
    let fx = configured();
    let (id, sha) = lane(&fx, 1);
    let event_id = fx.seal_done(&id, 1, 1, &sha, "report\n");
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
    fx.world
        .runner
        .on("ssh", crate::runner::fake::ok(&format!("{sha}\n")));
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
    let candidate = candidate(&fx, &review);
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
    let path = post_seal_follow_up(&fx, reviewer, &sealed_event);
    std::fs::write(&path, "changed report").unwrap();
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
    assert_eq!(
        thread::load(&fx.project, reviewer).unwrap().review_after,
        sealed_event.id
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
    assert!(crate::task::EvidenceSnapshot::load(&fx.project).lane_done(&record));
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
    assert!(crate::plan::show(&fx.world.ctx(), "demo").is_ok());
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
    let candidate = candidate(&fx, &review);
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
    let candidate = candidate(&fx, &review);
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
    assert!(review.candidate_branch.is_empty());
    assert!(git(&fx.repo, &["branch", "--list", "review/*"]).is_empty());
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
    let candidate = candidate(&fx, &review);
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
fn merits_reject_needs_follow_ups_and_cancel_releases_unchanged_members() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let base = review.base.clone();
    let candidate = candidate(&fx, &review);
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

    let member = &review.members[0];
    let rejected = thread::load(&fx.project, &member.thread).unwrap();
    assert_eq!(rejected.review_after, member.event);
    assert_eq!(
        rejected.review_reason,
        "pile rejected; follow up before the next review"
    );
    assert!(retry(&fx.world.ctx(), "demo", None).is_err());
    assert!(start(&fx.world.ctx(), "demo", None).unwrap().is_none());
    // The unspecified historical verdict still loads and remains conservative.
    assert!(
        !load(&fx.project, &review.id)
            .unwrap()
            .verdict
            .unwrap()
            .evidence_only
    );
    fx.seal_done(
        &member.thread,
        1,
        2,
        &member.sha,
        "follow-up fixed the implementation\n",
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
    let candidate = candidate(&fx, &review);
    review.push_remote = Some(String::new());
    landing_verdict(&mut review, &candidate);
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
    let candidate = candidate(&fx, &review);
    review.push_remote = Some(String::new());
    landing_verdict(&mut review, &candidate);
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
        |_| {
            Ok(ok(&crate::box_helper::tests::ready(
                crate::steps::CourierManifest {
                    boot_id: "boot-1".into(),
                    agents: Some(Vec::new()),
                    panes: Some(Vec::new()),
                    ..Default::default()
                },
            )))
        },
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
    assert!(review.candidate_branch.is_empty());
    assert!(git(&fx.repo, &["branch", "--list", "review/*"]).is_empty());
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
                let candidate = candidate(&fx, &review);
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
                landing_verdict(&mut review, &candidate);
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
    let candidate = candidate(&fx, &review);
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
    landing_verdict(&mut review, &candidate);
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
    let candidate = candidate(&fx, &review);
    let remote = fx.world.home.path().join("missing-remote.git");
    review.push_remote = Some(remote.to_string_lossy().into_owned());
    landing_verdict(&mut review, &candidate);
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
    let candidate = candidate(&fx, &review);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        1,
    );
    landing_verdict(&mut review, &candidate);
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
fn journey_d22_needs_an_independent_review_while_the_later_lane_is_unsealed() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    assert!(!crate::journey::independent_review(None, "later", false).unwrap());
    assert!(crate::journey::independent_review(Some(&review), "later", false).unwrap());
    assert!(crate::journey::independent_review(Some(&review), "later", true).is_err());
    let mut later = review.members[0].clone();
    later.thread = "later".into();
    review.members.push(later);
    assert!(crate::journey::independent_review(Some(&review), "later", false).is_err());
}

#[test]
fn journey_d22_starts_automatically_only_after_the_later_lane_exists() {
    let fx = configured();
    assert!(start(&fx.world.ctx(), "demo", None).unwrap().is_none());
    let lock = try_operation_lock(&fx.world.ctx(), fx.repo.to_str().unwrap())
        .unwrap()
        .unwrap();
    let (mac, _) = lane(&fx, 1);
    let now = "2026-09-18T10:01:05Z".parse().unwrap();
    tick_observed_at(&fx.world.ctx(), &fx.project, |_| true, now).unwrap();
    assert!(list(&fx.project).unwrap().is_empty());
    let (later, _) = lane_unsealed(&fx, 2);
    thread::update(&fx.project, &later, |t| {
        t.created = "2026-09-18T10:01:02Z".into()
    })
    .unwrap();
    // Reuse the allocation-before-binding fixture; this is not a manual start.
    let reviewer = fx.thread("pile reviewer");
    thread::update(&fx.project, &reviewer, |t| {
        t.role = "reviewer".into();
        t.review_id = "review-1".into();
        t.pane_id.clear();
    })
    .unwrap();
    drop(lock);
    tick_observed_at(&fx.world.ctx(), &fx.project, |_| true, now).unwrap();
    let review = list(&fx.project).unwrap().remove(0);
    assert_eq!(review.members.len(), 1);
    assert_eq!(review.members[0].thread, mac);
    assert!(crate::journey::independent_review(Some(&review), &later, false).unwrap());
}

#[test]
fn post_install_observation_failure_reports_without_undoing_landing_facts_or_counts() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    review.fast_forward = true;
    review.push = true;
    review.install_required = true;
    review.install = true;
    review.install_result = "installed on mac; plan counts unchanged; records load; ticker first full pass pending; journey pending".into();
    save(&fx.project, &review).unwrap();
    let counts = crate::plan::counts(&fx.project).unwrap();
    let result = "REVIEW demo/review-1: ticker first full pass: FAIL: session EINVAL; transient: cleared by second full pass; JOURNEY PASS";
    post_install_result(&fx.world.ctx(), &fx.project.slug, &review.id, result).unwrap();
    post_install_result(&fx.world.ctx(), &fx.project.slug, &review.id, result).unwrap();
    let record = load(&fx.project, &review.id).unwrap();
    assert!(record.fast_forward && record.push && record.install);
    assert_eq!(crate::plan::counts(&fx.project).unwrap(), counts);
    assert!(record.attention.is_empty());
    assert!(record.landing_summary().contains("FAIL: session EINVAL"));
    assert!(record.landing_summary().contains("transient"));
    assert!(!record.landing_summary().contains("pending"));
    assert_eq!(record.install_result.matches(result).count(), 1);
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
    let candidate = candidate(&fx, &review);
    landing_verdict(&mut review, &candidate);
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
    let candidate = candidate(&fx, &review);
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
    let event = crate::events::latest_done_event(&crate::events::list(&fx.project), &lane.id, 1)
        .unwrap()
        .clone();
    post_seal_follow_up(&fx, &lane.id, &event);
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.checked_event, event.id);
    assert_ne!(review.verdict_event, event.id);
    assert_eq!(review.phase, Phase::Reviewing);
    thread::update(&fx.project, &lane.id, |t| {
        t.review_after.clear();
        t.follow_ups[0].state = thread::FollowUpState::Closed;
    })
    .unwrap();
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
fn gates_are_observed_from_pile_and_reviewer_fix_paths_not_declared_exits() {
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
            .contains("not established")
    );
    let passing = std::rc::Rc::new(std::cell::Cell::new(false));
    let observed = passing.clone();
    fx.world.runner.on_fn(
        |cmd| cmd.program == "sh",
        move |cmd| {
            if !observed.get() && cmd.args.iter().any(|arg| arg.contains("docs-gate")) {
                Ok(crate::runner::fake::fail(1, "checker broke"))
            } else {
                Ok(crate::runner::fake::ok("code passed"))
            }
        },
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
    let error = advance(&fx.world.ctx(), &fx.project, &mut review)
        .unwrap_err()
        .to_string();
    assert!(error.contains("gate failed:"), "{error}");
    assert!(error.contains("reviewer verdict MERGE disagrees"));
    assert!(error.contains("checker broke"));
    passing.set(true);
    // No declarations at all: the two real ADE observations suffice.
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        3,
    );
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Complete);
}

fn criterion_reason(lane: &str, event: &str, condition: &str, established: bool) -> String {
    format!(
        "[[acceptance]]\nthread = {lane:?}\nevent = {event:?}\ncriterion = 1\ncondition = {condition:?}\nestablished = {established}\nevidence = \"immutable report artifact and inspected source coverage\""
    )
}

#[test]
fn partial_no_change_research_cannot_unlock_dependents_but_evidenced_no_change_can() {
    let fx = configured();
    let (id, _) = lane(&fx, 1);
    let lane = thread::load(&fx.project, &id).unwrap();
    git(
        Path::new(&lane.worktree_path),
        &["reset", "--hard", &lane.base],
    );
    let partial = fx.seal_done(
        &id,
        1,
        2,
        &lane.base,
        "Polished ledger: S1 complete; S2 complete (appendix not inspected).\n",
    );
    thread::update(&fx.project, &id, |lane| {
        lane.changes_seal = partial.clone();
        lane.has_changes = Some(false);
        lane.status = Status::Resolved;
    })
    .unwrap();
    let task = job(&fx, &id);
    let mut build = task.clone();
    build.id = "job-0002".into();
    build.attempts.clear();
    std::fs::write(
        fx.project
            .record_dir_for_write("tasks")
            .unwrap()
            .join("job-0002.toml"),
        toml::to_string(&build).unwrap(),
    )
    .unwrap();
    let mut plan = crate::contracts::Plan {
        does: "Exhaustive evidence before implementation".into(),
        steps: vec![
            crate::contracts::PlanStep {
                id: "s-1".into(),
                tasks: vec![task.id.clone()],
                ..Default::default()
            },
            crate::contracts::PlanStep {
                id: "s-2".into(),
                tasks: vec![build.id.clone()],
                after: vec!["s-1".into()],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    project::write_atomic(
        &fx.project.state_dir().join("plan.toml"),
        toml::to_string(&plan).unwrap().as_bytes(),
    )
    .unwrap();
    crate::plan::project_states(&fx.project, &mut plan);
    assert_eq!(
        plan.steps[0].state,
        crate::contracts::StepState::Done,
        "finish counts do not become an acceptance ladder"
    );
    assert!(
        crate::plan::check_prerequisites(&fx.project, &build.id)
            .unwrap_err()
            .to_string()
            .contains("acceptance not established")
    );
    crate::threads::attest(
        &fx.world.ctx(),
        "demo",
        &id,
        &criterion_reason(&id, &partial, &task.acceptance[0], false),
    )
    .unwrap();
    assert!(
        crate::task::load(&fx.project, &task.id)
            .unwrap()
            .acceptance_review
            .is_some()
    );
    assert!(crate::plan::check_prerequisites(&fx.project, &build.id).is_err());
    // The useful no-change outcome is allowed once independently evidenced;
    // no commit, merge, gate or live install is invented for report-only work.
    let complete = fx.seal_done(
        &id,
        1,
        3,
        &lane.base,
        "Full source and appendix inspected; exhaustive ledger with durable references.\n",
    );
    thread::update(&fx.project, &id, |lane| {
        lane.changes_seal = complete.clone()
    })
    .unwrap();
    assert!(
        crate::plan::check_prerequisites(&fx.project, &build.id).is_err(),
        "old judgment cannot accept a new seal"
    );
    crate::threads::attest(
        &fx.world.ctx(),
        "demo",
        &id,
        &criterion_reason(&id, &complete, &task.acceptance[0], true),
    )
    .unwrap();
    crate::plan::check_prerequisites(&fx.project, &build.id).unwrap();
    assert!(
        thread::load(&fx.project, &id)
            .unwrap()
            .merged_sha
            .is_empty()
    );
    assert!(list(&fx.project).unwrap().is_empty());
    let source = crate::events::load(&fx.project, &complete)
        .unwrap()
        .payload
        .done
        .unwrap()
        .artifact;
    let source_path = crate::events::artifact_path(&fx.project, &source);
    let original = std::fs::read(&source_path).unwrap();
    std::fs::remove_file(&source_path).unwrap();
    assert!(
        crate::plan::check_prerequisites(&fx.project, &build.id)
            .unwrap_err()
            .to_string()
            .contains("source report missing or corrupt"),
        "a judgment cannot replace lost source evidence"
    );
    std::fs::write(&source_path, "corrupted report").unwrap();
    assert!(crate::plan::check_prerequisites(&fx.project, &build.id).is_err());
    std::fs::write(&source_path, original).unwrap();
    crate::plan::check_prerequisites(&fx.project, &build.id).unwrap();
    let mut rewritten = crate::task::load(&fx.project, &task.id).unwrap();
    rewritten.acceptance[0] = "A newly requested outcome".into();
    std::fs::write(
        fx.project
            .record_dir_for_write("tasks")
            .unwrap()
            .join("job-0001.toml"),
        toml::to_string(&rewritten).unwrap(),
    )
    .unwrap();
    assert!(
        crate::plan::check_prerequisites(&fx.project, &build.id).is_err(),
        "old acceptance cannot cover new intent"
    );
}

#[test]
fn an_existing_independent_critic_can_establish_no_change_acceptance() {
    let fx = configured();
    let id = fx.thread("research");
    let task = job(&fx, &id);
    let event = fx.seal_done(&id, 1, 1, "base", "Full cited evidence");
    thread::update(&fx.project, &id, |lane| {
        lane.changes_seal = event.clone();
        lane.has_changes = Some(false);
    })
    .unwrap();
    let critic = fx.thread("requested critique");
    thread::update(&fx.project, &critic, |lane| lane.role = "critic".into()).unwrap();
    let snapshot = crate::task::EvidenceSnapshot::load(&fx.project);
    assert!(crate::task::require_accepted(&fx.project, &task, &snapshot).is_err());
    let reason = criterion_reason(&id, &event, &task.acceptance[0], true);
    fx.seal_done(
        &critic,
        1,
        1,
        "base",
        &format!("+++\nverdict = \"FAIL\"\n{reason}\n+++\nMissing coverage"),
    );
    assert!(
        crate::task::require_accepted(
            &fx.project,
            &task,
            &crate::task::EvidenceSnapshot::load(&fx.project)
        )
        .is_err()
    );
    fx.seal_done(
        &critic,
        1,
        2,
        "base",
        &format!("+++\nverdict = \"PASS\"\n{reason}\n+++\nEvery source inspected"),
    );
    crate::task::require_accepted(
        &fx.project,
        &task,
        &crate::task::EvidenceSnapshot::load(&fx.project),
    )
    .unwrap();
}

#[test]
fn required_criterion_rows_missing_partial_or_empty_cannot_merge_despite_passing_gates() {
    let fx = configured();
    let (id, _) = lane(&fx, 1);
    let task = job(&fx, &id);
    let mut review = prepared(&fx);
    review.gates = vec![project::Gate {
        command: "true".into(),
        ..Default::default()
    }];
    fx.world.runner.on_fn(
        |cmd| cmd.program == "sh",
        |cmd| crate::runner::Runner::run(&crate::runner::RealRunner, cmd),
    );
    let candidate = candidate(&fx, &review);
    let partial = criterion_reason(&id, &review.members[0].event, &task.acceptance[0], false);
    let empty = criterion_reason(&id, &review.members[0].event, &task.acceptance[0], true).replace(
        "immutable report artifact and inspected source coverage",
        "",
    );
    for (n, acceptance) in [(1, String::new()), (2, partial), (3, empty)] {
        let report = format!(
            "+++\nreview = {:?}\nverdict = \"MERGE\"\ncandidate = {candidate:?}\n{acceptance}\n+++\nAll gates passed!\n",
            review.id
        );
        fx.seal_done(
            review.reviewer.as_deref().unwrap(),
            1,
            n,
            &candidate,
            &report,
        );
        assert!(
            advance(&fx.world.ctx(), &fx.project, &mut review)
                .unwrap_err()
                .to_string()
                .contains("acceptance not established")
        );
        assert!(!review.fast_forward);
    }
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        4,
    );
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    crate::task::require_accepted(
        &fx.project,
        &task,
        &crate::task::EvidenceSnapshot::load(&fx.project),
    )
    .unwrap();
}

fn receipts(project: &Project, review: &Review) -> Vec<GateReceipt> {
    let mut records = Vec::new();
    for entry in std::fs::read_dir(dir(project).join(&review.id)).unwrap() {
        let path = entry.unwrap().path().join("receipt.toml");
        if path.is_file() {
            records.push(toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap());
        }
    }
    records
}

#[test]
fn landing_retry_cannot_use_declared_exits_or_missing_logs_as_execution_proof() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    review.gates = vec![project::Gate {
        command: "printf proof".into(),
        ..Default::default()
    }];
    let candidate = candidate(&fx, &review);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![GateRun {
            command: "printf proof".into(),
            exit: 0,
        }],
        1,
    );
    let events = crate::events::checked(&fx.project).unwrap();
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    let event = sealed(&events, &reviewer).unwrap();
    // Simulate an old cached landing verdict whose only proof was exit=0.
    review.verdict_event = event.id.clone();
    landing_verdict(&mut review, &candidate);
    assert!(
        land_with_install(&fx.world.ctx(), &fx.project, &mut review, || Ok(
            String::new()
        ))
        .unwrap_err()
        .to_string()
        .contains("not established")
    );
    fx.world.runner.on_fn(
        |cmd| cmd.program == "sh",
        |cmd| crate::runner::Runner::run(&crate::runner::RealRunner, cmd),
    );
    let git = Git::new(&fx.world.runner, &review.repo);
    review.verdict = Some(verdict(&fx.world.ctx(), &fx.project, &review, event, &git).unwrap());
    let receipt = receipts(&fx.project, &review).pop().unwrap();
    std::fs::remove_file(fx.project.state_dir().join(receipt.stdout)).unwrap();
    assert!(
        land_with_install(&fx.world.ctx(), &fx.project, &mut review, || Ok(
            String::new()
        ))
        .unwrap_err()
        .to_string()
        .contains("not established")
    );
    assert!(!review.fast_forward);
    // Same candidate but a new seal needs fresh observed proof.
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        2,
    );
    let fresh_events = crate::events::checked(&fx.project).unwrap();
    review.verdict_event = sealed(&fresh_events, &reviewer).unwrap().id.clone();
    assert!(verify_gate_receipts(&fx.world.ctx(), &fx.project, &review, &candidate, &git).is_err());
    save(&fx.project, &review).unwrap();
    let resumed = retry(&fx.world.ctx(), "demo", None).unwrap().unwrap();
    assert!(resumed.fast_forward);
    assert_eq!(resumed.phase, Phase::Complete);
    assert!(
        receipts(&fx.project, &resumed)
            .iter()
            .any(|receipt| receipt.event == resumed.verdict_event && receipt.exit == Some(0))
    );
}

#[test]
fn incomplete_capture_with_exit_zero_is_not_a_passing_receipt() {
    struct Incomplete;
    impl crate::runner::Runner for Incomplete {
        fn run(&self, cmd: &crate::runner::Cmd) -> Result<crate::runner::Output> {
            crate::runner::Runner::run(&crate::runner::RealRunner, cmd)
        }
        fn capture(
            &self,
            _: &crate::runner::Cmd,
            _: Option<&crate::runner::OutputLogs>,
        ) -> Result<crate::runner::Capture> {
            Ok(crate::runner::Capture {
                output: crate::runner::fake::ok("a valid-looking prefix"),
                stdout: crate::runner::StreamEvidence {
                    complete: false,
                    error: Some("pipe/log failed".into()),
                    ..Default::default()
                },
                stderr: crate::runner::StreamEvidence {
                    complete: true,
                    ..Default::default()
                },
            })
        }
    }
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    review.gates = vec![project::Gate {
        command: "checker".into(),
        ..Default::default()
    }];
    let candidate = candidate(&fx, &review);
    let mut ctx = fx.world.ctx();
    ctx.runner = &Incomplete;
    let git = Git::new(ctx.runner, &review.repo);
    assert!(
        observed_gates(
            &ctx,
            &fx.project,
            &review,
            &candidate,
            &git,
            &mut String::new()
        )
        .unwrap_err()
        .to_string()
        .contains("not established")
    );
    let receipt = receipts(&fx.project, &review).pop().unwrap();
    assert_eq!(receipt.exit, Some(0));
    assert!(!receipt.complete && receipt.error.contains("pipe/log failed"));
}

#[test]
fn passing_receipt_records_environment_candidate_and_full_logs_and_invalidates_on_change() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let gate = project::Gate {
        command: "test \"$RECEIPT_ENV\" = observed && test -f src/lane1.rs && printf 'behavior proved\\n'".into(),
        env: BTreeMap::from([("RECEIPT_ENV".into(), "observed".into())]),
        ..Default::default()
    };
    review.gates = vec![gate.clone()];
    let candidate = candidate(&fx, &review);
    let mut ctx = fx.world.ctx();
    ctx.runner = &crate::runner::RealRunner;
    let git = Git::new(ctx.runner, &review.repo);
    let runs = observed_gates(
        &ctx,
        &fx.project,
        &review,
        &candidate,
        &git,
        &mut String::new(),
    )
    .unwrap();
    assert_eq!(
        runs,
        vec![GateRun {
            command: gate.command.clone(),
            exit: 0
        }]
    );
    let receipt = receipts(&fx.project, &review).pop().unwrap();
    assert_eq!(receipt.environment.get("RECEIPT_ENV").unwrap(), "observed");
    assert_eq!(receipt.machine, "local");
    assert_eq!(receipt.candidate, candidate);
    assert_eq!(receipt.exit, Some(0));
    assert!(receipt.complete && receipt.error.is_empty());
    assert_eq!(
        std::fs::read_to_string(fx.project.state_dir().join(&receipt.stdout)).unwrap(),
        "behavior proved\n"
    );
    let cmd = gate_command(&gate, &candidate, &receipt.cwd, &receipt.environment, None);
    assert!(receipt.matches(&fx.project, &review, &candidate, &cmd, "local"));
    let changed = commit_file(
        Path::new(&receipt.cwd),
        "docs/next.md",
        "changed",
        "new candidate",
    );
    assert!(!receipt.matches(&fx.project, &review, &changed, &cmd, "local"));
    assert!(
        observed_gates(
            &ctx,
            &fx.project,
            &review,
            &candidate,
            &git,
            &mut String::new()
        )
        .unwrap_err()
        .to_string()
        .contains("not established")
    );
    observed_gates(
        &ctx,
        &fx.project,
        &review,
        &changed,
        &git,
        &mut String::new(),
    )
    .unwrap();
    assert!(
        receipts(&fx.project, &review)
            .iter()
            .any(|r| r.candidate == changed && r.exit == Some(0))
    );
    std::fs::write(fx.project.state_dir().join(&receipt.stdout), "fabricated").unwrap();
    assert!(!receipt.matches(&fx.project, &review, &candidate, &cmd, "local"));
}

#[test]
fn gate_free_and_allowlist_exclusions_are_visible_not_silently_broadened() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    let candidate = candidate(&fx, &review);
    let git = Git::new(&fx.world.runner, &review.repo);
    review.gates = vec![project::Gate {
        command: "never run".into(),
        paths: Some(vec!["old/**".into()]),
        ..Default::default()
    }];
    assert!(
        observed_gates(
            &fx.world.ctx(),
            &fx.project,
            &review,
            &candidate,
            &git,
            &mut String::new()
        )
        .unwrap()
        .is_empty()
    );
    let selection_path = dir(&fx.project)
        .join(&review.id)
        .join(format!("{candidate}-selection.toml"));
    let selection: toml::Value =
        toml::from_str(&std::fs::read_to_string(&selection_path).unwrap()).unwrap();
    assert!(selection["selected"].as_array().unwrap().is_empty());
    assert_eq!(
        selection["not_selected"][0]["paths"][0].as_str(),
        Some("old/**")
    );
    assert_eq!(selection["changed_paths"][0].as_str(), Some("src/lane1.rs"));
    review.gates.clear();
    review.gates_note = "no gates declared".into();
    observed_gates(
        &fx.world.ctx(),
        &fx.project,
        &review,
        &candidate,
        &git,
        &mut String::new(),
    )
    .unwrap();
    assert!(
        std::fs::read_to_string(selection_path)
            .unwrap()
            .contains("no gates declared")
    );
    assert_eq!(fx.world.runner.count("sh -c"), 0);
}

#[test]
fn complete_nonzero_gate_is_failed_and_notice_keeps_failing_lines_and_disagreement() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    review.gates = vec![project::Gate {
        command: "printf 'failures:\n    review::tests::holds::lock_and_start_errors_are_durable_holds\ntest result: FAILED\n'; exit 101".into(),
        ..Default::default()
    }];
    let candidate = candidate(&fx, &review);
    landing_verdict(&mut review, &candidate);
    let mut ctx = fx.world.ctx();
    ctx.runner = &crate::runner::RealRunner;
    let git = Git::new(ctx.runner, &review.repo);
    let error = observed_gates(
        &ctx,
        &fx.project,
        &review,
        &candidate,
        &git,
        &mut String::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.starts_with("gate failed:"), "{error}");
    assert!(error.contains("exit 101"));
    assert!(error.contains("reviewer verdict MERGE disagrees"));
    assert!(error.contains("stdout last lines: failures:"));
    assert!(!error.contains("not established"));
    needs_coordinator(&fx.project, &mut review, &error).unwrap();
    let notice = &review.notices.last().unwrap().line;
    assert!(notice.contains("gate failed:"));
    assert!(notice.contains("lock_and_start_errors_are_durable_holds"));
    assert!(notice.contains("reviewer verdict MERGE disagrees"));
    let receipt = receipts(&fx.project, &review).pop().unwrap();
    assert_eq!(receipt.exit, Some(101));
    assert!(receipt.complete && receipt.error.is_empty());
    assert!(!review.fast_forward);
    // A gate's own 125 is failure too; only ADE's checker marker is unknown.
    review.gates[0].command = "exit 125".into();
    let error = observed_gates(
        &ctx,
        &fx.project,
        &review,
        &candidate,
        &git,
        &mut String::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.starts_with("gate failed:"), "{error}");
    assert!(error.contains("exit 125"));
}

#[test]
fn gate_transport_error_is_not_established() {
    struct Transport;
    impl crate::runner::Runner for Transport {
        fn run(&self, cmd: &crate::runner::Cmd) -> Result<crate::runner::Output> {
            crate::runner::Runner::run(&crate::runner::RealRunner, cmd)
        }
        fn capture(
            &self,
            _: &crate::runner::Cmd,
            _: Option<&crate::runner::OutputLogs>,
        ) -> Result<crate::runner::Capture> {
            bail!("SSH transport: connection reset by peer")
        }
    }
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    review.gates = vec![project::Gate {
        command: "checker".into(),
        ..Default::default()
    }];
    let candidate = candidate(&fx, &review);
    let mut ctx = fx.world.ctx();
    ctx.runner = &Transport;
    let git = Git::new(ctx.runner, &review.repo);
    let error = observed_gates(
        &ctx,
        &fx.project,
        &review,
        &candidate,
        &git,
        &mut String::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("not established"), "{error}");
    assert!(error.contains("connection reset by peer"));
    assert!(!error.contains("gate failed:"));
}

#[test]
fn checker_timeout_and_spawn_errors_are_unknown_not_pass_or_work_failure() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    review.gates = vec![project::Gate {
        command: "checker".into(),
        ..Default::default()
    }];
    let candidate = candidate(&fx, &review);
    let git = Git::new(&fx.world.runner, &review.repo);
    let failure = observed_gates(
        &fx.world.ctx(),
        &fx.project,
        &review,
        &candidate,
        &git,
        &mut String::new(),
    )
    .unwrap_err();
    assert!(failure.to_string().contains("not established"));
    assert!(
        receipts(&fx.project, &review)[0]
            .error
            .contains("FakeRunner")
    );
    fx.world.runner.on_fn(
        |cmd| cmd.program == "sh",
        |_| Ok(crate::runner::fake::timeout()),
    );
    assert!(
        observed_gates(
            &fx.world.ctx(),
            &fx.project,
            &review,
            &candidate,
            &git,
            &mut String::new()
        )
        .is_err()
    );
    assert!(
        receipts(&fx.project, &review)
            .iter()
            .any(|r| r.timed_out && !r.complete && r.exit.is_none())
    );
    assert!(!review.fast_forward);
}

#[test]
fn receipt_capture_keeps_full_large_logs_instead_of_treating_clipping_as_pass() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    review.gates = vec![project::Gate {
        command: "head -c 2097152 /dev/zero".into(),
        ..Default::default()
    }];
    let candidate = candidate(&fx, &review);
    let mut ctx = fx.world.ctx();
    ctx.runner = &crate::runner::RealRunner;
    let git = Git::new(ctx.runner, &review.repo);
    observed_gates(
        &ctx,
        &fx.project,
        &review,
        &candidate,
        &git,
        &mut String::new(),
    )
    .unwrap();
    let receipt = receipts(&fx.project, &review).pop().unwrap();
    assert!(receipt.complete);
    assert_eq!(
        std::fs::metadata(fx.project.state_dir().join(receipt.stdout))
            .unwrap()
            .len(),
        2097152
    );
}

#[test]
fn remote_gate_command_uses_saved_target_checkout_and_environment() {
    let gate = project::Gate {
        command: "test \"$VALUE\" = 'a b'".into(),
        ..Default::default()
    };
    let environment = BTreeMap::from([
        ("PATH".into(), "/box/bin".into()),
        ("VALUE".into(), "a b".into()),
    ]);
    let command = gate_command(
        &gate,
        "sealed-sha",
        "/box/work tree",
        &environment,
        Some("saved-box"),
    );
    assert_eq!(command.program, "ssh");
    assert!(command.own_group);
    assert!(command.args.contains(&"saved-box".into()));
    assert!(command.env.is_empty());
    // Execute the generated SSH payload locally, using a small fake git, to
    // prove quoting and remote cwd/env behavior rather than matching prose.
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let fake_git = bin.join("git");
    std::fs::write(
        &fake_git,
        "#!/bin/sh\ncase \"$1\" in rev-parse) echo sealed-sha;; status) exit 0;; esac\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o755)).unwrap();
    let environment = BTreeMap::from([
        ("PATH".into(), format!("{}:/bin:/usr/bin", bin.display())),
        ("VALUE".into(), "a b".into()),
    ]);
    let command = gate_command(
        &gate,
        "sealed-sha",
        temp.path().to_str().unwrap(),
        &environment,
        Some("saved-box"),
    );
    let output = std::process::Command::new("sh")
        .args(["-c", command.args.last().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn historical_merged_acceptance_does_not_require_a_retained_done_seal() {
    let fx = configured();
    let (id, sha) = lane_unsealed(&fx, 1);
    let task = job(&fx, &id);
    thread::update(&fx.project, &id, |lane| lane.merged_sha = sha).unwrap();
    let snapshot = crate::task::EvidenceSnapshot::load(&fx.project);
    assert!(sealed(snapshot.events(), &thread::load(&fx.project, &id).unwrap()).is_none());
    crate::task::require_accepted(&fx.project, &task, &snapshot).unwrap();
    thread::update(&fx.project, &id, |lane| {
        lane.merged_review = "review-1".into()
    })
    .unwrap();
    // A read keeps its original lane/review evidence; the next read sees the change.
    crate::task::require_accepted(&fx.project, &task, &snapshot).unwrap();
    let fresh = crate::task::EvidenceSnapshot::load(&fx.project);
    assert!(crate::task::require_accepted(&fx.project, &task, &fresh).is_err());
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
