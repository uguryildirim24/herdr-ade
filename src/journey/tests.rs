//! Boundary regressions: real Git preflight and real process shutdown driven by
//! the existing lifecycle APIs. Only Herdr/provider transport is scripted.
use super::*;
use crate::runner::fake::ok;
use crate::runner::{RealRunner, Runner};
use crate::scenarios::{World, agent_json, pane_json};
use crate::thread;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn own(project: &Project) {
    let (device, inode) = project_identity(project).unwrap();
    project::write_json(
        &project.record_file("journey.json"),
        &Owned {
            slug: project.slug.clone(),
            reviewer_recipe: "small".into(),
            device,
            inode,
        },
    )
    .unwrap();
}

#[test]
fn generated_repository_passes_the_actual_box_start_preflight_with_real_git() {
    let world = World::new();
    world
        .runner
        .on_fn(|cmd| cmd.program == "git", |cmd| RealRunner.run(cmd));
    let config = format!(
        "[routing]\ndefault = 'small'\n[recipes.small]\nkind = 'pi'\nprovider = 'opencode-go'\nargs = ['--provider', 'opencode-go', '--model', 'deepseek-v4.1-flash', '--thinking', 'high', '--no-skills']\n{}",
        crate::remote::TEST_MACHINE
    );
    std::fs::write(world.ctx().config_dir.join("config.toml"), config).unwrap();
    world.runner.on("machine list --json", ok(r#"[{"id":"box-id","label":"buildbox","target":"buildbox-pi","session":"default","enabled":true}]"#));
    world
        .runner
        .on_fn(crate::box_helper::tests::is_doctor, |cmd| {
            Ok(crate::doctor::boundary_diagnostic_output(
                cmd, 99_999_999, None,
            ))
        });
    let scratch = world.home.path().join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let url = "git://127.0.0.1:43123/remote.git";
    let (repo, bare) = prepare_transport_repo(&world.ctx(), &scratch, url).unwrap();
    let project = world.project("journey-preflight", "scratch.sock");
    own(&project);
    let settings = project::Settings {
        repos: vec![project::Repo {
            path: repo.to_string_lossy().into_owned(),
            branch: Some("main".into()),
            push_remote: Some("journey".into()),
            box_path: Some("/box/journey-preflight/repo".into()),
            publish_url: Some(url.into()),
            gates: Some(Vec::new()),
            ..Default::default()
        }],
        ..Default::default()
    };
    project::write_atomic(
        &project.project_md(),
        format!("+++\n{}+++\n", toml::to_string(&settings).unwrap()).as_bytes(),
    )
    .unwrap();
    *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
    let args = || crate::threads::StartArgs {
        title: "journey box".into(),
        repo: Some(repo.to_string_lossy().into_owned()),
        machine: Some("buildbox".into()),
        recipe: Some("small".into()),
        task: "Write one file, commit, seal.".into(),
        base: None,
        workflow: None,
        task_id: String::new(),
        review_id: String::new(),
        attach: Vec::new(),
        paths: Vec::new(),
    };
    // This goes through threads::start -> placement -> remote_for_url -> allocation.
    // No Git command, nor remote_for_url itself, is faked.
    let started = crate::threads::start(&world.ctx(), &project.slug, args()).unwrap();
    assert!(started.is_remote());
    assert_eq!(started.machine, "buildbox");
    assert_eq!(
        crate::remote::remote_for_url(world.ctx().runner, repo.to_str().unwrap(), url).unwrap(),
        "journey-transport"
    );
    assert_eq!(
        git(&world.ctx(), &repo, &["remote", "get-url", "journey"]).unwrap(),
        bare.to_string_lossy()
    );
    let before = thread::list(&project).len();
    git(
        &world.ctx(),
        &repo,
        &["remote", "remove", "journey-transport"],
    )
    .unwrap();
    let error = crate::threads::start(&world.ctx(), &project.slug, args()).unwrap_err();
    assert!(format!("{error:#}").contains("no_url_remote"), "{error:#}");
    assert_eq!(
        thread::list(&project).len(),
        before,
        "preflight must fail before allocation"
    );
}

struct Actor {
    machine: String,
    workspace: String,
    tab: String,
    pane: String,
    cwd: String,
    name: String,
    agent: bool,
    alive: bool,
    child: Child,
}
impl Drop for Actor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn sleeper() -> Child {
    Command::new("sleep")
        .arg("300")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

struct Lifecycle<'a> {
    inner: &'a dyn Runner,
    endpoint: Ctx<'a>,
    actors: Rc<RefCell<Vec<Actor>>>,
    session: OwnedSession,
    running: Cell<bool>,
    server_pid: u32,
    calls: RefCell<Vec<Cmd>>,
}
impl Runner for Lifecycle<'_> {
    fn run(&self, cmd: &Cmd) -> Result<Output> {
        self.calls.borrow_mut().push(cmd.clone());
        if cmd.program == "git" {
            return RealRunner.run(cmd);
        }
        let line = cmd.display();
        let machine = cmd
            .args
            .iter()
            .position(|a| a == "--machine")
            .map(|i| cmd.args[i + 1].as_str())
            .unwrap_or("");
        if line.contains("session list --json") {
            return Ok(ok(&serde_json::json!({"sessions": [{"name": self.session.name, "socket_path": self.session.socket, "running": self.running.get()}]}).to_string()));
        }
        if line.contains("agent list") || line.contains("pane list") {
            let actors = self.actors.borrow();
            let agent = line.contains("agent list");
            let entries = actors
                .iter()
                .filter(|a| a.alive && a.machine == machine && (!agent || a.agent))
                .map(|a| {
                    if agent {
                        agent_json(&a.workspace, &a.tab, &a.pane, &a.cwd, &a.name, "working")
                    } else {
                        pane_json(&a.workspace, &a.tab, &a.pane, &a.cwd)
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            return Ok(ok(&format!(
                "{{\"result\":{{\"{}\":[{entries}]}}}}",
                if agent { "agents" } else { "panes" }
            )));
        }
        if line.contains("tab close")
            || line.contains("workspace close")
            || line.contains("session stop")
        {
            let stop = line.contains("session stop");
            if stop {
                assert!(line.contains(&self.session.name));
                assert!(machine.is_empty(), "never stop a shared remote session");
                self.running.set(false);
                let out = RealRunner.run(
                    &Cmd::new("kill", Duration::from_secs(2))
                        .args(["-TERM", &self.server_pid.to_string()]),
                )?;
                assert!(out.success());
            }
            let id = cmd.args.last().unwrap();
            for actor in self.actors.borrow_mut().iter_mut().filter(|a| {
                a.alive && a.machine == machine && (stop || &a.tab == id || &a.workspace == id)
            }) {
                actor.child.kill()?;
                actor.child.wait()?;
                actor.alive = false;
            }
            return Ok(ok(r#"{"result":{}}"#));
        }
        // Only a filesystem presence check is needed by retained box retirement;
        // no remote checkout removal is allowed in the diagnostic path.
        if cmd.program == "ssh" {
            if line.contains("HERDR_ADE_BOX_INPUT") {
                return crate::box_helper::tests::respond(
                    &self.endpoint,
                    cmd.stdin.as_deref().unwrap(),
                );
            }
            if line.contains("__HERDR_WORKTREE_PRESENT__") {
                return Ok(ok("__HERDR_WORKTREE_PRESENT__\n"));
            }
            return Ok(ok(""));
        }
        self.inner.run(cmd)
    }
}

fn actor(lane: &thread::Thread) -> Actor {
    Actor {
        machine: lane.machine.clone(),
        workspace: lane.workspace_id.clone(),
        tab: lane.tab_id.clone(),
        pane: lane.pane_id.clone(),
        cwd: lane.cwd.clone(),
        name: lane.agent_name.clone(),
        agent: true,
        alive: true,
        child: sleeper(),
    }
}

#[test]
fn forced_deadline_cancels_active_review_workers_coordinator_and_session_but_retains_evidence() {
    deadline_shutdown(false);
}

#[test]
fn pending_reviewer_preservation_still_stops_all_owned_processes_and_reports_unverified_cleanup() {
    deadline_shutdown(true);
}

fn deadline_shutdown(pending: bool) {
    let fx = crate::testkit::fixture();
    fx.world.runner.on(
        "machine list --json",
        ok(r#"[{"id":"box","label":"box","target":"box","session":"default","enabled":true}]"#),
    );
    // Fixture records use the same real filesystem/Git stores as the journey.
    let project = fx.world.project("journey-deadline", "deadline.sock");
    fx.world.add_repo(&project, fx.repo.to_str().unwrap());
    let (mut settings, _) = project.read_project_md().unwrap();
    settings.repos[0].box_path = Some(fx.repo.to_string_lossy().into_owned());
    settings.repos[0].publish_url = Some("/scratch/remote.git".into());
    project::write_atomic(
        &project.project_md(),
        format!("+++\n{}+++\n", toml::to_string(&settings).unwrap()).as_bytes(),
    )
    .unwrap();
    own(&project);
    let mut lanes = Vec::new();
    for (n, role, machine) in [(1, "lane", ""), (2, "lane", "box"), (3, "reviewer", "")] {
        let (id, sha) = fx.lane(n);
        let source = thread::load(&fx.project, &id).unwrap();
        let lane = thread::allocate(&project, |lane| {
            lane.title = format!("journey {n}");
            lane.role = role.into();
            lane.status = thread::Status::Open;
            lane.kind = thread::Kind::Worktree;
            lane.agent = "claude".into();
            lane.agent_name = thread::agent_name(&project.slug, &lane.id);
            lane.workspace_id = if machine.is_empty() {
                format!("w{}", n * 10)
            } else {
                "wb".into()
            };
            lane.tab_id = format!("{}:t{n}", lane.workspace_id);
            lane.pane_id = format!("{}:p{n}", lane.workspace_id);
            lane.machine = machine.into();
            lane.repo = source.repo.clone();
            lane.branch = source.branch.clone();
            lane.worktree_path = source.worktree_path.clone();
            lane.cwd = source.cwd.clone();
            lane.thread_dir = thread::thread_dir(&lane.cwd, &project.slug, &lane.id);
        })
        .unwrap();
        std::fs::create_dir_all(&lane.thread_dir).unwrap();
        std::fs::write(
            Path::new(&lane.thread_dir).join("report.md"),
            format!("diagnostic report {n}\n"),
        )
        .unwrap();
        if machine == "box" {
            let hash = thread::store_artifact(&project, b"remote diagnostic report\n").unwrap();
            thread::update(&project, &lane.id, |l| l.report_hash = hash.clone()).unwrap();
        }
        lanes.push((lane, sha));
    }
    if pending {
        let dir = Path::new(&lanes[2].0.thread_dir);
        std::fs::write(
            dir.join("evidence.txt"),
            "retain this diagnostic attachment\n",
        )
        .unwrap();
        std::fs::write(dir.join("report.md"), "[diagnosis](evidence.txt)\n").unwrap();
    }
    let review = crate::review::Review {
        id: "review-1".into(),
        repo: fx.repo.to_string_lossy().into_owned(),
        integration: "main".into(),
        base: crate::testkit::git(&fx.repo, &["rev-parse", "main"]),
        candidate_branch: String::new(),
        members: vec![crate::review::Member {
            thread: lanes[0].0.id.clone(),
            attempt: 1,
            event: "source-seal".into(),
            sha: lanes[0].1.clone(),
            branch: lanes[0].0.branch.clone(),
            artifact: String::new(),
        }],
        gates: Vec::new(),
        selected_gates: Vec::new(),
        gates_note: String::new(),
        reviewer: Some(lanes[2].0.id.clone()),
        phase: crate::review::Phase::Reviewing,
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
        push: false,
        install: false,
        install_result: String::new(),
        close: false,
        prune: false,
        attention: String::new(),
        no_verdict_since: String::new(),
        notices: Vec::new(),
    };
    crate::review::save(&project, &review).unwrap();
    let c = project.coordinator().unwrap();
    let owned = OwnedSession {
        name: format!("scratch-{}", project.slug),
        socket: c.socket.clone().into(),
        inode: c.server_socket_inode,
    };
    let server = sleeper();
    let mut all = lanes.iter().map(|(l, _)| actor(l)).collect::<Vec<_>>();
    all.push(Actor {
        machine: String::new(),
        workspace: c.workspace_id.clone(),
        tab: c.tab_id.clone(),
        pane: c.pane_id.clone(),
        cwd: c.cwd.clone(),
        name: c.agent_name.clone(),
        agent: true,
        alive: true,
        child: sleeper(),
    });
    // A run-owned initial shell, and an unrelated agent on the shared box.
    all.push(Actor {
        machine: String::new(),
        workspace: "initial".into(),
        tab: "initial:t1".into(),
        pane: "initial:p1".into(),
        cwd: c.cwd.clone(),
        name: String::new(),
        agent: false,
        alive: true,
        child: sleeper(),
    });
    all.push(Actor {
        machine: "box".into(),
        workspace: "other".into(),
        tab: "other:t1".into(),
        pane: "other:p1".into(),
        cwd: "/other-project".into(),
        name: "other-agent".into(),
        agent: true,
        alive: true,
        child: sleeper(),
    });
    let actors = Rc::new(RefCell::new(all));
    let runner = Lifecycle {
        inner: &fx.world.runner,
        endpoint: Ctx {
            runner: &RealRunner,
            ..fx.world.ctx()
        },
        actors: actors.clone(),
        session: owned.clone(),
        running: Cell::new(true),
        server_pid: server.id(),
        calls: RefCell::new(Vec::new()),
    };
    let ctx = Ctx {
        runner: &runner,
        ..fx.world.ctx()
    };
    let mut resources = RunResources {
        server: Some(server),
        session: Some(owned),
    };
    let mut report = Report {
        project: project.slug.clone(),
        created: true,
        ..Default::default()
    };
    assert!(
        report
            .step("Mac seal", || bounded_poll::<String>(
                Instant::now(),
                "Mac seal",
                OBSERVE,
                || panic!("forced deadline must not wait")
            ))
            .is_err()
    );
    // Crucially this runs AFTER the work deadline; it must not reuse an expired
    // observer runner and accidentally turn every shutdown command into a no-op.
    let stopped = shutdown_failed_run(
        &ctx,
        &report,
        &mut resources,
        Instant::now() + Duration::from_secs(30),
    );
    if pending {
        assert!(format!("{:#}", stopped.unwrap_err()).contains("shutdown not fully verified"));
    } else {
        assert!(stopped.is_ok(), "{stopped:?}");
    }
    assert!(!runner.running.get());
    assert!(
        resources
            .server
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_some()
    );
    for actor in actors.borrow_mut().iter_mut() {
        if actor.workspace == "other" {
            assert!(actor.alive && actor.child.try_wait().unwrap().is_none());
        } else {
            assert!(
                !actor.alive && actor.child.try_wait().unwrap().is_some(),
                "{} survived",
                actor.pane
            );
        }
    }
    assert!(project.project_md().exists());
    assert_eq!(project.status(), project::Status::Paused);
    assert!(project.coordinator().unwrap().socket.is_empty());
    let cancelled_review = crate::review::load(&project, &review.id).unwrap();
    assert_eq!(
        cancelled_review.phase,
        if pending {
            crate::review::Phase::Cancelling
        } else {
            crate::review::Phase::Cancelled
        }
    );
    if pending {
        assert!(
            Path::new(&lanes[2].0.thread_dir)
                .join("evidence.txt")
                .exists()
        );
        assert!(
            thread::load(&project, &lanes[2].0.id)
                .unwrap()
                .cleanup_pending
        );
    }
    assert!(
        !cancelled_review.fast_forward
            && !cancelled_review.push
            && !cancelled_review.install
            && !cancelled_review.prune
    );
    for (lane, sha) in &lanes {
        let record = thread::load(&project, &lane.id).unwrap();
        assert_eq!(record.status, thread::Status::Resolved);
        assert!(record.retirement.as_ref().unwrap().keep_checkout);
        assert!(
            record
                .retirement_request(thread::RetirementRequest::default())
                .keep_checkout
        );
        // A preexisting retained-removal pin must also acquire the diagnostic
        // retention choice, without losing its authority or retained tip.
        let mut pinned = record.clone();
        pinned.retirement = Some(thread::RetirementRequest {
            authority: thread::RetirementAuthority::Retained,
            retained_tip: sha.clone(),
            ..Default::default()
        });
        let request = pinned.retirement_request(thread::RetirementRequest {
            keep_checkout: true,
            ..Default::default()
        });
        assert!(request.keep_checkout);
        assert_eq!(request.authority, thread::RetirementAuthority::Retained);
        assert_eq!(request.retained_tip, *sha);
        assert_eq!(record.worktree_path, lane.worktree_path);
        assert!(Path::new(&record.worktree_path).exists());
        assert!(Path::new(&record.thread_dir).join("report.md").exists());
        assert_eq!(
            crate::testkit::git(&fx.repo, &["rev-parse", &record.branch]),
            *sha
        );
    }
    assert!(
        !runner
            .calls
            .borrow()
            .iter()
            .any(|c| c.args.iter().any(|a| a == "delete"))
    );
    assert!(
        runner
            .calls
            .borrow()
            .iter()
            .all(|c| c.timeout <= Duration::from_secs(30)),
        "shutdown commands must share the cleanup deadline"
    );
}

#[test]
fn changed_session_incarnation_is_not_shutdown_by_name() {
    let world = World::new();
    let socket = world.home.path().join("old.sock");
    std::fs::write(&socket, "old").unwrap();
    let owned = OwnedSession {
        name: "scratch-journey-identity".into(),
        inode: crate::ticker::socket_inode(&socket),
        socket: socket.clone(),
    };
    // Keep the old inode allocated while replacing the path.
    std::fs::rename(&socket, world.home.path().join("kept-old.sock")).unwrap();
    std::fs::write(&socket, "replacement").unwrap();
    *world.sessions.borrow_mut() =
        serde_json::json!([{"name": owned.name, "socket_path": socket, "running": true}])
            .to_string();
    assert!(
        stop_session(&world.ctx(), &owned)
            .unwrap_err()
            .to_string()
            .contains("incarnation changed")
    );
    assert_eq!(world.runner.count("session stop"), 0);
}
