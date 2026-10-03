use super::*;

fn enable(fx: &Fx) {
    project::write_atomic(
        &fx.project.state_dir().join("reviews-enabled"),
        b"enabled\n",
    )
    .unwrap();
}
fn allocated_reviewer(fx: &Fx, review: &str) {
    let id = fx.thread("pile reviewer");
    thread::update(&fx.project, &id, |t| {
        t.role = "reviewer".into();
        t.review_id = review.into();
        t.pane_id.clear();
    })
    .unwrap();
}
fn coordinator(fx: &Fx) {
    let c = fx.project.coordinator().unwrap();
    *fx.world.agents.borrow_mut() = format!(
        "[{}]",
        crate::scenarios::agent_json(
            &c.workspace_id,
            &c.tab_id,
            &c.pane_id,
            &c.cwd,
            &c.agent_name,
            "working",
        )
    );
    fx.world
        .runner
        .on("agent prompt", crate::runner::fake::ok(r#"{"result":{}}"#));
}

#[test]
fn waiting_lane_releases_pile_and_later_done_joins_next_pile() {
    let fx = configured();
    enable(&fx);
    let (done, _) = lane(&fx, 1);
    let (waiting, sha) = lane_unsealed(&fx, 2);
    fx.seal_waiting(&waiting, 1, 1, "Rolf's sign-off");
    allocated_reviewer(&fx, "review-1");
    tick(&fx.world.ctx(), &fx.project).unwrap();
    let mut first = list(&fx.project).unwrap().remove(0);
    assert_eq!(first.members.len(), 1);
    assert_eq!(first.members[0].thread, done);
    assert!(hold_notices(&fx.project).unwrap().is_empty());

    // Synthetic completed review: its finished member must not rejoin.
    first.phase = Phase::Complete;
    save(&fx.project, &first).unwrap();
    thread::update(&fx.project, &done, |t| t.status = Status::Resolved).unwrap();
    fx.seal_done(&waiting, 1, 2, &sha, "sign-off received\n");
    allocated_reviewer(&fx, "review-2");
    tick(&fx.world.ctx(), &fx.project).unwrap();
    let reviews = list(&fx.project).unwrap();
    assert_eq!(reviews.len(), 2);
    assert_eq!(reviews[1].members.len(), 1);
    assert_eq!(reviews[1].members[0].thread, waiting);
}

#[test]
fn working_lane_holds_once_through_outbox_and_clears_after_waiting() {
    let fx = configured();
    enable(&fx);
    let (done, _) = lane(&fx, 1);
    let (working, _) = lane_unsealed(&fx, 2);
    // The fixture seals use fixed historical times: this lane was already running.
    thread::update(&fx.project, &working, |t| {
        t.created = "2026-09-18T10:00:00Z".into()
    })
    .unwrap();
    // Repository aliases/worktrees must still produce only one hold notice.
    thread::update(&fx.project, &done, |t| t.repo = t.worktree_path.clone()).unwrap();
    coordinator(&fx);
    for _ in 0..3 {
        tick(&fx.world.ctx(), &fx.project).unwrap();
        crate::steps::deliver_transition_notices(&fx.world.ctx(), &fx.project).unwrap();
    }
    assert!(list(&fx.project).unwrap().is_empty());
    let notices = hold_notices(&fx.project).unwrap();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].submitted);
    assert!(notices[0].line.contains(&format!("{done} ready")));
    assert!(notices[0].line.contains(&format!("{working} (working)")));
    assert_eq!(
        fx.world
            .runner
            .calls
            .borrow()
            .iter()
            .filter(|cmd| cmd.display().contains("agent prompt") && cmd.display().contains("PILE"))
            .count(),
        1
    );

    fx.seal_waiting(&working, 1, 1, "waiting on coordinator");
    allocated_reviewer(&fx, "review-1");
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert!(load_holds(&fx.project).unwrap().current.is_empty());
}

#[test]
fn waiting_lane_with_unanswered_follow_up_still_holds() {
    for state in [
        thread::FollowUpState::Queued,
        thread::FollowUpState::Delivered,
    ] {
        let fx = configured();
        enable(&fx);
        lane(&fx, 1);
        let (waiting, _) = lane_unsealed(&fx, 2);
        let event = fx.seal_waiting(&waiting, 1, 1, "need input");
        thread::update(&fx.project, &waiting, |t| {
            t.follow_ups.push(thread::FollowUp {
                attempt: 1,
                state,
                waiting_event: event,
                ..Default::default()
            })
        })
        .unwrap();
        for _ in 0..3 {
            tick(&fx.world.ctx(), &fx.project).unwrap();
        }
        assert!(list(&fx.project).unwrap().is_empty());
        let notices = hold_notices(&fx.project).unwrap();
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0]
                .line
                .contains(&format!("{waiting} (follow-up pending)"))
        );
    }
}

#[test]
fn lock_and_start_errors_are_durable_holds() {
    let fx = configured();
    enable(&fx);
    lane(&fx, 1);
    let lock = operation_lock(&fx.world.ctx(), fx.repo.to_str().unwrap()).unwrap();
    for _ in 0..3 {
        tick(&fx.world.ctx(), &fx.project).unwrap();
    }
    assert_eq!(hold_notices(&fx.project).unwrap().len(), 1);
    assert!(
        hold_notices(&fx.project).unwrap()[0]
            .line
            .contains("operation lock")
    );
    drop(lock);
    // A configured integration branch that does not exist fails before review allocation.
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    settings.repos[0].branch = Some("missing".into());
    std::fs::write(
        fx.project.project_md(),
        format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
    )
    .unwrap();
    for _ in 0..3 {
        assert!(tick(&fx.world.ctx(), &fx.project).is_err());
    }
    let notices = hold_notices(&fx.project).unwrap();
    assert_eq!(notices.len(), 2);
    assert!(notices[1].line.contains("integration branch is missing"));
}

#[test]
fn disabled_and_unconfigured_reviews_do_not_hold_silently() {
    let fx = configured();
    lane(&fx, 1);
    for _ in 0..3 {
        tick(&fx.world.ctx(), &fx.project).unwrap();
    }
    assert!(list(&fx.project).unwrap().is_empty());
    let notices = hold_notices(&fx.project).unwrap();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].line.contains("automatic review not enabled"));

    enable(&fx);
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    settings.repos.clear();
    std::fs::write(
        fx.project.project_md(),
        format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
    )
    .unwrap();
    for _ in 0..3 {
        tick(&fx.world.ctx(), &fx.project).unwrap();
    }
    let notices = hold_notices(&fx.project).unwrap();
    assert_eq!(notices.len(), 2);
    assert!(notices[1].line.contains("repository not configured"));
}

#[test]
fn pending_hold_notice_survives_a_cleared_hold() {
    let fx = configured();
    enable(&fx);
    lane(&fx, 1);
    let (working, _) = lane_unsealed(&fx, 2);
    thread::update(&fx.project, &working, |t| {
        t.created = "2026-09-18T10:00:00Z".into()
    })
    .unwrap();
    // No coordinator agent is available to accept the notice yet.
    tick(&fx.world.ctx(), &fx.project).unwrap();
    crate::steps::deliver_transition_notices(&fx.world.ctx(), &fx.project).unwrap();
    assert!(!hold_notices(&fx.project).unwrap()[0].submitted);
    fx.seal_waiting(&working, 1, 1, "need input");
    allocated_reviewer(&fx, "review-1");
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert!(load_holds(&fx.project).unwrap().current.is_empty());
    coordinator(&fx);
    for _ in 0..3 {
        crate::steps::deliver_transition_notices(&fx.world.ctx(), &fx.project).unwrap();
    }
    let notices = hold_notices(&fx.project).unwrap();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].submitted);
    assert_eq!(
        fx.world
            .runner
            .calls
            .borrow()
            .iter()
            .filter(|cmd| cmd.display().contains("agent prompt") && cmd.display().contains("PILE"))
            .count(),
        1
    );
}

fn seal_at(fx: &Fx, id: &str, sha: &str, at: &str) {
    let event = fx.seal_done(id, 1, 1, sha, "finished\n");
    let path = fx
        .project
        .state_dir()
        .join("events")
        .join(format!("{event}.toml"));
    let mut record: crate::contracts::Event =
        toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    record.created = at.into();
    std::fs::write(path, toml::to_string(&record).unwrap()).unwrap();
}

#[test]
fn later_attempt_never_extends_an_older_ready_pile() {
    let fx = configured();
    enable(&fx);
    let (early, early_sha) = lane_unsealed(&fx, 1); // live t-0765
    thread::update(&fx.project, &early, |t| {
        t.created = "2026-10-03T18:00:00Z".into()
    })
    .unwrap();
    let (ready, ready_sha) = lane_unsealed(&fx, 2); // live t-0768
    seal_at(&fx, &ready, &ready_sha, "2026-10-03T18:44:00Z");
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert!(list(&fx.project).unwrap().is_empty());
    let initial = hold_notices(&fx.project).unwrap();
    assert_eq!(initial.len(), 1);
    assert!(initial[0].line.contains(&format!("{ready} ready")));
    assert!(initial[0].line.contains(&format!("{early} (working)")));

    let (later, later_sha) = lane_unsealed(&fx, 3); // live t-0769
    thread::update(&fx.project, &later, |t| {
        t.created = "2026-10-03T18:58:00Z".into()
    })
    .unwrap();
    for _ in 0..3 {
        tick(&fx.world.ctx(), &fx.project).unwrap();
    }
    assert!(list(&fx.project).unwrap().is_empty());
    assert_eq!(hold_notices(&fx.project).unwrap()[0].line, initial[0].line);
    assert_eq!(hold_notices(&fx.project).unwrap().len(), 1);

    // The oldest seal still supplies the cutoff, not this newer one.
    seal_at(&fx, &early, &early_sha, "2026-10-03T19:00:00Z");
    allocated_reviewer(&fx, "review-1");
    tick(&fx.world.ctx(), &fx.project).unwrap();
    let mut first = list(&fx.project).unwrap().remove(0);
    let members: std::collections::BTreeSet<_> =
        first.members.iter().map(|m| m.thread.clone()).collect();
    assert_eq!(
        members,
        [early.clone(), ready.clone()].into_iter().collect()
    );
    assert!(load_holds(&fx.project).unwrap().current.is_empty());
    assert_eq!(hold_notices(&fx.project).unwrap().len(), 1);

    first.phase = Phase::Complete;
    save(&fx.project, &first).unwrap();
    for id in [&early, &ready] {
        thread::update(&fx.project, id, |t| t.status = Status::Resolved).unwrap();
    }
    fx.seal_done(&later, 1, 1, &later_sha, "finished later\n");
    allocated_reviewer(&fx, "review-2");
    tick(&fx.world.ctx(), &fx.project).unwrap();
    let reviews = list(&fx.project).unwrap();
    assert_eq!(reviews.len(), 2);
    assert_eq!(reviews[1].members.len(), 1);
    assert_eq!(reviews[1].members[0].thread, later);
    assert_eq!(reviews[1].members[0].attempt, 1);
}

#[test]
fn working_hold_uses_attempt_times_and_keeps_unknown_history() {
    let seal = "2026-10-03T18:44:00Z".parse().unwrap();
    for (attempt, created, submitted, startup, holds) in [
        (1, "2026-10-03T18:00:00Z", "", "", true),
        (1, "2026-10-03T18:58:00Z", "", "", false),
        (1, "2026-10-03T18:44:00Z", "", "", false),
        (2, "2026-10-03T17:00:00Z", "2026-10-03T18:58:00Z", "", false),
        (2, "2026-10-03T17:00:00Z", "", "2026-10-03T18:58:00Z", false),
        (2, "2026-10-03T17:00:00Z", "2026-10-03T18:00:00Z", "", true),
        // Staging a brief later must not erase an already-running startup.
        (2, "", "2026-10-03T18:58:00Z", "2026-10-03T18:00:00Z", true),
        (1, "2026-10-03T18:00:00Z", "2026-10-03T18:58:00Z", "", true),
        (2, "2026-10-03T18:58:00Z", "", "", true),
        (1, "", "", "", true),
        (1, "invalid", "invalid", "invalid", true),
    ] {
        for status in [Status::Starting, Status::Open] {
            let lane = Thread {
                status,
                attempt,
                created: created.into(),
                brief_submitted_at: submitted.into(),
                startup_wait_started: startup.into(),
                ..Default::default()
            };
            assert_eq!(
                working_hold(&lane, &[], Some(seal)),
                holds.then_some("working")
            );
            assert_eq!(working_hold(&lane, &[], None), Some("working"));
        }
    }
}

#[test]
fn active_review_holds_only_new_members_and_notifies_once() {
    let fx = configured();
    enable(&fx);
    lane(&fx, 1);
    allocated_reviewer(&fx, "review-1");
    tick(&fx.world.ctx(), &fx.project).unwrap();
    tick(&fx.world.ctx(), &fx.project).unwrap();
    assert!(hold_notices(&fx.project).unwrap().is_empty());
    let (later, _) = lane(&fx, 2);
    for _ in 0..3 {
        tick(&fx.world.ctx(), &fx.project).unwrap();
    }
    assert_eq!(list(&fx.project).unwrap().len(), 1);
    let notices = hold_notices(&fx.project).unwrap();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].line.contains(&format!("{later} ready")));
    assert!(notices[0].line.contains("active review review-1"));
}
