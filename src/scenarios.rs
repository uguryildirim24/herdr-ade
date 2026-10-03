//! Multi-step behaviour checked against the scripted fake runner: what the
//! CLI and the ticker do together, without herdr, git or an agent.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::contracts::{DonePayload, Event, EventPayload, Recipient};
use crate::coordinator;
use crate::paths::{Ctx, Env};
use crate::project::{self, Project};
use crate::runner::Cmd;
use crate::runner::fake::{FakeRunner, fail, ok};
use crate::thread::{self, Kind, Status, Thread};
use crate::threads::{self, ResolveArgs, StartArgs};
use crate::ticker;

pub struct World {
    pub home: tempfile::TempDir,
    pub env: Env,
    pub root: PathBuf,
    pub runner: FakeRunner,
    /// JSON arrays served for live lists, changeable mid-test.
    pub agents: Rc<RefCell<String>>,
    pub panes: Rc<RefCell<String>>,
    pub sessions: Rc<RefCell<String>>,
}

impl World {
    pub fn new() -> World {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let env = Env::for_test(home.path(), &[]);
        std::fs::create_dir_all(home.path().join("cfg")).unwrap();
        std::fs::write(home.path().join("cfg/config.toml"), "[routing]\ndefault = \"test_claude\"\nretries = 1\n\n[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n\n[machines.box]\ntarget = \"box\"\nsession = \"default\"\nhome = \"/home/agent\"\nroot = \"/home/agent/.herdr-ade\"\nworktrees = \"/home/agent/projects\"\nbuild = \"/home/agent/build/lanes\"\npath = \"/home/agent/.local/bin:/usr/bin:/bin\"\nade_bin = \"/home/agent/.local/bin/herdr-ade\"\npi_bin = \"/home/agent/.local/bin/herdr-pi\"\nkinds = [\"claude\"]\n").unwrap();
        let world = World {
            env,
            root,
            runner: FakeRunner::new(),
            agents: Rc::new(RefCell::new("[]".into())),
            panes: Rc::new(RefCell::new("[]".into())),
            sessions: Rc::new(RefCell::new("[]".into())),
            home,
        };
        world.runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, cursor, agy]"),
        );
        world
            .runner
            .on_fn(|cmd| cmd.program == "claude", |_| Ok(ok("OK\n")));
        world
            .runner
            .on_fn(|cmd| cmd.program == "agy", |_| Ok(ok("OK\n")));
        let agents = world.agents.clone();
        world.runner.on_fn(
            |cmd| {
                cmd.program != "ssh" && cmd.program != "scp" && cmd.display().contains("agent list")
            },
            move |_| {
                Ok(ok(&format!(
                    r#"{{"result":{{"agents":{}}}}}"#,
                    agents.borrow()
                )))
            },
        );
        let panes = world.panes.clone();
        world.runner.on_fn(
            |cmd| {
                cmd.program != "ssh" && cmd.program != "scp" && cmd.display().contains("pane list")
            },
            move |_| {
                Ok(ok(&format!(
                    r#"{{"result":{{"panes":{}}}}}"#,
                    panes.borrow()
                )))
            },
        );
        let sessions = world.sessions.clone();
        world.runner.on_fn(
            |cmd| cmd.display().contains("session list --json"),
            move |_| Ok(ok(&format!(r#"{{"sessions":{}}}"#, sessions.borrow()))),
        );
        world.runner.on("session stop", ok(r#"{}"#));
        world.runner.on("session delete", ok(r#"{}"#));
        world.runner.on("report-metadata", ok(r#"{"result":{}}"#));
        // `thread resolve` closes a dedicated workspace or a shared tab.
        world.runner.on("workspace close", ok(r#"{"result":{}}"#));
        world.runner.on("tab close", ok(r#"{"result":{}}"#));
        world
    }

    pub fn ctx(&self) -> Ctx<'_> {
        Ctx {
            env: &self.env,
            root: self.root.clone(),
            config_dir: self.home.path().join("cfg"),
            runner: &self.runner,
            detached_ticker: false,
        }
    }

    /// A project that has been opened: coordinator in `w1:p1` of `socket`.
    pub fn project(&self, slug: &str, socket: &str) -> Project {
        let project = project::create(&self.root, slug, "", vec![]).unwrap();
        // Scenario fixtures may write coordinator-owned files directly instead
        // of exercising their first-use commands.
        for dir in ["tasks", "inbox", "inbox/done"] {
            std::fs::create_dir_all(project.state_dir().join(dir)).unwrap();
        }
        let socket = self.home.path().join(socket);
        std::fs::write(&socket, b"").unwrap();
        let cwd = project.canonical_dir().to_string_lossy().into_owned();
        project
            .update_coordinator(|c| {
                c.socket = socket.to_string_lossy().into_owned();
                c.workspace_id = "w1".into();
                c.tab_id = "w1:t1".into();
                c.pane_id = "w1:p1".into();
                c.agent_name = format!("hp-{slug}-coordinator");
                c.cwd = cwd;
                c.server_socket_inode = ticker::socket_inode(&socket);
            })
            .unwrap();
        project
    }

    /// Add a local repository to the project's `PROJECT.md`.
    pub fn add_repo(&self, project: &Project, path: &str) {
        let (mut settings, body) = project.read_project_md().unwrap();
        settings.repos.push(project::Repo {
            path: path.to_string(),
            ..project::Repo::default()
        });
        let front = toml::to_string(&settings).unwrap();
        std::fs::write(project.project_md(), format!("+++\n{front}+++\n\n{body}")).unwrap();
    }

    pub fn coordinator_pane(&self, project: &Project) -> String {
        pane_json(
            "w1",
            "w1:t1",
            "w1:p1",
            &project.canonical_dir().to_string_lossy(),
        )
    }

    /// A thread record placed in pane `w2:p1`, working directory `cwd`.
    pub fn thread(
        &self,
        project: &Project,
        cwd: &Path,
        change: impl FnOnce(&mut Thread),
    ) -> Thread {
        let dir = thread::thread_dir(&cwd.to_string_lossy(), &project.slug, "t-0001");
        let t = thread::allocate(project, |t| {
            t.title = "Task".into();
            t.kind = Kind::Worktree;
            t.status = Status::Open;
            t.agent = "claude".into();
            t.agent_name = thread::agent_name(&project.slug, "t-0001");
            t.workspace_id = "w2".into();
            t.tab_id = "w2:t1".into();
            t.pane_id = "w2:p1".into();
            t.cwd = cwd.to_string_lossy().into_owned();
            t.worktree_path = t.cwd.clone();
            t.repo = "/repo".into();
            t.thread_dir = dir;
        })
        .unwrap();
        thread::update(project, &t.id, change).unwrap()
    }
}

pub fn pane_json(workspace: &str, tab: &str, pane: &str, cwd: &str) -> String {
    format!(r#"{{"pane_id":"{pane}","tab_id":"{tab}","workspace_id":"{workspace}","cwd":"{cwd}"}}"#)
}

fn record_stored_report(project: &Project, thread_id: &str) {
    let bytes = format!("report for {thread_id}\n").into_bytes();
    let artifact = thread::sha256_hex(&bytes);
    let artifact_path = crate::events::artifact_path(project, &artifact);
    std::fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
    std::fs::write(&artifact_path, bytes).unwrap();
    let attempt = thread::load(project, thread_id).unwrap().attempt.max(1);
    crate::events::seal_create_if_absent(
        project,
        &Event {
            id: format!("{thread_id}-{attempt}-done"),
            op: format!("{thread_id}-{attempt}-done"),
            thread: thread_id.into(),
            attempt,
            recipient: Recipient::default(),
            created: project::now(),
            payload: EventPayload {
                done: Some(DonePayload {
                    has_changes: None,
                    sha: "lane-sha".into(),
                    report_path: format!(".reports/{thread_id}.md"),
                    artifact,
                    attestation: None,
                    published_ref: None,
                }),
                ..EventPayload::default()
            },
        },
    )
    .unwrap();
}

pub fn agent_json(
    workspace: &str,
    tab: &str,
    pane: &str,
    cwd: &str,
    name: &str,
    state: &str,
) -> String {
    format!(
        r#"{{"pane_id":"{pane}","tab_id":"{tab}","workspace_id":"{workspace}","cwd":"{cwd}","name":"{name}","agent":"claude","agent_status":"{state}"}}"#
    )
}

fn socket_of(cmd: &Cmd) -> String {
    cmd.env
        .iter()
        .find(|(k, _)| k == "HERDR_SOCKET_PATH")
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

#[test]
fn one_agent_start_per_project_per_tick_and_missing_agent_state_stays_unknown() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let cwd = world.home.path().to_path_buf();
    let cwd_text = cwd.to_string_lossy().into_owned();
    world.thread(&project, &cwd, |t| t.prompt_pending = true);
    let second = thread::allocate(&project, |t| {
        t.status = Status::Open;
        t.kind = Kind::Tab;
        t.prompt_pending = true;
        t.agent = "claude".into();
        t.agent_name = "hp-demo-t-0002".into();
        t.workspace_id = "w1".into();
        t.tab_id = "w1:t2".into();
        t.pane_id = "w1:p2".into();
        t.cwd = cwd_text.clone();
    })
    .unwrap();
    *world.panes.borrow_mut() = format!(
        "[{},{}]",
        pane_json("w2", "w2:t1", "w2:p1", &cwd_text),
        pane_json("w1", "w1:t2", "w1:p2", &cwd_text)
    );
    world.runner.on(
        "agent start",
        fail(
            1,
            r#"{"error":{"code":"timeout","message":"timed out waiting for agent startup"}}"#,
        ),
    );

    let ctx = world.ctx();
    for tick in 1..=6 {
        let _ = ticker::tick_project(&ctx, &project);
        assert_eq!(
            world.runner.count("agent start"),
            tick,
            "one lane start per tick; a closed coordinator is not relaunched"
        );
    }
    // Six lane starts: three each. The absent coordinator stays unavailable.
    // Absent agent state in the listed lane panes is still unknown.
    let _ = ticker::tick_project(&ctx, &project);
    let _ = ticker::tick_project(&ctx, &project);
    assert_eq!(world.runner.count("agent start"), 6);
    assert_eq!(world.runner.count("tab close"), 0);
    assert!(project.coordinator().unwrap().closed_by_rolf_at.is_empty());
    assert!(
        inbox::unhandled(&project)
            .iter()
            .any(|i| i.summary.contains("coordinator_unavailable"))
    );
    for id in ["t-0001", &second.id] {
        let t = thread::load(&project, id).unwrap();
        assert_eq!(t.status, Status::Open, "{id}");
        assert_eq!(t.launch_attempts, thread::MAX_LAUNCH_ATTEMPTS, "{id}");
        assert!(t.prompt_pending, "{id}");
        assert!(t.error.is_empty(), "{id}: {}", t.error);
        assert_eq!(t.last_group, thread::Group::Unknown.token(), "{id}");
    }
}

#[test]
fn two_projects_in_two_sockets_sharing_a_pane_id_do_not_mix() {
    let world = World::new();
    let a = world.project("alpha", "a.sock");
    let b = world.project("beta", "b.sock");
    let cwd = world.home.path().to_string_lossy().into_owned();
    for project in [&a, &b] {
        world.thread(project, world.home.path(), |t| t.prompt_pending = true);
    }
    // Only beta's session has the agent; both record pane w2:p1.
    let b_socket = b.coordinator().unwrap().socket;
    let beta_agents = format!(
        r#"{{"result":{{"agents":[{}]}}}}"#,
        agent_json("w2", "w2:t1", "w2:p1", &cwd, "hp-beta-t-0001", "idle")
    );
    let world2 = World {
        runner: FakeRunner::new(),
        ..world
    };
    let socket = b_socket.clone();
    world2.runner.on_fn(
        move |cmd| cmd.display().contains("agent list") && socket_of(cmd) == socket,
        move |_| Ok(ok(&beta_agents)),
    );
    world2
        .runner
        .on("agent list", ok(r#"{"result":{"agents":[]}}"#));
    world2
        .runner
        .on("pane list", ok(r#"{"result":{"panes":[]}}"#));
    world2.runner.on("agent prompt", ok(r#"{"result":{}}"#));
    world2.runner.on("report-metadata", ok(r#"{"result":{}}"#));

    let ctx = world2.ctx();
    ticker::tick_project(&ctx, &a).unwrap();
    ticker::tick_project(&ctx, &b).unwrap();
    let calls = world2.runner.calls.borrow();
    let prompts: Vec<_> = calls
        .iter()
        .filter(|c| c.display().contains("agent prompt"))
        .collect();
    assert_eq!(prompts.len(), 1);
    assert_eq!(socket_of(prompts[0]), b_socket);
    assert!(prompts[0].display().contains(".herdr-project/"));
    assert!(prompts[0].display().contains("/brief.md"));
    drop(calls);
    assert!(thread::load(&a, "t-0001").unwrap().prompt_pending);
    assert!(!thread::load(&b, "t-0001").unwrap().prompt_pending);
}

#[test]
fn the_ticker_hashes_a_changed_report_without_copying_it() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let t = world.thread(&project, world.home.path(), |_| {});
    std::fs::create_dir_all(Path::new(&t.thread_dir)).unwrap();
    std::fs::write(
        Path::new(&t.thread_dir).join("report.md"),
        "## Report\nv1\n",
    )
    .unwrap();
    let ctx = world.ctx();
    ticker::tick_project(&ctx, &project).unwrap();
    let after = thread::load(&project, "t-0001").unwrap();
    assert_eq!(after.report_hash, thread::sha256_hex(b"## Report\nv1\n"));
    assert!(!after.last_report_change.is_empty());
    assert!(!thread::home_report_path(&project, "t-0001").exists());

    let stamp = after.last_report_change.clone();
    ticker::tick_project(&ctx, &project).unwrap();
    assert_eq!(
        thread::load(&project, "t-0001").unwrap().last_report_change,
        stamp
    );
}

#[test]
fn rebind_moves_an_existing_thread_to_its_verified_live_agent() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let cwd = world.home.path().to_string_lossy().into_owned();
    world.thread(&project, world.home.path(), |t| t.status = Status::Failed);
    *world.panes.borrow_mut() = format!(
        "[{},{}]",
        world.coordinator_pane(&project),
        pane_json("w3", "w3:t1", "w3:p1", &cwd)
    );
    *world.agents.borrow_mut() = format!(
        "[{}]",
        agent_json("w3", "w3:t1", "w3:p1", &cwd, "hp-demo-t-0001", "idle")
    );

    let outcome = threads::rebind(&world.ctx(), "demo", "t-0001", "w3:p1").unwrap();
    assert_eq!(outcome.pane_id, "w3:p1");
    let record = thread::load(&project, "t-0001").unwrap();
    assert_eq!(record.status, Status::Open);
    assert_eq!(record.workspace_id, "w3");
    assert_eq!(record.tab_id, "w3:t1");
    assert_eq!(record.identity.pane_id, "w3:p1");
}

#[test]
fn an_adopted_no_repo_thread_rebinds_at_its_original_cwd() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let cwd = world.home.path().join("original");
    std::fs::create_dir_all(&cwd).unwrap();
    let managed = project.state_dir().join("threads/t-0001");
    std::fs::create_dir_all(managed.join(".git")).unwrap();
    world.thread(&project, &cwd, |t| {
        t.kind = Kind::Adopted;
        t.status = Status::Failed;
        t.repo.clear();
        t.worktree_path = std::fs::canonicalize(&managed)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        t.agent_name.clear();
    });
    let cwd = cwd.to_string_lossy().into_owned();
    *world.panes.borrow_mut() = format!(
        "[{},{}]",
        world.coordinator_pane(&project),
        pane_json("w3", "w3:t1", "w3:p1", &cwd)
    );
    *world.agents.borrow_mut() =
        format!("[{}]", agent_json("w3", "w3:t1", "w3:p1", &cwd, "", "idle"));

    let outcome = threads::rebind(&world.ctx(), "demo", "t-0001", "w3:p1").unwrap();
    assert_eq!(outcome.pane_id, "w3:p1");
    let record = thread::load(&project, "t-0001").unwrap();
    assert_eq!(record.status, Status::Open);
    assert_eq!(record.cwd, cwd);
    assert_eq!(
        Path::new(&record.worktree_path),
        std::fs::canonicalize(managed).unwrap().as_path()
    );
}

#[test]
fn cancel_reports_pending_cleanup_when_the_session_is_unreachable() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.thread(&project, world.home.path(), |t| {
        t.kind = crate::thread::Kind::Tab;
        t.status = Status::Open;
        t.workspace_id = "w1".into();
        t.tab_id = "w1:t2".into();
        t.pane_id = "w1:p2".into();
        t.worktree_path.clear();
    });
    std::fs::remove_file(world.home.path().join("a.sock")).unwrap();
    *world.sessions.borrow_mut() = serde_json::json!([{
        "name": "demo",
        "running": true,
        "socket_path": world.home.path().join("a.sock"),
    }])
    .to_string();

    let outcome = threads::cancel(&world.ctx(), "demo", "t-0001", "no longer needed").unwrap();
    assert_eq!(outcome.state, "cleanup_pending");
    assert_eq!(outcome.pane, "cleanup_pending");
    assert!(outcome.worktree_reason.is_some());
    let record = thread::load(&project, "t-0001").unwrap();
    assert_eq!(record.status, Status::Resolved);
    assert!(record.cleanup_pending);
    assert_eq!(world.runner.count("tab close"), 0);

    std::fs::write(world.home.path().join("a.sock"), "").unwrap();
    *world.panes.borrow_mut() = format!(
        "[{},{}]",
        world.coordinator_pane(&project),
        pane_json("w1", "w1:t2", "w1:p2", &world.home.path().to_string_lossy())
    );
    threads::retry_pending_cleanup(&world.ctx(), &project).unwrap();
    assert!(!thread::load(&project, "t-0001").unwrap().cleanup_pending);
    assert_eq!(world.runner.count("tab close w1:t2"), 1);
}

#[test]
fn cancel_keeps_non_disposable_ignored_data() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let worktree = world.home.path().join("cancel-data-worktree");
    std::fs::create_dir_all(worktree.join("runs")).unwrap();
    std::fs::write(worktree.join("runs/raw.bin"), vec![0; 2048]).unwrap();
    let t = world.thread(&project, &worktree, |thread| {
        thread.repo = "/repo".into();
        thread.branch = "lane".into();
    });
    world.runner.on(
        "status --porcelain --ignored --untracked-files=all",
        ok("!! runs/raw.bin\0"),
    );

    let outcome = threads::cancel(&world.ctx(), "demo", &t.id, "stop this run").unwrap();

    assert_eq!(outcome.state, "cancelled");
    assert_eq!(outcome.worktree, "kept");
    let reason = outcome.worktree_reason.unwrap();
    assert!(reason.starts_with("ignored_data:"), "{reason}");
    assert!(
        reason.contains("runs") && reason.contains("KiB"),
        "{reason}"
    );
    assert_eq!(world.runner.count("worktree remove"), 0);
    assert_eq!(
        thread::load(&project, &t.id).unwrap().worktree_path,
        worktree.to_string_lossy()
    );
}

#[test]
fn cancel_uses_the_repository_specific_disposable_list() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.add_repo(&project, "/repo");
    let (mut settings, body) = project.read_project_md().unwrap();
    settings.repos[0].disposable = vec!["runs/pytest-*".into()];
    let front = toml::to_string(&settings).unwrap();
    std::fs::write(project.project_md(), format!("+++\n{front}+++\n\n{body}")).unwrap();
    let worktree = world.home.path().join("cancel-generated-worktree");
    std::fs::create_dir_all(worktree.join("runs/pytest-cancel")).unwrap();
    std::fs::write(worktree.join("runs/pytest-cancel/cache"), "generated").unwrap();
    let t = world.thread(&project, &worktree, |thread| {
        thread.repo = "/repo".into();
        thread.branch = "lane".into();
    });
    world.runner.on(
        "status --porcelain --ignored --untracked-files=all",
        ok("!! runs/pytest-cancel/cache\0"),
    );
    world.runner.on("worktree remove", ok(""));

    let outcome = threads::cancel(&world.ctx(), "demo", &t.id, "stop this run").unwrap();

    assert_eq!(outcome.worktree, "removed");
    assert_eq!(world.runner.count("worktree remove"), 1);
}

#[test]
fn coordinator_retry_moves_an_unknown_failure_without_replacing_its_work() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let cwd = world.home.path().to_string_lossy().into_owned();
    let brief_hash = thread::store_artifact(&project, b"The frozen brief.").unwrap();
    world.thread(&project, world.home.path(), |t| {
        t.status = Status::Failed;
        t.error = "openai-codex unreachable: fetch failed".into();
        t.failure_class = crate::contracts::FailureClass::Unknown;
        t.launch_attempts = 3;
        t.attempt = 1;
        t.launch.kind = "claude".into();
        t.launch.recipe_id = "test_claude".into();
        t.launch.brief_hash = brief_hash.clone();
        t.launch.same_recipe_retries = 100; // Automatic recovery has long since stopped.
    });
    std::fs::write(thread::task_path(&project, "t-0001"), "The task.").unwrap();
    let pending = world.home.path().join("uncommitted-work.txt");
    std::fs::write(&pending, "keep me").unwrap();
    *world.panes.borrow_mut() = format!(
        "[{},{}]",
        world.coordinator_pane(&project),
        pane_json("w2", "w2:t1", "w2:p1", &cwd)
    );
    world
        .runner
        .on("rev-parse --git-path", fail(1, "not a repo"));
    world.runner.on("tab close", ok(r#"{"result":{}}"#));
    world.runner.on(
        "tab create",
        ok(&format!(
            r#"{{"result":{{"root_pane":{{"workspace_id":"w1","tab_id":"w1:t3","pane_id":"w1:p3","cwd":"{cwd}"}}}}}}"#
        )),
    );

    threads::retry(
        &world.ctx(),
        "demo",
        "t-0001",
        "the coordinator chose to retry after the network recovered",
    )
    .unwrap();
    let t = thread::load(&project, "t-0001").unwrap();
    assert_eq!(
        (t.status, t.prompt_pending, t.launch_attempts),
        (Status::Open, true, 0)
    );
    assert!(t.error.is_empty());
    assert_eq!(
        world
            .runner
            .calls
            .borrow()
            .iter()
            .filter(|call| {
                let line = call.display();
                line.contains("agent start") && !line.contains("--help")
            })
            .count(),
        0
    );
    assert_eq!(world.runner.count("agent prompt"), 0);
    // A1 M5: the bare shell's HERDR_ADE_LAUNCH names attempt 1, so attempt 2
    // gets a new tab carrying its own launch line.
    assert_eq!(t.attempt, 2);
    assert_eq!(t.pane_id, "w1:p3");
    assert_eq!(t.worktree_path, cwd);
    assert_eq!(std::fs::read_to_string(pending).unwrap(), "keep me");
    assert_eq!(t.launch.recipe_id, "test_claude");
    assert_eq!(t.launch.same_recipe_retries, 101);
    let dispatch = std::fs::read_to_string(project.state_dir().join("dispatch.jsonl")).unwrap();
    let decision: serde_json::Value =
        serde_json::from_str(dispatch.lines().last().unwrap()).unwrap();
    assert_eq!(decision["kind"], "coordinator-retry");
    assert_eq!(decision["class"], "unknown");
    assert_eq!(
        decision["failure"],
        "the coordinator chose to retry after the network recovered"
    );
    assert_eq!(world.runner.count("workspace close w2"), 1);
    let calls = world.runner.calls.borrow();
    assert!(calls.iter().any(|c| {
        let line = c.display();
        line.contains("tab create")
            && line.contains(&format!("HERDR_ADE_LAUNCH=demo/t-0001/2/{brief_hash}"))
    }));
}

#[test]
fn every_resolve_copies_first_and_a_partial_copy_keeps_the_worktree() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let t = world.thread(&project, world.home.path(), |_| {});
    let dir = PathBuf::from(&t.thread_dir);
    std::fs::create_dir_all(dir.join("library")).unwrap();
    std::fs::write(dir.join("report.md"), "late report").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", dir.join("library/link")).unwrap();
    world.runner.on("du -sk", ok("4\t/x\n"));
    world.runner.on("rsync", ok(""));
    world.runner.on("worktree remove", ok(r#"{"result":{}}"#));
    let ctx = world.ctx();

    // A partial deliverable copy resolves the thread but refuses automatic
    // removal. The unsealed report remains a draft in the lane folder.
    threads::resolve(&ctx, "demo", "t-0001", &ResolveArgs::default()).unwrap();
    assert_eq!(
        thread::load(&project, "t-0001").unwrap().status,
        Status::Resolved
    );
    assert!(!thread::home_report_path(&project, "t-0001").exists());
    assert_eq!(world.runner.count("worktree remove"), 0);

    let resolved = thread::load(&project, "t-0001").unwrap();
    assert_eq!(
        (resolved.status, resolved.resolved_reason.as_str()),
        (Status::Resolved, "manual")
    );

    // --reopen starts nothing.
    threads::resolve(
        &ctx,
        "demo",
        "t-0001",
        &ResolveArgs {
            reopen: true,
            ..ResolveArgs::default()
        },
    )
    .unwrap();
    let reopened = thread::load(&project, "t-0001").unwrap();
    assert_eq!(reopened.status, Status::Open);
    assert!(reopened.resolved_reason.is_empty());
    assert_eq!(world.runner.count("agent start"), 0);
}

#[test]
fn resolve_closes_the_pane_unless_keep_pane() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.thread(&project, world.home.path(), |_| {});
    let cwd = world.home.path().to_string_lossy().into_owned();
    *world.panes.borrow_mut() = format!(
        "[{},{}]",
        world.coordinator_pane(&project),
        pane_json("w2", "w2:t1", "w2:p1", &cwd)
    );
    world.runner.on("tab close", ok(r#"{"result":{}}"#));
    let ctx = world.ctx();

    // --keep-pane leaves the pane and its agent running.
    threads::resolve(
        &ctx,
        "demo",
        "t-0001",
        &ResolveArgs {
            keep_pane: true,
            ..ResolveArgs::default()
        },
    )
    .unwrap();
    assert_eq!(world.runner.count("tab close"), 0);

    // A plain resolve closes the dedicated lane workspace.
    threads::resolve(
        &ctx,
        "demo",
        "t-0001",
        &ResolveArgs {
            reopen: true,
            ..ResolveArgs::default()
        },
    )
    .unwrap();
    threads::resolve(&ctx, "demo", "t-0001", &ResolveArgs::default()).unwrap();
    assert_eq!(world.runner.count("workspace close w2"), 1);
    assert_eq!(world.runner.count("tab close w2:t1"), 0);
    assert_eq!(
        thread::load(&project, "t-0001").unwrap().status,
        Status::Resolved
    );
}

#[test]
fn resolve_deletes_a_leftover_isolated_scratch_session() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.thread(&project, world.home.path(), |_| {});
    *world.sessions.borrow_mut() =
        r#"[{"name":"scratch-t-0001","running":true,"socket_path":"/tmp/scratch.sock"}]"#.into();

    threads::resolve(&world.ctx(), "demo", "t-0001", &ResolveArgs::default()).unwrap();

    assert_eq!(world.runner.count("session stop scratch-t-0001"), 1);
    assert_eq!(world.runner.count("session delete scratch-t-0001"), 1);
}

#[test]
fn resolve_keeps_a_workspace_that_holds_something_else() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.thread(&project, world.home.path(), |_| {});
    let cwd = world.home.path().to_string_lossy().into_owned();
    *world.panes.borrow_mut() = format!(
        "[{},{},{}]",
        world.coordinator_pane(&project),
        pane_json("w2", "w2:t1", "w2:p1", &cwd),
        pane_json("w2", "w2:t2", "w2:p2", "/other")
    );

    threads::resolve(&world.ctx(), "demo", "t-0001", &ResolveArgs::default()).unwrap();

    assert_eq!(world.runner.count("workspace close w2"), 0);
    assert_eq!(world.runner.count("tab close w2:t1"), 1);
}

#[test]
fn resolve_keeps_the_project_workspace_when_its_coordinator_pane_is_gone() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let cwd = world.home.path().to_string_lossy().into_owned();
    world.thread(&project, world.home.path(), |thread| {
        thread.workspace_id = "w1".into();
        thread.tab_id = "w1:t2".into();
        thread.pane_id = "w1:p2".into();
    });
    *world.panes.borrow_mut() = format!("[{}]", pane_json("w1", "w1:t2", "w1:p2", &cwd));

    threads::resolve(&world.ctx(), "demo", "t-0001", &ResolveArgs::default()).unwrap();

    assert_eq!(world.runner.count("workspace close w1"), 0);
    assert_eq!(world.runner.count("tab close w1:t2"), 1);
}

#[test]
fn resolve_never_closes_an_adopted_workspace() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let cwd = world.home.path().to_string_lossy().into_owned();
    world.thread(&project, world.home.path(), |thread| {
        thread.kind = Kind::Adopted;
    });
    *world.panes.borrow_mut() = format!(
        "[{},{}]",
        world.coordinator_pane(&project),
        pane_json("w2", "w2:t1", "w2:p1", &cwd)
    );

    threads::resolve(&world.ctx(), "demo", "t-0001", &ResolveArgs::default()).unwrap();

    assert_eq!(world.runner.count("workspace close w2"), 0);
    assert_eq!(world.runner.count("tab close w2:t1"), 1);
}

/// A1 review H5: only `working` stopped a removal; an idle lane with no
/// sealed `done` lost its worktree.
#[test]
fn resolving_unlanded_work_keeps_its_worktree() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.thread(&project, world.home.path(), |_| {});
    world.runner.on("worktree remove", ok(""));
    threads::resolve(&world.ctx(), "demo", "t-0001", &ResolveArgs::default()).unwrap();
    assert_eq!(world.runner.count("worktree remove"), 0);
    let resolved = thread::load(&project, "t-0001").unwrap();
    assert_eq!(resolved.status, Status::Resolved);
    assert!(!resolved.worktree_path.is_empty());
}

#[test]
fn resolving_a_dirty_finished_worktree_keeps_it_with_a_reason() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let worktree = world.home.path().join("dirty-worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let t = world.thread(&project, &worktree, |thread| {
        thread.repo = "/repo".into();
        thread.branch = "lane".into();
    });
    thread::update(&project, &t.id, |t| t.merged_sha = "landed".into()).unwrap();
    world.runner.on(
        "status --porcelain --ignored --untracked-files=all",
        ok("?? scratch.txt\0"),
    );

    let error = threads::resolve(&world.ctx(), "demo", &t.id, &ResolveArgs::default())
        .unwrap_err()
        .to_string();

    assert!(error.contains("worktree_dirty"), "{error}");
    let kept = thread::load(&project, &t.id).unwrap();
    assert_eq!(kept.status, Status::Open);
    assert_eq!(kept.worktree_path, worktree.to_string_lossy());
    assert_eq!(world.runner.count("worktree remove"), 0);
}

#[test]
fn resolving_ignored_data_keeps_the_worktree_but_resolves_the_thread() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let worktree = world.home.path().join("data-worktree");
    std::fs::create_dir_all(worktree.join("camber-runs")).unwrap();
    std::fs::write(worktree.join("camber-runs/raw.bin"), vec![0; 2048]).unwrap();
    let t = world.thread(&project, &worktree, |thread| {
        thread.repo = "/repo".into();
        thread.branch = "lane".into();
    });
    thread::update(&project, &t.id, |t| t.merged_sha = "landed".into()).unwrap();
    world.runner.on(
        "status --porcelain --ignored --untracked-files=all",
        ok("!! camber-runs/raw.bin\0"),
    );

    let outcome = threads::resolve(&world.ctx(), "demo", &t.id, &ResolveArgs::default()).unwrap();

    assert_eq!(outcome.worktree, "kept");
    let reason = outcome.worktree_reason.unwrap();
    assert!(reason.starts_with("ignored_data:"), "{reason}");
    assert!(
        reason.contains("camber-runs") && reason.contains("KiB"),
        "{reason}"
    );
    let kept = thread::load(&project, &t.id).unwrap();
    assert_eq!(kept.status, Status::Resolved);
    assert_eq!(kept.worktree_path, worktree.to_string_lossy());
    assert_eq!(world.runner.count("worktree remove"), 0);
    assert!(
        !std::fs::read_to_string(world.home.path().join("cfg/config.toml"))
            .unwrap()
            .contains("[worktrees]")
    );
}

#[test]
fn resolving_disposable_ignored_output_removes_the_worktree() {
    let world = World::new();
    let config = world.home.path().join("cfg/config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("\n[worktrees]\ndisposable = [\"target\"]\n");
    std::fs::write(config, text).unwrap();
    let project = world.project("demo", "a.sock");
    let worktree = world.home.path().join("generated-worktree");
    std::fs::create_dir_all(worktree.join("target/debug")).unwrap();
    std::fs::write(worktree.join("target/debug/cache"), "generated").unwrap();
    let t = world.thread(&project, &worktree, |thread| {
        thread.repo = "/repo".into();
        thread.branch = "lane".into();
    });
    thread::update(&project, &t.id, |t| t.merged_sha = "landed".into()).unwrap();
    world.runner.on(
        "status --porcelain --ignored --untracked-files=all",
        ok("!! target/debug/cache\0"),
    );
    world.runner.on("worktree remove", ok(""));

    let outcome = threads::resolve(&world.ctx(), "demo", &t.id, &ResolveArgs::default()).unwrap();

    assert_eq!(outcome.worktree, "removed");
    assert_eq!(world.runner.count("worktree remove"), 1);
}

#[test]
fn resolving_a_lane_with_only_its_stored_report_removes_the_worktree() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let worktree = world.home.path().join("reported-worktree");
    std::fs::create_dir_all(worktree.join(".reports")).unwrap();
    std::fs::write(worktree.join(".reports/t-0001.md"), "lane report\n").unwrap();
    let t = world.thread(&project, &worktree, |thread| {
        thread.repo = "/repo".into();
        thread.branch = "lane".into();
    });
    thread::update(&project, &t.id, |t| t.merged_sha = "landed".into()).unwrap();
    record_stored_report(&project, &t.id);
    world.runner.on(
        "status --porcelain --ignored --untracked-files=all",
        ok("!! .reports/t-0001.md\0"),
    );
    world.runner.on("worktree remove", ok(""));

    let outcome = threads::resolve(&world.ctx(), "demo", &t.id, &ResolveArgs::default()).unwrap();

    assert_eq!(outcome.worktree, "removed");
    assert_eq!(world.runner.count("worktree remove"), 1);
}

#[test]
fn resolve_uses_the_repository_specific_disposable_list() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.add_repo(&project, "/repo");
    let (mut settings, body) = project.read_project_md().unwrap();
    settings.repos[0].disposable = vec!["runs/pytest-*".into()];
    let front = toml::to_string(&settings).unwrap();
    std::fs::write(project.project_md(), format!("+++\n{front}+++\n\n{body}")).unwrap();
    let worktree = world.home.path().join("repo-generated-worktree");
    std::fs::create_dir_all(worktree.join("runs/pytest-resolve")).unwrap();
    std::fs::write(worktree.join("runs/pytest-resolve/cache"), "generated").unwrap();
    let t = world.thread(&project, &worktree, |thread| {
        thread.repo = "/repo".into();
        thread.branch = "lane".into();
    });
    thread::update(&project, &t.id, |t| t.merged_sha = "landed".into()).unwrap();
    world.runner.on(
        "status --porcelain --ignored --untracked-files=all",
        ok("!! runs/pytest-resolve/cache\0"),
    );
    world.runner.on("worktree remove", ok(""));

    let outcome = threads::resolve(&world.ctx(), "demo", &t.id, &ResolveArgs::default()).unwrap();

    assert_eq!(outcome.worktree, "removed");
    assert_eq!(world.runner.count("worktree remove"), 1);
}

#[test]
fn a_nested_worktree_is_kept_inside_a_disposable_folder() {
    let world = World::new();
    let config = world.home.path().join("cfg/config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("\n[worktrees]\ndisposable = [\"target\"]\n");
    std::fs::write(config, text).unwrap();
    let project = world.project("demo", "a.sock");
    let worktree = world.home.path().join("nested-worktree");
    std::fs::create_dir_all(worktree.join("target/child")).unwrap();
    std::fs::write(
        worktree.join("target/child/.git"),
        "gitdir: /repo/.git/worktrees/child\n",
    )
    .unwrap();
    let t = world.thread(&project, &worktree, |thread| {
        thread.repo = "/repo".into();
        thread.branch = "lane".into();
    });
    thread::update(&project, &t.id, |t| t.merged_sha = "landed".into()).unwrap();
    world.runner.on(
        "status --porcelain --ignored --untracked-files=all",
        ok("!! target/child/output.bin\0"),
    );

    let outcome = threads::resolve(&world.ctx(), "demo", &t.id, &ResolveArgs::default()).unwrap();

    assert_eq!(outcome.worktree, "kept");
    assert!(outcome.worktree_reason.unwrap().contains("target/child"),);
    assert_eq!(
        thread::load(&project, &t.id).unwrap().status,
        Status::Resolved
    );
}

fn configure_test_box(world: &World) {
    let path = world.home.path().join("cfg/config.toml");
    let existing = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        path,
        format!(
            "{existing}{}\n[[machines.buildbox.repos]]\npath = \"/Users/agent/projects/herdr-ade\"\nbox_path = \"/home/agent/projects/herdr-ade\"\npublish_url = \"https://github.com/uguryildirim24/herdr-ade.git\"\n",
            crate::remote::TEST_MACHINE
        ),
    )
    .unwrap();
}

#[test]
fn resolving_a_merged_box_lane_uses_the_box_clone_path() {
    let world = World::new();
    configure_test_box(&world);
    let project = world.project("demo", "a.sock");
    let t = world.thread(
        &project,
        Path::new("/home/agent/projects/herdr-ade/.worktrees/t-0001"),
        |thread| {
            thread.repo = "/Users/agent/projects/herdr-ade".into();
            thread.branch = "hp/demo/t-0001-task".into();
            thread.machine = "buildbox".into();
            thread.machine_id = "buildbox-id".into();
        },
    );
    thread::update(&project, &t.id, |t| t.merged_sha = "landed".into()).unwrap();
    world
        .runner
        .on("for-each-ref --format=%(objectname) %(refname)", ok(""));
    world.runner.on("worktree list --porcelain", ok(""));
    world.runner.on("ls-remote --heads", ok(""));
    world.runner.on(
        "machine list --json",
        ok(r#"[{"id":"buildbox-id","label":"buildbox","target":"buildbox-pi","session":"default","enabled":true}]"#),
    );
    world.runner.on_fn(
        |cmd| cmd.program == "ssh",
        |cmd| {
            let line = cmd.display();
            if line.contains("__HERDR_WORKTREE_PRESENT__") {
                Ok(ok("__HERDR_WORKTREE_PRESENT__\n"))
            } else if line.contains("__HERDR_DRAFT_PRESENT__") {
                Ok(ok("__HERDR_DRAFT_ABSENT__\n"))
            } else if line.contains("status --porcelain") {
                Ok(ok("\0__HERDR_NESTED_WORKTREES__\0"))
            } else {
                Ok(ok(""))
            }
        },
    );

    threads::resolve(
        &world.ctx(),
        "demo",
        &t.id,
        &ResolveArgs {
            skip_copy: true,
            ..ResolveArgs::default()
        },
    )
    .unwrap();

    let calls = world.runner.calls.borrow();
    let removal = calls
        .iter()
        .find(|call| call.program == "ssh" && call.display().contains("git worktree remove"))
        .expect("box removal ssh call");
    let command = removal.display();
    assert!(
        command.contains("PATH=/home/agent/.local/bin:/home/agent/.cargo/bin:/usr/local/bin:/usr/bin:/bin; export PATH"),
        "{command}"
    );
    assert!(
        command.contains("cd /home/agent/projects/herdr-ade"),
        "{command}"
    );
    assert!(!command.contains("cd /Users/agent"), "{command}");
    assert!(!command.contains("rm -rf --"), "{command}");
    assert!(calls.iter().any(|call| {
        call.program == "ssh"
            && call
                .display()
                .contains("git update-ref -d refs/heads/hp/demo/t-0001-task")
            && call.display().contains("cd /home/agent/projects/herdr-ade")
    }));
    assert!(calls.iter().any(|call| {
        call.display()
            .contains("rm -rf -- /home/agent/build/lanes/demo-t-0001")
    }));
    let scratch = calls
        .iter()
        .find(|call| call.program == "ssh" && call.display().contains("scratch-t-0001"))
        .expect("scratch-session ssh call")
        .display();
    assert!(
        scratch.contains("PATH=/home/agent/.local/bin:/home/agent/.cargo/bin:/usr/local/bin:/usr/bin:/bin; export PATH"),
        "{scratch}"
    );
    assert!(scratch.contains("herdr session list --json"), "{scratch}");
    assert!(
        thread::load(&project, &t.id)
            .unwrap()
            .worktree_path
            .is_empty()
    );
}

#[test]
fn cancelling_other_ignored_data_keeps_a_box_worktree_but_removes_its_build() {
    let world = World::new();
    configure_test_box(&world);
    let project = world.project("demo", "a.sock");
    let t = world.thread(
        &project,
        Path::new("/home/agent/projects/herdr-ade/.worktrees/t-0001"),
        |thread| {
            thread.repo = "/Users/agent/projects/herdr-ade".into();
            thread.branch = "hp/demo/t-0001-task".into();
            thread.machine = "buildbox".into();
            thread.machine_id = "buildbox-id".into();
        },
    );
    thread::update(&project, &t.id, |t| t.merged_sha = "landed".into()).unwrap();
    record_stored_report(&project, &t.id);
    world.runner.on(
        "machine list --json",
        ok(r#"[{"id":"buildbox-id","label":"buildbox","target":"buildbox-pi","session":"default","enabled":true}]"#),
    );
    world.runner.on_fn(
        |cmd| cmd.program == "ssh",
        |cmd| {
            let line = cmd.display();
            if line.contains("__HERDR_WORKTREE_PRESENT__") {
                Ok(ok("__HERDR_WORKTREE_PRESENT__\n"))
            } else if line.contains("status --porcelain --ignored --untracked-files=all") {
                Ok(ok(
                    "!! .reports/t-0001.md\0!! runs/raw.bin\0\0__HERDR_NESTED_WORKTREES__\0",
                ))
            } else if line.contains("du -sk") {
                Ok(ok(
                    "4096\t/home/agent/projects/herdr-ade/.worktrees/t-0001/runs\n",
                ))
            } else {
                Ok(ok(""))
            }
        },
    );

    let outcome =
        threads::cancel(&world.ctx(), "demo", &t.id, "the work is no longer needed").unwrap();

    assert_eq!(outcome.worktree, "kept");
    let reason = outcome.worktree_reason.unwrap();
    assert!(reason.starts_with("ignored_data:") && reason.contains("runs"));
    assert!(!reason.contains(".reports"), "{reason}");
    assert_eq!(
        thread::load(&project, &t.id).unwrap().status,
        Status::Resolved
    );
    assert_eq!(world.runner.count("git worktree remove"), 0);
    assert_eq!(
        world
            .runner
            .count("rm -rf -- /home/agent/build/lanes/demo-t-0001"),
        1
    );
}

#[test]
fn a_failed_final_copy_blocks_resolve_unless_skipped() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let t = world.thread(&project, world.home.path(), |_| {});
    std::fs::create_dir_all(Path::new(&t.thread_dir).join("library")).unwrap();
    std::fs::write(Path::new(&t.thread_dir).join("library/out.txt"), "output").unwrap();
    world.runner.on("du -sk", ok("4\t/x\n"));
    world
        .runner
        .on("rsync", fail(12, "rsync: connection unexpectedly closed"));
    let ctx = world.ctx();

    assert!(threads::resolve(&ctx, "demo", "t-0001", &ResolveArgs::default()).is_err());
    assert_eq!(
        thread::load(&project, "t-0001").unwrap().status,
        Status::Open
    );
    let both = ResolveArgs {
        skip_copy: true,
        discard_uncopied: true,
        ..ResolveArgs::default()
    };
    assert!(threads::resolve(&ctx, "demo", "t-0001", &both).is_err());
    threads::resolve(
        &ctx,
        "demo",
        "t-0001",
        &ResolveArgs {
            skip_copy: true,
            ..ResolveArgs::default()
        },
    )
    .unwrap();
    assert_eq!(
        thread::load(&project, "t-0001").unwrap().status,
        Status::Resolved
    );
}

#[test]
fn linked_files_over_cap_or_missing_keep_the_worktree() {
    for missing in [false, true] {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |t| {
            t.repo = "/repo".into();
            t.branch = "lane".into();
            t.merged_sha = "landed".into();
        });
        world
            .runner
            .on("status --porcelain --ignored --untracked-files=all", ok(""));
        world.runner.on("worktree remove", ok(""));
        let dir = Path::new(&lane.thread_dir);
        std::fs::create_dir_all(dir).unwrap();
        let report = "![capture](figma/a.png)\n";
        std::fs::write(dir.join("report.md"), report).unwrap();
        if !missing {
            std::fs::create_dir_all(dir.join("figma")).unwrap();
            let file = std::fs::File::create(dir.join("figma/a.png")).unwrap();
            file.set_len(201 * 1024 * 1024).unwrap();
        }
        let hash = crate::events::store_artifact(&project, report.as_bytes()).unwrap();
        crate::events::seal_create_if_absent(
            &project,
            &Event {
                id: "t-0001-1-done".into(),
                op: "t-0001-1-done".into(),
                thread: lane.id.clone(),
                attempt: 1,
                recipient: Recipient::default(),
                created: project::now(),
                payload: EventPayload {
                    done: Some(DonePayload {
                        has_changes: None,
                        sha: "sealed".into(),
                        report_path: lane.report_path(),
                        artifact: hash,
                        attestation: None,
                        published_ref: None,
                    }),
                    ..EventPayload::default()
                },
            },
        )
        .unwrap();
        let outcome =
            threads::resolve(&world.ctx(), "demo", &lane.id, &ResolveArgs::default()).unwrap();
        let record = thread::load(&project, &lane.id).unwrap();
        if missing {
            assert_eq!(outcome.worktree, "kept", "{outcome:?}");
            assert_eq!(record.missing_report_links, vec!["figma/a.png"]);
            assert!(!outcome.copy_notes.is_empty());
            assert!(record.cleanup_pending);
            assert_eq!(world.runner.count("worktree remove"), 0);
        } else {
            assert_eq!(outcome.worktree, "kept");
            assert!(!outcome.copy_notes.is_empty());
            assert!(
                outcome
                    .worktree_reason
                    .unwrap()
                    .contains("linked_files_not_kept")
            );
            assert!(record.cleanup_pending);
            assert_eq!(world.runner.count("worktree remove"), 0);
        }
    }
}

#[test]
fn copy_overrides_do_not_discard_unsealed_or_unavailable_linked_reports() {
    for missing_seal in [false, true] {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |_| {});
        let dir = Path::new(&lane.thread_dir);
        std::fs::create_dir_all(dir).unwrap();
        let report = "![capture](figma/a.png)\n";
        std::fs::write(dir.join("report.md"), report).unwrap();
        if !missing_seal {
            std::fs::create_dir_all(dir.join("figma")).unwrap();
            std::fs::write(dir.join("figma/a.png"), "image").unwrap();
        }
        if missing_seal {
            let hash = crate::events::store_artifact(&project, report.as_bytes()).unwrap();
            crate::events::seal_create_if_absent(
                &project,
                &Event {
                    id: "t-0001-1-done".into(),
                    op: "t-0001-1-done".into(),
                    thread: lane.id.clone(),
                    attempt: 1,
                    recipient: Recipient::default(),
                    created: project::now(),
                    payload: EventPayload {
                        done: Some(DonePayload {
                            has_changes: None,
                            sha: "sealed".into(),
                            report_path: lane.report_path(),
                            artifact: hash.clone(),
                            attestation: None,
                            published_ref: None,
                        }),
                        ..EventPayload::default()
                    },
                },
            )
            .unwrap();
            std::fs::remove_file(crate::events::artifact_path(&project, &hash)).unwrap();
        }
        let outcome = threads::resolve(
            &world.ctx(),
            "demo",
            &lane.id,
            &ResolveArgs {
                skip_copy: true,
                ..ResolveArgs::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.worktree, "kept");
        assert!(
            outcome
                .worktree_reason
                .unwrap()
                .contains("linked_files_not_kept")
        );
        assert!(thread::load(&project, &lane.id).unwrap().cleanup_pending);
        assert_eq!(world.runner.count("worktree remove"), 0);
    }
}

#[test]
fn a_no_change_lane_closes_with_its_sealed_report_artifact() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let cwd = world.home.path().join("lane");
    std::fs::create_dir_all(&cwd).unwrap();
    let lane = world.thread(&project, &cwd, |t| {
        t.kind = Kind::Tab;
        t.worktree_path.clear();
        t.base = "brief-sha".into();
    });
    std::fs::create_dir_all(&lane.thread_dir).unwrap();
    let report = "report only ![a](figma/a.png) [notes](notes/b.md) <img src=figma/x.png> <source srcset=\"data:image/png;base64,AAAA 1x, figma/small.webp 2x, figma/large.webp 3x\">\n";
    for (name, bytes) in [
        ("figma/a.png", "png"),
        ("notes/b.md", "notes"),
        ("figma/x.png", "image"),
        ("figma/small.webp", "small"),
        ("figma/large.webp", "large"),
    ] {
        let path = Path::new(&lane.thread_dir).join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    std::fs::write(Path::new(&lane.thread_dir).join("report.md"), report).unwrap();
    let artifact = crate::events::store_artifact(&project, report.as_bytes()).unwrap();
    let event = Event {
        id: "t-0001-1-done".into(),
        op: "t-0001-1-done".into(),
        thread: lane.id.clone(),
        attempt: 1,
        recipient: Recipient::default(),
        created: project::now(),
        payload: EventPayload {
            done: Some(DonePayload {
                has_changes: Some(false),
                sha: "brief-sha".into(),
                report_path: lane.report_path(),
                artifact: artifact.clone(),
                attestation: None,
                published_ref: None,
            }),
            ..EventPayload::default()
        },
    };
    crate::events::seal_create_if_absent(&project, &event).unwrap();
    *world.panes.borrow_mut() = format!(
        "[{},{}]",
        world.coordinator_pane(&project),
        pane_json("w2", "w2:t1", "w2:p1", &cwd.to_string_lossy())
    );
    *world.agents.borrow_mut() = format!(
        "[{}]",
        agent_json(
            "w2",
            "w2:t1",
            "w2:p1",
            &cwd.to_string_lossy(),
            &lane.agent_name,
            "idle"
        )
    );

    thread::update(&project, &lane.id, |record| {
        record.follow_ups.push(thread::FollowUp {
            attempt: 1,
            text: "Check the report".into(),
            ..Default::default()
        });
    })
    .unwrap();
    threads::resolve_report_only(&world.ctx(), &project);
    assert_eq!(
        thread::load(&project, &lane.id).unwrap().status,
        Status::Open
    );

    thread::update(&project, &lane.id, |record| {
        record.follow_ups[0].state = thread::FollowUpState::Delivered;
        record.follow_ups[0].after_seal = event.id.clone();
    })
    .unwrap();
    threads::resolve_report_only(&world.ctx(), &project);
    assert_eq!(
        thread::load(&project, &lane.id).unwrap().status,
        Status::Open
    );

    let mut second = event.clone();
    second.id = "t-0001-1-done-2".into();
    second.op = second.id.clone();
    crate::events::seal_create_if_absent(&project, &second).unwrap();
    threads::resolve_report_only(&world.ctx(), &project);

    let closed = thread::load(&project, &lane.id).unwrap();
    assert_eq!(closed.status, Status::Resolved);
    assert_eq!(closed.resolved_reason, "report-only");
    assert!(!thread::home_report_path(&project, &lane.id).exists());
    assert_eq!(
        std::fs::read(crate::events::artifact_path(&project, &artifact)).unwrap(),
        report.as_bytes()
    );
    let stored = thread::sealed_report_path(&project, &closed).unwrap();
    let rewritten = std::fs::read_to_string(stored.clone()).unwrap();
    for (name, bytes) in [
        ("figma/a.png", "png"),
        ("notes/b.md", "notes"),
        ("figma/x.png", "image"),
        ("figma/small.webp", "small"),
        ("figma/large.webp", "large"),
    ] {
        let hash = thread::sha256_hex(bytes.as_bytes());
        assert!(!rewritten.contains(name));
        assert!(rewritten.contains(&hash));
        assert_eq!(
            std::fs::read(stored.parent().unwrap().join(hash)).unwrap(),
            bytes.as_bytes()
        );
    }
    assert_eq!(world.runner.count("workspace close w2"), 1);
}

#[test]
fn thread_start_is_refused_when_paused() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    project.set_status(project::Status::Paused).unwrap();
    let args = StartArgs {
        title: "x".into(),
        repo: None,
        machine: None,
        base: None,
        task: "t".into(),
        plain: "The lane does the work.".into(),
        workflow: None,
        recipe: None,
        task_id: String::new(),
        review_id: String::new(),
    };
    let error = threads::start(&world.ctx(), "demo", args)
        .unwrap_err()
        .to_string();
    assert!(error.contains("paused"), "{error}");
    assert!(thread::list(&project).is_empty());
}

#[test]
fn unreachable_session_prints_records_without_treating_panes_as_gone() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.thread(&project, world.home.path(), |t| {
        t.last_group = "working".into()
    });
    let broken = World {
        runner: FakeRunner::new(),
        ..world
    };
    broken
        .runner
        .on("agent list", fail(1, "connection refused"));
    let rows = threads::rows(&broken.ctx(), &project);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].group, thread::Group::Working);
}

// ------------------------------------------------------------------ stage 5

use crate::inbox;
use crate::steps::Memory;

fn items_of(project: &Project, kind: &str) -> Vec<inbox::Item> {
    inbox::unhandled(project)
        .into_iter()
        .filter(|i| i.kind == kind)
        .collect()
}

#[test]
fn a_restarted_session_gives_one_session_item_not_one_per_thread() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.thread(&project, world.home.path(), |t| {
        t.last_group = "working".into()
    });
    // The list call succeeds and every recorded pane (coordinator + thread) is gone.
    let ctx = world.ctx();
    ticker::tick_project(&ctx, &project).unwrap();
    ticker::tick_project(&ctx, &project).unwrap();
    assert_eq!(items_of(&project, "session").len(), 1);
    assert!(items_of(&project, "thread-state").is_empty());
}

#[test]
fn an_unreachable_session_writes_nothing() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.thread(&project, world.home.path(), |t| {
        t.last_group = "working".into()
    });
    let broken = World {
        runner: FakeRunner::new(),
        ..world
    };
    broken
        .runner
        .on("agent list", fail(1, "connection refused"));
    assert!(!ticker::tick_project(&broken.ctx(), &project).unwrap());
    assert!(inbox::unhandled(&project).is_empty());
    assert_eq!(
        thread::load(&project, "t-0001").unwrap().last_group,
        "working"
    );
}

// ------------------------------------------------------------------ stage 6

fn remote_world() -> (World, Project) {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    world.thread(&project, Path::new("/home/me/wt"), |t| {
        t.machine = "box".into();
        t.last_group = "working".into();
        t.last_state = "working".into();
        t.last_state_change = "2026-01-01T00:00:00Z".into();
    });
    *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
    world.runner.on(
        "machine list --json",
        ok(r#"[{"id":"1","label":"box","target":"me@box","session":"default","enabled":true}]"#),
    );
    (world, project)
}

#[test]
fn forty_minute_sleep_defers_dark_wakes_and_imports_seals_before_resuming_starts() {
    let (world, project) = remote_world();
    let now = jiff::Timestamp::now().as_second();
    let start = jiff::Timestamp::from_second(now - 2400)
        .unwrap()
        .to_string();
    thread::update(&project, "t-0001", |t| {
        t.launch.kind = "claude".into();
        t.launch.ready_timeout_ms = 90_000;
        t.provider_wait_started = start.clone();
        t.error = "waiting for provider: old unreachable probe".into();
        t.created = start.clone();
        t.progress_since = start.clone();
        t.no_commit_since = start.clone();
    })
    .unwrap();
    let local = thread::allocate(&project, |t| {
        t.status = Status::Starting;
        t.created = start.clone();
        t.startup_wait_started = start.clone();
        t.launch.ready_timeout_ms = 90_000;
    })
    .unwrap();
    let review = crate::review::Review {
        id: "review-1".into(),
        repo: "/repo".into(),
        integration: String::new(),
        base: String::new(),
        candidate_branch: String::new(),
        members: vec![],
        gates: vec![],
        gates_note: String::new(),
        selected_gates: vec![],
        reviewer: Some("t-0001".into()),
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
        close: false,
        prune: false,
        attention: String::new(),
        no_verdict_since: start.clone(),
        notices: vec![],
    };
    crate::review::save(&project, &review).unwrap();
    let review_path = crate::review::path(&project, "review-1");
    let review_before = std::fs::read(&review_path).unwrap();
    let remote_before = toml::to_string(&thread::load(&project, "t-0001").unwrap()).unwrap();
    let local_before = toml::to_string(&local).unwrap();
    let offline = Rc::new(RefCell::new(true));
    let flag = offline.clone();
    let report = b"finished during sleep\n";
    let artifact = thread::sha256_hex(report);
    let event = Event {
        id: "t-0001-1-1".into(),
        op: "t-0001-1-1".into(),
        thread: "t-0001".into(),
        attempt: 1,
        recipient: Recipient {
            pane: "w1:p1".into(),
            coordinator_attempt: 1,
        },
        created: start.clone(),
        payload: EventPayload {
            done: Some(DonePayload {
                has_changes: Some(false),
                sha: "abc".into(),
                report_path: ".reports/t-0001.md".into(),
                artifact: artifact.clone(),
                attestation: None,
                published_ref: None,
            }),
            waiting: None,
            failed: None,
        },
    };
    let bytes = crate::events::bytes(&event).unwrap();
    let hash = thread::sha256_hex(&bytes);
    let manifest = format!(
        "boot\tboot-1\nagents\t{{\"result\":{{\"agents\":[]}}}}\npanes\t{{\"result\":{{\"panes\":[]}}}}\n\
         event\tdemo\tt-0001-1-1\t/box/events/t-0001-1-1.toml\t{hash}\t/box/artifacts/{artifact}\t{artifact}\n\
         receipt\tdemo\tt-0001-1-1\t{hash}\t{artifact}\n"
    );
    world.runner.on_fn(
        |cmd| cmd.program == "ssh",
        move |cmd| {
            Ok(if *flag.borrow() {
                fail(255, "ssh: connect to host box: Operation timed out")
            } else if cmd
                .args
                .last()
                .is_some_and(|script| script.contains("disk_free_kb"))
            {
                ok("disk_free_kb\t99999999\nOK\n")
            } else {
                ok(&manifest)
            })
        },
    );
    world.runner.on_fn(
        |cmd| cmd.program == "scp",
        move |cmd| {
            let dir = PathBuf::from(cmd.args.last().unwrap().trim_end_matches('/'));
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("t-0001-1-1.toml"), &bytes)?;
            std::fs::write(dir.join(&artifact), report)?;
            Ok(ok(""))
        },
    );
    let ctx = world.ctx();
    crate::awake::set_sample(Some((now - 2400, 100)));
    drop(crate::awake::enter(&ctx.root, true).unwrap());
    let mut memory = Memory::new(&ctx);
    for n in 1..=7 {
        crate::awake::set_sample(Some((now - 2400 + n * 300, 100 + n as u64 * 5)));
        assert!(ticker::tick_for_test(&ctx, &mut memory));
        assert_eq!(
            toml::to_string(&thread::load(&project, "t-0001").unwrap()).unwrap(),
            remote_before
        );
        assert_eq!(
            toml::to_string(&thread::load(&project, &local.id).unwrap()).unwrap(),
            local_before
        );
        assert_eq!(std::fs::read(&review_path).unwrap(), review_before);
        assert!(crate::events::list(&project).is_empty());
        assert!(inbox::unhandled(&project).is_empty());
        // Exercise persistence, not only the process-local clock/tracker.
        memory = Memory::new(&ctx);
    }
    *offline.borrow_mut() = false;
    // Wake forces a courier poll even if its previous backoff is still due.
    memory
        .machines
        .entry("box".into())
        .or_default()
        .skip_until_tick = 100;
    crate::awake::set_sample(Some((now + 15, 155)));
    ticker::tick_for_test(&ctx, &mut memory);
    assert!(crate::events::load(&project, "t-0001-1-1").is_ok());
    assert!(
        thread::load(&project, "t-0001")
            .unwrap()
            .provider_wait_started
            .is_empty()
    );
    assert_eq!(
        thread::load(&project, &local.id).unwrap().status,
        Status::Starting
    );
    let (_scope, _) = crate::awake::enter(&ctx.root, false).unwrap();
    for timer in [
        &start,
        &review.no_verdict_since,
        &local.startup_wait_started,
    ] {
        assert_eq!(
            thread::seconds_since(timer, jiff::Timestamp::from_second(now + 15).unwrap()),
            55
        );
    }
    crate::awake::set_sample(None);
    let calls = world.runner.calls.borrow();
    let import = calls.iter().position(|cmd| cmd.program == "scp").unwrap();
    let ready = calls
        .iter()
        .position(|cmd| {
            cmd.program == "ssh"
                && cmd
                    .args
                    .last()
                    .is_some_and(|script| script.contains("disk_free_kb"))
        })
        .unwrap();
    assert!(
        import < ready,
        "seals must be imported before waiting starts and timers"
    );
}

#[test]
fn unreachable_box_does_not_freeze_another_projects_due_work_during_backoff() {
    let (world, offline) = remote_world();
    let before = toml::to_string(&thread::load(&offline, "t-0001").unwrap()).unwrap();
    world.runner.on_fn(
        |cmd| cmd.program == "ssh",
        |_| Ok(fail(255, "ssh: connect to host box: Operation timed out")),
    );
    let other = world.project("other", "b.sock");
    let coordinator = other.coordinator().unwrap();
    *world.agents.borrow_mut() = format!(
        "[{}]",
        agent_json(
            "w1",
            "w1:t1",
            "w1:p1",
            &coordinator.cwd,
            &coordinator.agent_name,
            "idle"
        )
    );
    *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&other));
    world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
    world.runner.on("pane read", ok("❯ "));
    let ctx = world.ctx();
    let mut memory = Memory::new(&ctx);
    for _ in 0..2 {
        other
            .update_coordinator(|c| {
                c.prime_pending = true;
                c.prime_sent = false;
            })
            .unwrap();
        assert!(ticker::tick_for_test(&ctx, &mut memory));
        assert!(other.coordinator().unwrap().prime_sent);
        assert_eq!(
            toml::to_string(&thread::load(&offline, "t-0001").unwrap()).unwrap(),
            before
        );
    }
    assert_eq!(world.runner.count("ssh"), 1);
}

#[test]
fn a_saved_machine_lookup_fault_never_becomes_a_lost_connection() {
    let (world, project) = remote_world();
    thread::update(&project, "t-0001", |t| {
        t.machine_id = "1".into();
        t.failure_class = crate::contracts::FailureClass::LostConnection;
        t.last_failure = "the old lookup fault was misclassified".into();
        t.error = t.last_failure.clone();
    })
    .unwrap();
    std::fs::write(
        world.home.path().join("cfg/config.toml"),
        "[routing]\ndefault = \"test_claude\"\nretries = 1\n\n[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n",
    )
    .unwrap();
    let ctx = world.ctx();
    let mut memory = Memory::new(&ctx);
    memory.outage_secs = 0;
    memory.tick = 1;

    let _ = ticker::tick_project_with(&ctx, &project, &mut memory);

    let lane = thread::load(&project, "t-0001").unwrap();
    assert_eq!(lane.failure_class, crate::contracts::FailureClass::Unknown);
    assert!(lane.last_failure.is_empty());
    assert!(lane.error.is_empty());
    assert!(items_of(&project, "outage").is_empty());
    assert_eq!(world.runner.count("ssh"), 0);
    assert!(!memory.machine_views.contains_key("1"));
}

#[test]
fn a_successful_courier_clears_a_persisted_lost_connection_after_restart() {
    let (world, project) = remote_world();
    thread::update(&project, "t-0001", |t| {
        t.machine_id = "1".into();
        t.failure_class = crate::contracts::FailureClass::LostConnection;
        t.last_failure = "the link was unreachable".into();
        t.error = t.last_failure.clone();
    })
    .unwrap();
    world
        .runner
        .on("ssh", ok("boot\tboot-1\nagents\t-\npanes\t-\n"));
    let ctx = world.ctx();
    let mut fresh_memory = Memory::new(&ctx);
    fresh_memory.tick = 1;

    ticker::tick_project_with(&ctx, &project, &mut fresh_memory).unwrap();

    let lane = thread::load(&project, "t-0001").unwrap();
    assert_eq!(lane.failure_class, crate::contracts::FailureClass::Unknown);
    assert!(lane.last_failure.is_empty());
    assert!(lane.error.is_empty());
}

#[test]
fn a_thread_without_any_listed_repo_is_refused() {
    let world = World::new();
    world.project("demo", "a.sock");
    let args = StartArgs {
        title: "x".into(),
        repo: None,
        machine: Some("box".into()),
        base: None,
        task: "t".into(),
        plain: "The lane does the work.".into(),
        workflow: None,
        recipe: None,
        task_id: String::new(),
        review_id: String::new(),
    };
    assert!(
        threads::start(&world.ctx(), "demo", args)
            .unwrap_err()
            .to_string()
            .contains("repo_required")
    );
}

#[test]
fn explicit_lane_recipe_still_checks_validity() {
    let world = World::new();
    let config = world.home.path().join("cfg/config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(
        "\n[recipes.disabled_choice]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nenabled = false\nplain = \"disabled\"\n",
    );
    std::fs::write(&config, text).unwrap();
    let project = world.project("demo", "a.sock");
    let validate = |recipe, task| {
        crate::launch::validate_explicit_recipe(
            &world.ctx(),
            &project,
            "job-1",
            task,
            "lane",
            recipe,
        )
        .unwrap_err()
        .to_string()
    };
    let unknown = validate("not_configured", "Do the work.");
    assert!(unknown.contains("routing_recipe_unknown"), "{unknown}");
    assert!(validate("disabled_choice", "Do the work.").contains("routing_recipe_disabled"));
    assert!(
        validate(
            "test_claude",
            "+++\ncapability = \"not-declared\"\n+++\nDo the work."
        )
        .contains("routing_capability_missing")
    );
}

#[test]
fn typed_provider_errors_reach_recovery_and_retry_the_same_recipe() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    for (text, provider_kind) in [
        ("openai-codex unreachable: fetch failed", "unreachable"),
        (
            "openai-codex error: Codex error: Our servers are currently overloaded. Please try again later.",
            "error",
        ),
    ] {
        let launch = crate::launch::resolve_launch(
            &world.ctx(),
            &project,
            &crate::launch::ResolveInput {
                task: "Do the work.",
                workflow: "lane",
                ..Default::default()
            },
        )
        .unwrap();
        let lane = world.thread(&project, world.home.path(), |thread| {
            thread.attempt = 1;
            thread.launch = launch;
        });
        std::fs::write(thread::task_path(&project, &lane.id), "Do the work.").unwrap();
        let event_id = format!("{}-1-1", lane.id);
        let event = crate::contracts::Event {
            id: event_id.clone(),
            op: event_id,
            thread: lane.id.clone(),
            attempt: 1,
            recipient: crate::contracts::Recipient::default(),
            created: project::now(),
            payload: crate::contracts::EventPayload {
                failed: Some(crate::contracts::WaitingPayload {
                    text: text.into(),
                    class: crate::contracts::FailureClass::Provider,
                    provider_kind: Some(provider_kind.into()),
                }),
                ..Default::default()
            },
        };

        crate::recovery::consume(&world.ctx(), &project, &event).unwrap();

        let retry = thread::load(&project, &lane.id).unwrap();
        assert_eq!(
            retry.failure_class,
            crate::contracts::FailureClass::Provider
        );
        assert_eq!(retry.provider_failure_kind.as_deref(), Some(provider_kind));
        assert_eq!(retry.attempt, 2);
        assert_eq!(retry.launch.recipe_id, "test_claude");
        assert_eq!(retry.launch.same_recipe_retries, 1);
        assert!(retry.recovery_pending);
    }
}

#[test]
fn once_only_failure_waits_until_a_reasoned_coordinator_retry() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let task = "+++\nonce = true\n+++\nRelease exactly once.";
    let brief_hash = thread::store_artifact(&project, task.as_bytes()).unwrap();
    let mut first = crate::launch::resolve_launch(
        &world.ctx(),
        &project,
        &crate::launch::ResolveInput {
            task,
            workflow: "lane",
            ..Default::default()
        },
    )
    .unwrap();
    first.brief_hash = brief_hash;
    let lane = world.thread(&project, world.home.path(), |thread| {
        thread.attempt = 1;
        thread.launch = first;
        thread.launch_attempts = 1;
    });
    std::fs::write(thread::task_path(&project, &lane.id), task).unwrap();
    let event = crate::contracts::Event {
        id: "process-gone-1".into(),
        op: "process-gone-1".into(),
        thread: lane.id.clone(),
        attempt: 1,
        recipient: crate::contracts::Recipient::default(),
        created: project::now(),
        payload: crate::contracts::EventPayload {
            failed: Some(crate::contracts::WaitingPayload {
                text: "the process vanished".into(),
                class: crate::contracts::FailureClass::ProcessGone,
                provider_kind: None,
            }),
            ..Default::default()
        },
    };
    crate::recovery::consume(&world.ctx(), &project, &event).unwrap();
    let waiting = thread::load(&project, &lane.id).unwrap();
    assert_eq!(waiting.attempt, 1);
    assert!(!waiting.recovery_pending);
    assert!(waiting.error.starts_with("WAITING: recovery_exhausted:"));
    assert!(
        !std::fs::read_to_string(project.state_dir().join("dispatch.jsonl"))
            .unwrap()
            .contains("\"kind\":\"placement\"")
    );

    let cwd = world.home.path().to_string_lossy().into_owned();
    *world.panes.borrow_mut() = format!(
        "[{},{}]",
        world.coordinator_pane(&project),
        pane_json("w2", "w2:t1", "w2:p1", &cwd)
    );
    world
        .runner
        .on("rev-parse --git-path", fail(1, "not a repo"));
    world.runner.on("tab close", ok(r#"{"result":{}}"#));
    world.runner.on(
        "tab create",
        ok(&format!(
            r#"{{"result":{{"root_pane":{{"workspace_id":"w1","tab_id":"w1:t3","pane_id":"w1:p3","cwd":"{cwd}"}}}}}}"#
        )),
    );
    let reason = "the coordinator verified the release did not occur";
    threads::retry(&world.ctx(), "demo", &lane.id, reason).unwrap();
    assert_eq!(thread::load(&project, &lane.id).unwrap().attempt, 2);
    let dispatch = std::fs::read_to_string(project.state_dir().join("dispatch.jsonl")).unwrap();
    let decision: serde_json::Value =
        serde_json::from_str(dispatch.lines().last().unwrap()).unwrap();
    assert_eq!(decision["kind"], "coordinator-retry");
    assert_eq!(decision["failure"], reason);

    // Without the marker the same failed event still schedules the next attempt.
    let normal = "Release with retries.";
    let first = crate::launch::resolve_launch(
        &world.ctx(),
        &project,
        &crate::launch::ResolveInput {
            task: normal,
            workflow: "lane",
            ..Default::default()
        },
    )
    .unwrap();
    let lane = world.thread(&project, world.home.path(), |thread| {
        thread.attempt = 1;
        thread.launch = first;
    });
    std::fs::write(thread::task_path(&project, &lane.id), normal).unwrap();
    let mut event = event;
    event.id = "process-gone-2".into();
    event.op = event.id.clone();
    event.thread = lane.id.clone();
    crate::recovery::consume(&world.ctx(), &project, &event).unwrap();
    let retried = thread::load(&project, &lane.id).unwrap();
    assert_eq!(retried.attempt, 2);
    assert!(retried.recovery_pending);
}

#[test]
fn provider_retries_do_not_consume_failed_work_retries() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let input = |previous, failure| crate::launch::ResolveInput {
        task: "Do the work.",
        workflow: "lane",
        previous,
        failure,
        source_truncation: None,
        ..Default::default()
    };
    let first = crate::launch::resolve_launch(&world.ctx(), &project, &input(None, None)).unwrap();
    let provider = crate::launch::resolve_failure(
        &world.ctx(),
        &project,
        &input(Some(&first), Some("fetch failed")),
        crate::contracts::FailureClass::Provider,
    )
    .unwrap();
    assert_eq!(provider.recipe_id, "test_claude");
    assert_eq!(provider.work_retries, 0);
    assert_eq!(provider.same_recipe_retries, 1);

    let first_work = crate::launch::resolve_failure(
        &world.ctx(),
        &project,
        &input(Some(&provider), Some("the approach failed")),
        crate::contracts::FailureClass::WorkFailed,
    )
    .unwrap();
    assert_eq!(first_work.recipe_id, "test_claude");
    assert_eq!(first_work.work_retries, 1);
    assert_eq!(first_work.same_recipe_retries, 0);
    let error = crate::launch::resolve_failure(
        &world.ctx(),
        &project,
        &input(Some(&first_work), Some("the retry failed")),
        crate::contracts::FailureClass::WorkFailed,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("recovery_exhausted"));
}

#[test]
fn provider_readiness_and_a_gone_process_schedule_same_recipe_restarts() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    let launch = crate::launch::resolve_launch(
        &world.ctx(),
        &project,
        &crate::launch::ResolveInput {
            task: "Do the work.",
            workflow: "lane",
            ..Default::default()
        },
    )
    .unwrap();
    let lane = world.thread(&project, world.home.path(), |thread| {
        thread.attempt = 1;
        thread.launch = launch;
        thread.launch_attempts = 1;
    });
    std::fs::write(thread::task_path(&project, &lane.id), "Do the work.").unwrap();
    threads::fail_start(
        &world.ctx(),
        &project,
        &lane.id,
        "the pane or agent is gone without a report",
        crate::contracts::FailureClass::ProcessGone,
        true,
    )
    .unwrap();
    let restarted = thread::load(&project, &lane.id).unwrap();
    assert_eq!(
        restarted.failure_class,
        crate::contracts::FailureClass::ProcessGone
    );
    assert_eq!(restarted.attempt, 2);
    assert_eq!(restarted.launch.recipe_id, "test_claude");
    assert_eq!(restarted.launch.work_retries, 0);
    assert_eq!(restarted.launch.same_recipe_retries, 1);
    assert!(restarted.recovery_pending);

    let provider_launch = crate::launch::resolve_launch(
        &world.ctx(),
        &project,
        &crate::launch::ResolveInput {
            task: "Check the provider.",
            workflow: "lane",
            ..Default::default()
        },
    )
    .unwrap();
    let provider_lane = world.thread(&project, world.home.path(), |thread| {
        thread.attempt = 1;
        thread.launch = provider_launch;
        thread.launch_attempts = 1;
    });
    std::fs::write(
        thread::task_path(&project, &provider_lane.id),
        "Check the provider.",
    )
    .unwrap();
    threads::fail_start(
        &world.ctx(),
        &project,
        &provider_lane.id,
        "pi_not_ready: the provider probe timed out",
        crate::contracts::FailureClass::Provider,
        true,
    )
    .unwrap();
    let provider_retry = thread::load(&project, &provider_lane.id).unwrap();
    assert_eq!(
        provider_retry.failure_class,
        crate::contracts::FailureClass::Provider
    );
    assert_eq!(provider_retry.launch.recipe_id, "test_claude");
    assert_eq!(provider_retry.launch.work_retries, 0);
    assert_eq!(provider_retry.launch.same_recipe_retries, 1);
}

#[test]
fn a_project_recipe_is_stored_and_used_again_for_a_coordinator_relaunch() {
    let world = World::new();
    let config = world.home.path().join("cfg/config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(
        "\n[recipes.chosen_agy]\nkind = \"agy\"\nargs = [\"--dangerously-skip-permissions\", \"--model\", \"chosen\"]\nplain = \"Rolf's chosen coordinator\"\n",
    );
    std::fs::write(&config, text).unwrap();
    let authority = project::create(&world.root, "authority", "", vec![]).unwrap();
    crate::prompt::record_test_request(
        &authority,
        "q-choice",
        "Use the chosen coordinator recipe for demo.",
    )
    .unwrap();
    let project = world.project("demo", "a.sock");
    *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
    world.runner.on(
        "agent start hp-demo-coordinator",
        fail(
            1,
            r#"{"error":{"code":"timeout","message":"still starting"}}"#,
        ),
    );

    let mut options = crate::coordinator::OpenOptions {
        session: crate::paths::SessionFlags {
            session: None,
            socket: Some(world.home.path().join("a.sock")),
        },
        reprime: false,
        rebind: false,
        recipe: Some("chosen_agy".into()),
        recipe_basis: Some("request:authority/q-choice".into()),
    };
    options.recipe_basis = Some("request:authority/q-missing".into());
    let error = crate::coordinator::open(&world.ctx(), "demo", &options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no request"), "{error}");

    options.recipe_basis = Some("request:authority/q-choice".into());
    crate::coordinator::open(&world.ctx(), "demo", &options).unwrap();
    let first = project.coordinator().unwrap();
    assert_eq!(first.launch.recipe_id, "chosen_agy");
    assert_eq!(first.launch.routing_rule, "project");
    assert_eq!(first.launch.recipe_basis, "request:authority/q-choice");
    assert_eq!(first.launch.recipe_request, "request:authority/q-choice");

    std::fs::remove_file(world.home.path().join("a.sock")).unwrap();
    world.runner.on(
        "workspace create",
        ok(r#"{"result":{"root_pane":{"workspace_id":"w2","tab_id":"w2:t1","pane_id":"w2:p1"}}}"#),
    );
    options.recipe = None;
    options.recipe_basis = None;
    options.rebind = true;
    options.session.socket = Some(world.home.path().join("b.sock"));
    crate::coordinator::open(&world.ctx(), "demo", &options).unwrap();
    let relaunched = project.coordinator().unwrap();
    assert_eq!(relaunched.launch.recipe_id, "chosen_agy");
    assert_eq!(relaunched.launch.args, first.launch.args);
    assert_eq!(relaunched.launch.recipe_basis, first.launch.recipe_basis);
    let starts: Vec<_> = world
        .runner
        .calls
        .borrow()
        .iter()
        .filter(|call| call.display().contains("agent start hp-demo-coordinator"))
        .map(Cmd::display)
        .collect();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert!(
        starts
            .iter()
            .all(|start| start.contains("--kind agy") && start.contains("--model chosen")),
        "{starts:?}"
    );
}

#[test]
fn open_reopens_a_closed_coordinator_and_a_new_message_requests_reopen() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    project
        .update_coordinator(|c| c.closed_by_rolf_at = project::now())
        .unwrap();
    world.runner.on(
        "workspace create",
        ok(r#"{"result":{"root_pane":{"workspace_id":"w2","tab_id":"w2:t1","pane_id":"w2:p1"}}}"#),
    );
    world.runner.on(
        "agent start hp-demo-coordinator",
        fail(
            1,
            r#"{"error":{"code":"timeout","message":"still starting"}}"#,
        ),
    );
    let options = coordinator::OpenOptions {
        session: crate::paths::SessionFlags {
            session: None,
            socket: Some(world.home.path().join("a.sock")),
        },
        reprime: false,
        rebind: false,
        recipe: None,
        recipe_basis: None,
    };
    coordinator::open(&world.ctx(), "demo", &options).unwrap();
    let record = project.coordinator().unwrap();
    assert_eq!(record.pane_id, "w2:p1");
    assert!(record.closed_by_rolf_at.is_empty());
    project
        .update_coordinator(|c| c.closed_by_rolf_at = project::now())
        .unwrap();
    crate::prompt::record_test_request(&project, "q-1", "Continue").unwrap();
    let record = project.coordinator().unwrap();
    assert!(record.closed_by_rolf_at.is_empty());
    assert!(record.reopen_requested);
}

#[test]
fn ticker_does_not_relaunch_a_gone_coordinator() {
    let world = World::new();
    let project = world.project("demo", "a.sock");
    project
        .update_coordinator(|c| {
            c.launch.kind = "claude".into();
            c.launch.recipe_id = "chosen".into();
            c.launch.machine = "oci".into();
            c.launch.args = vec!["--model".into(), "recorded".into()];
        })
        .unwrap();
    let replacement = world.home.path().join("new.sock");
    std::fs::write(&replacement, b"").unwrap();
    std::fs::rename(replacement, project.coordinator().unwrap().socket).unwrap();
    world.runner.on(
        "workspace create",
        ok(r#"{"result":{"root_pane":{"workspace_id":"w2","tab_id":"w2:t1","pane_id":"w2:p1"}}}"#),
    );
    world.runner.on(
        "agent start hp-demo-coordinator",
        fail(
            1,
            r#"{"error":{"code":"timeout","message":"still starting"}}"#,
        ),
    );
    ticker::tick_project(&world.ctx(), &project).unwrap();
    assert_eq!(project.coordinator().unwrap().launch.recipe_id, "chosen");
    assert_eq!(project.coordinator().unwrap().launch.machine, "oci");
    assert_eq!(
        project.coordinator().unwrap().launch.args,
        ["--model", "recorded"]
    );
    let starts = world
        .runner
        .calls
        .borrow()
        .iter()
        .filter(|call| call.display().contains("agent start hp-demo-coordinator"))
        .map(Cmd::display)
        .collect::<Vec<_>>();
    assert!(starts.is_empty(), "{starts:?}");
    ticker::tick_project(&world.ctx(), &project).unwrap();
    assert_eq!(world.runner.count("agent start hp-demo-coordinator"), 0);
}

// ---------------------------------------------------------- harness (t-0054)

fn harness_repo(home: &Path, name: &str, package: &str) -> String {
    let repo = home.join(name);
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        format!("[package]\nname = \"{package}\"\nversion = \"0.1.0\"\n"),
    )
    .unwrap();
    if package == "herdr-ade" {
        for (path, text) in [
            (
                ".claude-plugin/plugin.json",
                include_str!("../mods/coordinator-handoff/.claude-plugin/plugin.json"),
            ),
            (
                "hooks/hooks.json",
                include_str!("../mods/coordinator-handoff/hooks/hooks.json"),
            ),
            (
                "hooks/register.ts",
                include_str!("../mods/coordinator-handoff/hooks/register.ts"),
            ),
        ] {
            let file = repo.join("mods/coordinator-handoff").join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
    }
    std::fs::canonicalize(&repo)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn write_harness_config(world: &World, repos: &[(&str, &str)]) {
    world.runner.on_fn(
        |cmd| cmd.program == "git" && cmd.display().contains("rev-parse HEAD"),
        |_| Ok(ok("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n")),
    );
    world.runner.on_fn(
        |cmd| cmd.program == "git" && cmd.display().contains("status --porcelain"),
        |_| Ok(ok("")),
    );
    let dir = world.home.path().join("cfg");
    std::fs::create_dir_all(&dir).unwrap();
    let rows: Vec<String> = repos
        .iter()
        .map(|(path, box_path)| format!("  {{ path = \"{path}\", box_path = \"{box_path}\" }},"))
        .collect();
    std::fs::write(
        dir.join("config.toml"),
        format!(
            "[harness]\nrepos = [\n{}\n]\n[dispatch]\nmachine = \"buildbox\"\n",
            rows.join("\n")
        ),
    )
    .unwrap();
    std::fs::write(dir.join("RULES.md"), "# Lane rules\n").unwrap();
    world.runner.on("herdr-pi refresh-guard", ok(""));
}

#[test]
fn harness_install_runs_the_box_steps_only_when_buildbox_is_saved() {
    let plugin = |world: &World| {
        (
            harness_repo(world.home.path(), "plugin", "herdr-ade"),
            harness_repo(world.home.path(), "fork", "herdr"),
        )
    };

    let with_box = World::new();
    let (p, f) = plugin(&with_box);
    write_harness_config(
        &with_box,
        &[
            (&p, "/home/agent/projects/herdr-ade"),
            (&f, "/home/agent/projects/herdr"),
        ],
    );
    configure_test_box(&with_box);
    with_box.runner.on_fn(
        |cmd| {
            cmd.program == "ssh"
                && cmd
                    .args
                    .last()
                    .is_some_and(|arg| arg.contains("git fetch --quiet"))
        },
        |_| Ok(ok("HERDR_ADE_INSTALLED_HEAD=abc123\n")),
    );
    with_box.runner.on("cargo build", ok(""));
    with_box.runner.on("cp ", ok(""));
    with_box.runner.on("mv -f", ok(""));
    with_box.runner.on_fn(
        |cmd| {
            cmd.program == "ssh"
                && cmd
                    .args
                    .last()
                    .is_some_and(|arg| arg.contains("ticker start"))
        },
        |_| {
            Ok(ok(&format!(
                "HERDR_ADE_BOX_BINARY=herdr-ade {}\nHERDR_ADE_BOX_TICKER=42:{}\n",
                crate::VERSION,
                crate::VERSION
            )))
        },
    );
    with_box.runner.on("--version", ok("installed version\n"));
    with_box.runner.on("ssh", ok(""));
    with_box.runner.on(
        "machine list --json",
        ok(r#"[{"id":"buildbox","label":"buildbox","target":"buildbox-pi","session":"default","enabled":true}]"#),
    );
    crate::harness::install(&with_box.ctx()).unwrap();
    assert_eq!(
        with_box.runner.count("ssh"),
        5,
        "one box build per repo plus lane settings, the pi guard, and the running-process check"
    );
    let calls = with_box.runner.calls.borrow();
    let local_guard = calls
        .iter()
        .find(|call| {
            call.program.ends_with("/.local/bin/herdr-pi") && call.args == ["refresh-guard"]
        })
        .unwrap();
    assert!(local_guard.env.contains(&(
        "HERDR_ADE_ROOT".into(),
        with_box.ctx().root.display().to_string()
    )));
    let scripts: Vec<String> = calls
        .iter()
        .filter(|c| c.program == "ssh")
        .map(|c| c.args.last().cloned().unwrap_or_default())
        .collect();
    let build_scripts: Vec<_> = scripts
        .iter()
        .filter(|script| script.contains("git fetch --quiet"))
        .collect();
    assert_eq!(build_scripts.len(), 2, "{scripts:?}");
    assert!(
        build_scripts
            .iter()
            .all(|s| s.contains("git merge --ff-only")
                && s.contains("cargo build --release --locked")
                && s.contains("source_dirty=\"$(git status --porcelain")
                && s.contains("cp target/release/")
                && s.contains("install_to=/home/agent/.local/bin/")
                && s.contains("mv -f \"$install_tmp\" \"$install_to\"")),
        "{scripts:?}"
    );
    for script in build_scripts {
        let refresh = script
            .find("git read-tree HEAD && git update-index -q --refresh")
            .expect("box build refreshes the sync-stale index");
        let source_head = script.find("source_head=").unwrap();
        let source_dirty = script.find("source_dirty=").unwrap();
        let build = script.find("cargo build --release --locked").unwrap();
        assert!(
            refresh < source_head && refresh < source_dirty && refresh < build,
            "box index refresh must precede source inspection and build: {script}"
        );
    }
    let settings = calls
        .iter()
        .find(|call| {
            call.program == "ssh"
                && call
                    .args
                    .last()
                    .is_some_and(|script| script.contains(crate::harness::BOX_WORKER_MARKER))
        })
        .unwrap();
    assert_eq!(settings.stdin.as_deref(), Some("# Lane rules\n"));
    assert!(
        scripts.iter().any(
            |script| script.contains("HERDR_ADE_ROOT=/home/agent/.herdr-ade")
                && script.contains("/home/agent/.local/bin/herdr-pi refresh-guard")
        ),
        "{scripts:?}"
    );
    drop(calls);

    let without_box = World::new();
    let (p, f) = plugin(&without_box);
    write_harness_config(
        &without_box,
        &[
            (&p, "/home/agent/projects/herdr-ade"),
            (&f, "/home/agent/projects/herdr"),
        ],
    );
    without_box.runner.on("cargo build", ok(""));
    without_box.runner.on("cp ", ok(""));
    without_box.runner.on("mv -f", ok(""));
    without_box
        .runner
        .on("--version", ok("installed version\n"));
    without_box.runner.on("machine list --json", ok("[]"));
    without_box.runner.on("ssh", ok(""));
    crate::harness::install(&without_box.ctx()).unwrap();
    assert_eq!(without_box.runner.count("ssh"), 0);
}

#[test]
fn harness_install_reports_unavailable_box_without_reexec() {
    let world = World::new();
    let plugin = harness_repo(world.home.path(), "plugin", "herdr-ade");
    write_harness_config(&world, &[(&plugin, "/home/agent/projects/herdr-ade")]);
    configure_test_box(&world);
    world.runner.on("cargo build", ok(""));
    world.runner.on("cp ", ok(""));
    world.runner.on("mv -f", ok(""));
    world.runner.on("--version", ok("installed version\n"));

    world
        .runner
        .on("machine list --json", fail(1, "machine list unavailable"));
    let outcome = crate::harness::install(&world.ctx()).unwrap();
    assert_eq!(world.runner.count("cargo build"), 1);
    assert!(!outcome.warnings.is_empty());
}

#[test]
fn harness_install_lock_refuses_a_second_install() {
    let world = World::new();
    let plugin = harness_repo(world.home.path(), "plugin", "herdr-ade");
    write_harness_config(&world, &[(&plugin, "/home/agent/projects/herdr-ade")]);
    let _held = crate::harness::lock(&world.home.path().join("cfg")).unwrap();
    let error = crate::harness::install(&world.ctx())
        .unwrap_err()
        .to_string();
    assert!(error.contains("harness_install_busy"), "{error}");
}
