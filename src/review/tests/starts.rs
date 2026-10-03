use super::*;

fn harness_gates(fx: &Fx, gates: Option<Vec<project::Gate>>, listed: bool) {
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    let mut row = settings.repos[0].clone();
    row.gates = gates;
    let config = fx.world.ctx().config_dir.join("config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push('\n');
    text.push_str(
        &toml::to_string(&BTreeMap::from([(
            "harness",
            BTreeMap::from([("repos", vec![row])]),
        )]))
        .unwrap(),
    );
    std::fs::write(config, text).unwrap();
    if !listed {
        settings.repos.clear();
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
    }
}

fn four_gates() -> Vec<project::Gate> {
    [
        "cargo fmt --check",
        "cargo test",
        "cargo clippy --all-targets -- -D warnings",
        "git diff --check",
    ]
    .into_iter()
    .map(|command| project::Gate {
        command: command.into(),
        paths: None,
        env: BTreeMap::new(),
    })
    .collect()
}

#[test]
fn unlisted_harness_repo_selects_all_four_harness_gates() {
    let fx = configured();
    harness_gates(&fx, Some(four_gates()), false);
    lane(&fx, 1);
    let review = prepared(&fx);
    assert!(review.install_required);
    assert_eq!(review.gates, four_gates());
    assert_eq!(review.selected_gates, four_gates());
    assert!(review.gates_note.is_empty());
    let brief = task(&fx.project, &review);
    for gate in four_gates() {
        assert!(brief.contains(&gate.command));
    }
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
    let events = crate::events::checked(&fx.project).unwrap();
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    let event = sealed(&events, &reviewer).unwrap();
    let git = Git::new(&fx.world.runner, fx.repo.to_str().unwrap());
    assert!(verdict(&fx.project, &review, event, &git).is_err());
    let runs = four_gates()
        .into_iter()
        .map(|gate| GateRun {
            command: gate.command,
            exit: 0,
        })
        .collect();
    seal_verdict(&fx, &review, &candidate, "MERGE", BTreeMap::new(), runs, 2);
    let events = crate::events::checked(&fx.project).unwrap();
    assert!(
        verdict(
            &fx.project,
            &review,
            sealed(&events, &reviewer).unwrap(),
            &git
        )
        .is_ok()
    );
}

#[test]
fn harness_gates_override_listing_project_gates() {
    let fx = configured();
    // The listing project explicitly has no gates; the harness policy wins.
    harness_gates(&fx, Some(four_gates()), true);
    lane(&fx, 1);
    let review = prepared(&fx);
    assert_eq!(review.gates, four_gates());
    assert_eq!(review.selected_gates, four_gates());
}

#[test]
fn harness_repo_without_any_gate_declaration_refuses_before_allocating() {
    for listed in [false, true] {
        let fx = configured();
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos[0].gates = None;
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        harness_gates(&fx, None, listed);
        lane(&fx, 1);
        let error = start(&fx.world.ctx(), "demo", Some(fx.repo.to_str().unwrap()))
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            format!(
                "harness_gates_missing: {} has no gates; add gates to its [harness] row in config.toml",
                fx.repo.display()
            )
        );
        assert!(list(&fx.project).unwrap().is_empty());
        assert!(reviewer_ids(&fx.project).unwrap().is_empty());
        assert!(git(&fx.repo, &["branch", "--list", "review/*"]).is_empty());
    }
}

#[test]
fn harness_repo_uses_listing_project_gates_when_harness_gates_are_omitted() {
    let fx = configured();
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    settings.repos[0].gates = Some(four_gates());
    std::fs::write(
        fx.project.project_md(),
        format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
    )
    .unwrap();
    harness_gates(&fx, None, true);
    lane(&fx, 1);
    let review = prepared(&fx);
    assert_eq!(review.gates, four_gates());
    assert_eq!(review.selected_gates, four_gates());
}

#[test]
fn repository_worktrees_share_one_review_lock() {
    let fx = configured();
    let (id, _) = lane(&fx, 1);
    let lane = thread::load(&fx.project, &id).unwrap();
    assert!(same_repo(fx.repo.to_str().unwrap(), &lane.worktree_path));
    let _held = operation_lock(&fx.world.ctx(), fx.repo.to_str().unwrap()).unwrap();
    let identity = repo_identity(&lane.worktree_path);
    let key = fx
        .world
        .ctx()
        .root
        .join(".review-locks")
        .join(thread::sha256_hex(identity.to_string_lossy().as_bytes()));
    let second = std::fs::File::options().write(true).open(key).unwrap();
    assert!(second.try_lock().is_err());
}

#[test]
fn plugin_manifest_has_no_review_event() {
    let manifest: toml::Value = toml::from_str(include_str!("../../../herdr-plugin.toml")).unwrap();
    assert!(manifest.get("events").is_none());
}

#[test]
fn ticker_skips_locked_pile_then_advances_after_release() {
    let fx = configured();
    lane(&fx, 1);
    project::write_atomic(
        &fx.project.state_dir().join("reviews-enabled"),
        b"enabled\n",
    )
    .unwrap();
    let held = operation_lock(&fx.world.ctx(), fx.repo.to_str().unwrap()).unwrap();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert!(list(&fx.project).unwrap().is_empty());
    drop(held);

    // Recover an already allocated reviewer rather than placing a new pane.
    let reviewer = fx.thread("pile reviewer");
    thread::update(&fx.project, &reviewer, |t| {
        t.role = "reviewer".into();
        t.review_id = "review-1".into();
    })
    .unwrap();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert_eq!(
        list(&fx.project).unwrap()[0].reviewer.as_deref(),
        Some(reviewer.as_str())
    );
}

#[test]
fn ticker_holds_pile_while_a_failed_lane_has_a_live_agent() {
    let fx = configured();
    lane(&fx, 1);
    let (failed, _) = lane_unsealed(&fx, 2);
    thread::update(&fx.project, &failed, |t| {
        t.status = Status::Failed;
        t.pane_id = "w1:p12".into();
        t.tab_id = "w1:t12".into();
        t.workspace_id = "w1".into();
        t.launch.kind = "claude".into();
        t.agent_name = "old-name".into();
        t.error = "agent_not_ready: still blocked at the end of its ready window".into();
    })
    .unwrap();
    let record = thread::load(&fx.project, &failed).unwrap();
    *fx.world.panes.borrow_mut() = serde_json::json!([{
        "pane_id": record.pane_id, "tab_id": record.tab_id,
        "workspace_id": record.workspace_id, "cwd": record.cwd
    }])
    .to_string();
    *fx.world.agents.borrow_mut() = serde_json::json!([{
        "pane_id": record.pane_id, "tab_id": record.tab_id,
        "workspace_id": record.workspace_id, "cwd": record.worktree_path,
        "name": "", "agent": "claude", "agent_status": "working"
    }])
    .to_string();
    project::write_atomic(
        &fx.project.state_dir().join("reviews-enabled"),
        b"enabled\n",
    )
    .unwrap();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert!(list(&fx.project).unwrap().is_empty());

    // A reused pane with an unrelated agent must not hold the pile forever.
    *fx.world.agents.borrow_mut() = serde_json::json!([{
        "pane_id": record.pane_id, "tab_id": record.tab_id,
        "workspace_id": record.workspace_id, "cwd": "/other/worktree",
        "name": "", "agent": "claude", "agent_status": "working"
    }])
    .to_string();
    let reviewer = fx.thread("pile reviewer");
    thread::update(&fx.project, &reviewer, |t| {
        t.role = "reviewer".into();
        t.review_id = "review-1".into();
    })
    .unwrap();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert_eq!(
        list(&fx.project).unwrap()[0].reviewer.as_deref(),
        Some(reviewer.as_str())
    );
}

#[test]
fn reviewer_placement_leaves_launch_pending_and_releases_the_lock() {
    let fx = configured();
    lane(&fx, 1);
    *fx.world.panes.borrow_mut() = format!("[{}]", fx.world.coordinator_pane(&fx.project));
    fx.world.runner.on("HERDR_ADE_LAUNCH", crate::runner::fake::ok(r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","cwd":"/wt"}}}"#));
    let review = start(&fx.world.ctx(), "demo", None).unwrap().unwrap();
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    assert!(
        reviewer.prompt_pending,
        "ticker must own the pending launch"
    );
    let held = try_operation_lock(&fx.world.ctx(), &review.repo).unwrap();
    assert!(held.is_some(), "review lock must be free before launch");
    drop(held);
    // A second ticker pass can enter while startup remains pending.
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert_eq!(list(&fx.project).unwrap()[0].reviewer, review.reviewer);
}

#[test]
fn historical_lock_holder() {
    let Ok(path) = std::env::var("HERDR_ADE_TEST_HISTORICAL_LOCK") else {
        return;
    };
    use std::io::{Read, Write};
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.lock().unwrap();
    println!("historical lock held");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
}

#[test]
fn historical_read_skips_a_locked_repository() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let fx = configured();
    let (id, _) = lane(&fx, 1);
    thread::update(&fx.project, &id, |t| t.status = Status::Resolved).unwrap();
    // Hold the lock in another process: advisory locks held by this test's
    // process cannot prove contention consistently across platforms.
    let lock = lock_file(&fx.world.ctx(), fx.repo.to_str().unwrap()).unwrap();
    let path = lock_path(&fx.world.ctx(), fx.repo.to_str().unwrap());
    drop(lock);
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "review::tests::starts::historical_lock_holder",
            "--nocapture",
        ])
        .env("HERDR_ADE_TEST_HISTORICAL_LOCK", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(holder.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "lock holder exited before acquiring lock"
        );
        if line.contains("historical lock held") {
            break;
        }
    }
    assert!(
        try_operation_lock(&fx.world.ctx(), fx.repo.to_str().unwrap())
            .unwrap()
            .is_none()
    );
    classify_old_seals(&fx.world.ctx(), &fx.project, true).unwrap();
    assert!(
        thread::load(&fx.project, &id)
            .unwrap()
            .historical_seal
            .is_empty()
    );
    drop(holder.stdin.take());
    assert!(holder.wait().unwrap().success());
    classify_old_seals(&fx.world.ctx(), &fx.project, true).unwrap();
    assert!(
        !thread::load(&fx.project, &id)
            .unwrap()
            .historical_seal
            .is_empty()
    );
}

#[test]
fn ticker_pass_finishes_with_box_reviewer_launch_pending() {
    let fx = configured();
    lane(&fx, 1);
    let review = prepared(&fx);
    let id = review.reviewer.as_deref().unwrap();
    thread::update(&fx.project, id, |t| {
        t.machine = "buildbox".into();
        t.machine_id = "buildbox-id".into();
        t.prompt_pending = true;
        t.status = Status::Open;
    })
    .unwrap();
    project::write_atomic(
        &fx.project.state_dir().join("reviews-enabled"),
        b"enabled\n",
    )
    .unwrap();
    // The remote agent has not become ready. Review advancement must leave its
    // pending launch to the remote startup pass, not start/wait inline.
    crate::ticker::tick_project(&fx.world.ctx(), &fx.project).unwrap();
    assert!(thread::load(&fx.project, id).unwrap().prompt_pending);
    assert!(
        try_operation_lock(&fx.world.ctx(), &review.repo)
            .unwrap()
            .is_some()
    );
}

#[test]
fn published_descendant_is_a_successful_push_postcondition() {
    let fx = configured();
    let candidate = git(&fx.repo, &["rev-parse", "main"]);
    commit_file(&fx.repo, "later.txt", "later", "later integration change");
    let remote = fx.world.home.path().join("published.git");
    git(
        &fx.repo,
        &["init", "--bare", "-q", remote.to_str().unwrap()],
    );
    git(&fx.repo, &["push", remote.to_str().unwrap(), "main"]);
    assert!(
        remote_contains(
            &Git::new(fx.world.ctx().runner, &fx.repo),
            remote.to_str().unwrap(),
            "refs/heads/main",
            &candidate
        )
        .unwrap()
    );
}

#[test]
fn conflicts_are_left_for_the_one_reviewer_without_a_dirty_integration_checkout() {
    let fx = configured();
    let (a, _) = lane(&fx, 1);
    let (b, _) = lane(&fx, 2);
    for (id, text) in [(&a, "first\n"), (&b, "second\n")] {
        let lane = thread::load(&fx.project, id).unwrap();
        let sha = commit_file(
            Path::new(&lane.worktree_path),
            "README.md",
            text,
            "conflicting edit",
        );
        fx.seal_done(id, 1, 2, &sha, "ready with conflict");
    }
    let review = prepared(&fx);
    assert_eq!(review.members.len(), 2);
    assert_eq!(review.phase, Phase::Reviewing);
    assert_eq!(git(&fx.repo, &["status", "--porcelain"]), "");
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]), review.base);
    let candidate = git(&fx.repo, &["rev-parse", &review.candidate_branch]);
    let git = Git::new(fx.world.ctx().runner, &fx.repo);
    assert!(git.is_ancestor(&review.members[0].sha, &candidate).unwrap());
    assert!(!git.is_ancestor(&review.members[1].sha, &candidate).unwrap());
}

#[test]
fn a_crash_during_cancel_cannot_accept_the_old_merge_verdict() {
    let fx = configured();
    lane(&fx, 1);
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
    review.phase = Phase::Cancelling;
    review.attention = "cancel requested before crash".into();
    save(&fx.project, &review).unwrap();
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Cancelled);
    assert!(!review.fast_forward);
    assert!(review.close && review.prune);
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]), review.base);
}
