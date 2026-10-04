use super::*;
use crate::runner::fake::ok;
use crate::testkit::{Fx, fixture, git};
use crate::thread::{self, Status};

fn task_for(fx: &Fx, attempts: Vec<String>, dropped: bool) {
    write(
        &fx.project,
        &Task {
            id: "job-0001".into(),
            title: "Rejected work".into(),
            authority: vec!["request:q-1".into()],
            attempts,
            acceptance: vec!["A checked result".into()],
            dropped: if dropped {
                vec![DropEvidence {
                    at: project::now(),
                    reason: "not needed".into(),
                }]
            } else {
                vec![]
            },
            ..Default::default()
        },
    )
    .unwrap();
}

fn reject(fx: &Fx, lane: &thread::Thread, sha: &str, event: &str) {
    let review = crate::review::Review {
        id: "review-1".into(),
        repo: lane.repo.clone(),
        integration: "main".into(),
        base: git(&fx.repo, &["rev-parse", "main"]),
        candidate_branch: "candidate".into(),
        members: vec![crate::review::Member {
            thread: lane.id.clone(),
            attempt: 1,
            event: event.into(),
            sha: sha.into(),
            branch: lane.branch.clone(),
            artifact: String::new(),
        }],
        gates: vec![],
        gates_note: String::new(),
        selected_gates: vec![],
        reviewer: None,
        review_machine: None,
        phase: crate::review::Phase::Rejected,
        verdict: None,
        verdict_event: String::new(),
        reviewer_after: String::new(),
        checked_event: String::new(),
        retry_attempt: None,
        retry_generation: 0,
        moved: 0,
        refresh_tip: None,
        push_remote: None,
        install_required: false,
        fast_forward: false,
        merged_at: String::new(),
        installed_at: String::new(),
        push: false,
        install: false,
        install_result: String::new(),
        close: false,
        prune: false,
        attention: String::new(),
        no_verdict_since: String::new(),
        notices: vec![],
    };
    let dir = fx.project.state_dir().join("reviews");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("review-1.toml"), toml::to_string(&review).unwrap()).unwrap();
}

#[test]
fn drop_retires_rejected_seals_keeps_reports_and_never_deletes_unique_work() {
    for unique in [false, true] {
        let fx = fixture();
        std::fs::write(
            fx.repo.join(".git/info/exclude"),
            ".worktrees/\n.herdr-project/\n",
        )
        .unwrap();
        let config = fx.world.home.path().join("cfg/config.toml");
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(
            config,
            format!("{text}\n[worktrees]\ndisposable = [\".herdr-project\"]\n"),
        )
        .unwrap();
        let (id, mut sha) = fx.lane(1);
        let lane = thread::load(&fx.project, &id).unwrap();
        if !unique {
            git(
                std::path::Path::new(&lane.worktree_path),
                &["reset", "--hard", "main"],
            );
            sha = git(&fx.repo, &["rev-parse", "main"]);
        }
        let lane = thread::update(&fx.project, &id, |t| {
            t.base = git(&fx.repo, &["rev-parse", "main"]);
            t.thread_dir = format!("{}/.herdr-project/demo-{}", t.worktree_path, id);
            t.parked = true;
        })
        .unwrap();
        std::fs::create_dir_all(std::path::Path::new(&lane.thread_dir).join("library")).unwrap();
        std::fs::write(
            std::path::Path::new(&lane.thread_dir).join("report.md"),
            "# Rejected result\n",
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&lane.thread_dir).join("library/notes.md"),
            "Useful findings\n",
        )
        .unwrap();
        fx.world.runner.on("du -sk", ok("1\tlibrary\n"));
        fx.world.runner.on_fn(
            |cmd| cmd.program == "rsync",
            |cmd| crate::runner::Runner::run(&crate::runner::RealRunner, cmd),
        );
        let event = fx.seal_done(&id, 1, 1, &sha, "# Rejected result\n");
        reject(&fx, &lane, &sha, &event);
        *fx.world.panes.borrow_mut() = serde_json::json!([{
            "pane_id": lane.pane_id, "tab_id": lane.tab_id,
            "workspace_id": lane.workspace_id, "cwd": lane.worktree_path
        }])
        .to_string();
        fx.world.runner.on(
            "pane process-info",
            ok(&serde_json::json!({"result":{"process_info":{
                "pane_id": lane.pane_id, "foreground_processes": [{"pid": 1, "name": "bash"}]
            }}})
            .to_string()),
        );
        task_for(&fx, vec![id.clone()], false);
        let outcome =
            drop_task(&fx.world.ctx(), &fx.project, "job-0001", "no longer needed").unwrap();
        assert_eq!(outcome.lanes.len(), 1);
        assert_eq!(outcome.lanes[0].state, "resolved", "{:?}", outcome.lanes);
        let retired = thread::load(&fx.project, &id).unwrap();
        assert_eq!(retired.status, Status::Resolved);
        assert!(!retired.cleanup_pending);
        assert_eq!(outcome.lanes[0].pane, "closed");
        assert_eq!(
            fx.world.runner.count(&format!("tab close {}", lane.tab_id)),
            1
        );
        assert_eq!(
            std::path::Path::new(&lane.worktree_path).exists(),
            unique,
            "{:?}",
            outcome.lanes
        );
        assert_eq!(
            outcome.lanes[0].worktree,
            if unique { "kept" } else { "removed" }
        );
        if unique {
            assert!(
                outcome.lanes[0]
                    .worktree_reason
                    .as_ref()
                    .unwrap()
                    .starts_with("work_not_done:")
            );
            assert_eq!(git(&fx.repo, &["rev-parse", &lane.branch]), sha);
        }
        assert_eq!(
            std::fs::read_to_string(thread::sealed_report_path(&fx.project, &retired).unwrap())
                .unwrap(),
            "# Rejected result\n"
        );
        let library = fx.project.dir().join("library").join(&id);
        assert_eq!(
            std::fs::read_to_string(library.join("notes.md")).unwrap(),
            "Useful findings\n"
        );
        assert_eq!(
            std::fs::read_to_string(library.join("report.md")).unwrap(),
            "# Rejected result\n"
        );
        assert!(!outcome.task.dropped.is_empty());
    }
}

#[test]
fn existing_dropped_seals_show_retire_command_and_working_lanes_show_cancel() {
    let fx = fixture();
    let (done, sha) = fx.lane(1);
    let (waiting, _) = fx.lane(2);
    let (working, _) = fx.lane(3);
    fx.seal_done(&done, 1, 1, &sha, "# Rejected result\n");
    reject(
        &fx,
        &thread::load(&fx.project, &done).unwrap(),
        &sha,
        &format!("{done}-1-1"),
    );
    fx.seal_waiting(&waiting, 1, 1, "Missing input");
    task_for(
        &fx,
        vec![done.clone(), waiting.clone(), working.clone()],
        true,
    );
    let settings = fx.project.read_project_md().unwrap().0;
    let view = crate::project_view::View::capture(&fx.project, &settings, None, None);
    for id in [&done, &waiting] {
        let row = view.lanes.iter().find(|r| &r.thread.id == id).unwrap();
        assert!(row.note.contains("task job-0001 dropped"));
        assert!(
            row.note
                .contains(&format!("retire with ha thread resolve demo {id}"))
        );
        assert!(!row.note.contains("awaiting review"));
        assert_eq!(thread::load(&fx.project, id).unwrap().status, Status::Open);
    }
    let row = view.lanes.iter().find(|r| r.thread.id == working).unwrap();
    assert!(row.note.contains(&format!(
        "cancel with ha thread cancel demo {working} --reason \"task job-0001 dropped\""
    )));
    assert_eq!(fx.world.runner.count("worktree remove"), 0);
}

#[test]
fn drop_retires_waiting_but_leaves_working_and_landed_lanes_alone() {
    let fx = fixture();
    let waiting = fx.thread("Waiting lane");
    let working = fx.thread("Working lane");
    let landed = fx.thread("Landed lane");
    let resumed = fx.thread("Answered wait");
    let follow_up = fx.thread("Working after done");
    let wait_event = fx.seal_waiting(&resumed, 1, 1, "Input arrived later");
    thread::update(&fx.project, &resumed, |t| {
        t.answered_waiting_event = wait_event
    })
    .unwrap();
    let done_event = fx.seal_done(&follow_up, 1, 1, "sha", "# Earlier result\n");
    thread::update(&fx.project, &follow_up, |t| {
        t.follow_ups.push(thread::FollowUp {
            attempt: 1,
            after_seal: done_event,
            state: thread::FollowUpState::Delivered,
            ..Default::default()
        })
    })
    .unwrap();
    fx.seal_waiting(&waiting, 1, 1, "Missing input");
    fx.seal_done(&landed, 1, 1, "landed-sha", "# Landed\n");
    thread::update(&fx.project, &landed, |t| t.merged_sha = "landed-sha".into()).unwrap();
    task_for(
        &fx,
        vec![
            waiting.clone(),
            working.clone(),
            landed.clone(),
            resumed.clone(),
            follow_up.clone(),
        ],
        false,
    );
    let outcome = drop_task(&fx.world.ctx(), &fx.project, "job-0001", "not needed").unwrap();
    assert_eq!(outcome.lanes.len(), 1);
    assert_eq!(outcome.lanes[0].thread, waiting);
    assert_eq!(
        thread::load(&fx.project, &waiting).unwrap().status,
        Status::Resolved
    );
    for id in [&working, &landed, &resumed, &follow_up] {
        assert_eq!(thread::load(&fx.project, id).unwrap().status, Status::Open);
    }
}
