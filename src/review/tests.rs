use super::*;
use crate::testkit::{Fx, commit_file, fixture, git};

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
fn lane(fx: &Fx, n: u32) -> (String, String) {
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
fn missing_repo_gates_means_gate_free_review() {
    let fx = configured();
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    settings.repos[0].gates = None;
    std::fs::write(
        fx.project.project_md(),
        format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
    )
    .unwrap();
    lane(&fx, 1);
    let review = prepared(&fx);
    assert!(review.gates.is_empty());
    assert!(review.selected_gates.is_empty());
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
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Complete);
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
fn unopted_project_never_starts_review_and_old_files_are_not_read() {
    let fx = configured();
    lane(&fx, 1);
    for name in ["rounds", "checkpoints", "holds"] {
        std::fs::create_dir_all(fx.project.state_dir().join(name)).unwrap();
        std::fs::write(
            fx.project.state_dir().join(name).join("old.toml"),
            "not valid TOML [",
        )
        .unwrap();
    }
    let before = fx.world.runner.calls.borrow().len();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert!(list(&fx.project).unwrap().is_empty());
    assert_eq!(fx.world.runner.calls.borrow().len(), before);
    let review = prepared(&fx);
    let before = fx.world.runner.calls.borrow().len();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert_eq!(
        fx.world.runner.calls.borrow().len(),
        before,
        "waiting review must not probe git"
    );
    assert_eq!(list(&fx.project).unwrap().len(), 1);
    assert_eq!(
        start(&fx.world.ctx(), "demo", None).unwrap().unwrap().id,
        review.id
    );
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
    });
    review.phase = Phase::Landing;
    review.install_required = true;
    save(&fx.project, &review).unwrap();
    // Crash after git accepted the FF but before fast_forward was recorded.
    git(&fx.repo, &["merge", "--ff-only", &candidate]);
    assert!(cancel_record(&fx.world.ctx(), &fx.project, &mut review, "too late").is_err());
    assert!(review.fast_forward);
    assert!(
        land_with_install(&fx.world.ctx(), &fx.project, &mut review, || bail!(
            "install interrupted"
        ))
        .is_err()
    );
    assert!(review.fast_forward && review.push && !review.install && !review.close);
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
    let mut restored = load(&fx.project, &review.id).unwrap();
    land_with_install(&fx.world.ctx(), &fx.project, &mut restored, || Ok(())).unwrap();
    assert_eq!(restored.phase, Phase::Complete);
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
            .contains("path-selected gate")
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
