//! Adopting an already-running local agent pane into a project.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::herdr::{Agent, Herdr};
use crate::paths::{self, Ctx, SessionFlags};
use crate::project::{self, Project};
use crate::runner::Cmd;
use crate::thread::{self, Kind, Status, Thread};
use crate::{coordinator, threads, ticker};

const DEFAULT_TASK: &str = "Continue the work you were already doing in this pane. It now belongs to the project described above: follow its instructions, and when you finish or stop to wait for the user, write the report.";

/// The refusals shared by `thread adopt` and `adopt-workspace`, checked before
/// anything is created. Returns the agent herdr detects in the pane.
pub(crate) fn adoptable_agent(ctx: &Ctx, herdr: &Herdr, socket: &str, pane: &str) -> Result<Agent> {
    let agents = herdr
        .agent_list()
        .map_err(|e| anyhow::anyhow!("the herdr session at {socket} is not reachable: {e}"))?;
    let agent = agents
        .into_iter()
        .find(|a| a.pane_id == pane)
        .with_context(|| format!("no agent is detected in pane {pane} of the session at {socket}; only a running local agent pane can be adopted"))?;
    // A pane is identified by (socket, machine, id): the same id in another
    // session is another pane.
    for slug in project::list_slugs(&ctx.root) {
        let Ok(other) = Project::load(&ctx.root, &slug) else {
            continue;
        };
        let Some(record) = other.coordinator() else {
            continue;
        };
        if record.socket != socket {
            continue;
        }
        if record.pane_id == pane && coordinator::agent_matches(&record, &agent) {
            bail!("pane {pane} is the coordinator of `{slug}`");
        }
        if let Some(t) = thread::list(&other).iter().find(|t| {
            !t.is_remote()
                && t.status != Status::Resolved
                && t.pane_id == pane
                && thread::agent_matches(t, &agent)
        }) {
            bail!("pane {pane} is already thread {} of `{slug}`", t.id);
        }
    }
    Ok(agent)
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AdeAdopt {
    pub(crate) plain: String,
    pub(crate) role: Option<String>,
    pub(crate) passive: bool,
}

pub(crate) fn adopt(
    ctx: &Ctx,
    slug: &str,
    pane: &str,
    title: &str,
    task: Option<String>,
    ade: AdeAdopt,
) -> Result<Thread> {
    let project = Project::load(&ctx.root, slug)?;
    if project.status() != project::Status::Active {
        bail!(
            "`{slug}` is {}; adopting is refused until it is active again",
            project.status()
        );
    }
    if title.trim().is_empty() {
        bail!("--title may not be empty");
    }
    crate::threads::check_birth_plain(&ade.plain)?;
    let role = ade
        .role
        .as_deref()
        .filter(|r| !r.is_empty())
        .unwrap_or("lane");
    let passive = ade.passive;
    let record = project
        .coordinator()
        .with_context(|| format!("`{slug}` has never been opened; run `open {slug}` first"))?;
    ticker::start(ctx)?;
    let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, ctx.runner);
    let agent = adoptable_agent(ctx, &herdr, &record.socket, pane)?;
    // Adoption records the process already running; it never selects a model.
    let spec = crate::contracts::RoleSpec {
        kind: agent.agent.clone(),
        ..Default::default()
    };

    // The pane's repository and branch, when it is in one.
    let git = |args: &[&str]| -> Option<String> {
        let out = ctx
            .runner
            .run(
                &Cmd::new("git", Duration::from_secs(5))
                    .args(["-C", &agent.cwd])
                    .args(args.iter().copied()),
            )
            .ok()?;
        out.success()
            .then(|| out.stdout.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let repo = git(&["rev-parse", "--show-toplevel"]).unwrap_or_default();
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|b| b != "HEAD")
        .unwrap_or_default();
    let origin = git(&["remote", "get-url", "origin"]).unwrap_or_default();

    let created = thread::allocate(&project, |t| {
        t.title = title.trim().to_string();
        t.kind = Kind::Adopted;
        t.repo = repo;
        t.branch = branch;
        t.origin = origin;
        t.agent = agent.agent.clone();
        // Not started by the binary: whatever name herdr reports, possibly empty.
        t.agent_name = agent.name.clone();
        t.cwd = agent.cwd.clone();
        t.workspace_id = agent.workspace_id.clone();
        t.tab_id = agent.tab_id.clone();
        t.pane_id = agent.pane_id.clone();
        t.role = role.to_string();
        t.plain = ade.plain.trim().to_string();
        t.passive = passive;
        t.attempt = 1;
        t.launch = project::launch_recipe(
            &spec,
            1,
            String::new(),
            project::policy_hash(&ctx.config_dir),
            role,
        );
        t.agent = spec.kind.clone();
    })?;
    let id = created.id.clone();
    let task = task
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_TASK.to_string());

    let briefed = (|| -> Result<()> {
        {
            let _lock = project.lock()?;
            project::write_atomic(
                &thread::task_path_for_write(&project, &id)?,
                task.as_bytes(),
            )?;
        }
        let prefix = crate::coordinator::current_prefix(&ctx.root)?;
        if created.repo.is_empty() {
            // An adopted process cannot have its cwd replaced, but all of its
            // durable work lives in the same project-owned git folder as a
            // newly started no-repository thread. `done` stages this folder.
            let folder = thread::threads_dir_for_write(&project)?.join(&id);
            let with_dir = Thread {
                worktree_path: folder.to_string_lossy().into_owned(),
                thread_dir: folder.to_string_lossy().into_owned(),
                ..created.clone()
            };
            let brief = thread::with_lane_skill(
                &prefix,
                &thread::brief_for(&project, &with_dir, &task, false)?,
            );
            let (folder, hash, base) =
                threads::prepare_managed_git_folder(ctx.runner, &folder, &brief)?;
            let folder = folder.to_string_lossy().into_owned();
            thread::update(&project, &id, |t| {
                t.worktree_path = folder.clone();
                t.thread_dir = folder;
                t.branch = "main".into();
                t.base = base;
                t.launch.brief_hash = hash;
            })?;
        } else {
            // An adopted process already in a code repository keeps its
            // per-thread report directory out of that repository.
            let dir = thread::thread_dir(&agent.cwd, slug, &id);
            let with_dir = Thread {
                thread_dir: dir.clone(),
                ..created.clone()
            };
            let brief = thread::with_lane_skill(
                &prefix,
                &thread::brief_for(&project, &with_dir, &task, false)?,
            );
            std::fs::create_dir_all(&dir).with_context(|| format!("could not create {dir}"))?;
            threads::exclude_from_git(ctx.runner, &agent.cwd)?;
            project::write_atomic(&Path::new(&dir).join("brief.md"), brief.as_bytes())?;
            thread::update(&project, &id, |t| t.thread_dir = dir)?;
        }
        Ok(())
    })();
    if let Err(error) = briefed {
        let message = format!("{error:#}");
        let _ = thread::update(&project, &id, |t| {
            t.status = Status::Failed;
            t.error = message;
        });
        return Err(error);
    }

    // Keep the record in Starting while this command owns first delivery, so
    // the ticker cannot race it and send the brief twice. Passive adopt
    // (SPEC-ADE D7) sends nothing.
    herdr.pane_clear_tokens(pane, &["parent"])?;
    let pending = !passive;
    let adopted = thread::update(&project, &id, |t| {
        t.status = if passive {
            Status::Open
        } else {
            Status::Starting
        };
        t.prompt_pending = pending;
        t.last_state = agent.agent_status.clone();
        t.last_state_change = project::now();
        thread::bind_identity(
            t,
            &record.socket,
            &agent,
            herdr
                .pane_process_info(pane)
                .ok()
                .and_then(|p| p.identity(&agent.agent)),
        );
    })?;
    if passive {
        threads::report_thread_tokens(&herdr, &adopted, slug, thread::Group::Working);
        return Ok(adopted);
    }

    let timeout_ms = adopted.launch.ready_timeout_ms.max(1);
    let ready = if agent.ready() {
        agent
    } else {
        match herdr.agent_wait_ready(pane, timeout_ms) {
            Ok(agent) => agent,
            Err(error) => {
                let pending = thread::update(&project, &id, |t| t.status = Status::Open)?;
                threads::report_thread_tokens(&herdr, &pending, slug, thread::Group::Working);
                bail!(
                    "thread {id} was adopted, but its brief was not delivered: herdr reported status `{}` in pane {pane}, then readiness failed: {error}; the brief remains pending",
                    agent.agent_status
                );
            }
        }
    };
    let placed = thread::update(&project, &id, |t| {
        t.last_state = ready.agent_status.clone();
        t.last_state_change = project::now();
        thread::bind_identity(
            t,
            &record.socket,
            &ready,
            herdr
                .pane_process_info(pane)
                .ok()
                .and_then(|p| p.identity(&ready.agent)),
        );
    })?;
    if let Err(error) =
        herdr.agent_prompt_wait_started(pane, &thread::launch_prompt("", slug, &placed), timeout_ms)
    {
        let pending = thread::update(&project, &id, |t| t.status = Status::Open)?;
        threads::report_thread_tokens(&herdr, &pending, slug, thread::Group::Working);
        bail!(
            "thread {id} was adopted, but its brief was not delivered: herdr reported status `{}` in pane {pane}, then refused or stalled the prompt: {error}; the brief remains pending",
            ready.agent_status
        );
    }
    let adopted = thread::update(&project, &id, |t| {
        t.status = Status::Open;
        t.prompt_pending = false;
    })?;
    threads::report_thread_tokens(&herdr, &adopted, slug, thread::Group::Working);
    Ok(adopted)
}

pub(crate) struct AdoptWorkspace {
    pub(crate) name: String,
    pub(crate) goal: String,
    /// The adopted thread's birth sentence (SPEC-ADE D17 item 6).
    pub(crate) plain: String,
    pub(crate) pane: String,
    pub(crate) workspace_cwd: String,
    pub(crate) session: SessionFlags,
}

/// "Continue as a project": `new`, then `open`, then `thread adopt`. It
/// refuses, before creating anything, under the same conditions as `thread adopt`.
pub(crate) fn adopt_workspace(ctx: &Ctx, args: &AdoptWorkspace) -> Result<()> {
    crate::threads::check_birth_plain(&args.plain)?;
    let session = paths::resolve_session(&args.session, ctx.env, ctx.runner)?;
    let socket = session.socket.to_string_lossy().into_owned();
    let herdr = Herdr::new(ctx.env.herdr_bin(), &session.socket, ctx.runner);
    let agent = adoptable_agent(ctx, &herdr, &socket, &args.pane)?;
    let slug = project::slug_from_name(&args.name)?;
    if ctx.root.join(&slug).exists() {
        bail!("`{slug}` already exists in {}", ctx.root.display());
    }

    // repo = the workspace's directory when that is a git repository.
    let cwd = if args.workspace_cwd.is_empty() {
        agent.cwd.clone()
    } else {
        args.workspace_cwd.clone()
    };
    let is_repo = ctx
        .runner
        .run(&Cmd::new("git", Duration::from_secs(5)).args([
            "-C",
            &cwd,
            "rev-parse",
            "--show-toplevel",
        ]))
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty());
    let repos = is_repo
        .map(|path| {
            vec![project::Repo {
                path,
                machine: None,
                ..project::Repo::default()
            }]
        })
        .unwrap_or_default();

    let project = project::create(&ctx.root, &args.name, &args.goal, repos)?;
    println!("created `{}` at {}", project.slug, project.dir().display());
    coordinator::open(
        ctx,
        &project.slug,
        &coordinator::OpenOptions {
            session: SessionFlags {
                session: None,
                socket: Some(session.socket.clone()),
            },
            reprime: false,
            rebind: false,
            recipe: None,
            recipe_basis: None,
        },
    )?;
    let adopted = adopt(
        ctx,
        &project.slug,
        &args.pane,
        &args.name,
        None,
        AdeAdopt {
            plain: args.plain.clone(),
            ..AdeAdopt::default()
        },
    )?;
    println!(
        "adopted pane {} as thread {} of `{}`",
        args.pane, adopted.id, project.slug
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Runner;
    use crate::runner::fake::ok;
    use crate::scenarios::{World, agent_json};

    fn lane() -> AdeAdopt {
        AdeAdopt {
            plain: "The lane does the work.".into(),
            ..AdeAdopt::default()
        }
    }

    fn world_with_agent(state: &str, name: &str) -> (World, Project, String) {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        let cwd = cwd.to_string_lossy().into_owned();
        *world.agents.borrow_mut() = format!(
            "[{},{}]",
            agent_json("w5", "w5:t1", "w5:p1", &cwd, name, state),
            agent_json("w5", "w5:t1", "w5:p2", &cwd, "", "idle")
        );
        world.runner.on_fn(
            |cmd| cmd.program == "git",
            |cmd| crate::runner::RealRunner.run(cmd),
        );
        (world, project, cwd)
    }

    #[test]
    fn adopting_a_ready_agent_writes_a_brief_and_prompts_it() {
        let (world, project, _cwd) = world_with_agent("idle", "my-agent");
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let t = adopt(
            &world.ctx(),
            "demo",
            "w5:p1",
            "Adopted work",
            Some("Finish the refactor.".into()),
            lane(),
        )
        .unwrap();
        assert_eq!(
            (t.kind, t.status, t.prompt_pending),
            (Kind::Adopted, Status::Open, false)
        );
        assert_eq!(t.agent_name, "my-agent");
        assert_eq!(
            Path::new(&t.thread_dir),
            std::fs::canonicalize(thread::threads_dir(&project).join("t-0001"))
                .unwrap()
                .as_path()
        );
        assert_eq!(t.worktree_path, t.thread_dir);
        assert!(Path::new(&t.thread_dir).join(".git").is_dir());
        let brief = std::fs::read_to_string(format!("{}/brief.md", t.thread_dir)).unwrap();
        assert!(brief.contains("Finish the refactor."));
        assert_eq!(world.runner.count("agent prompt"), 1);
        assert_eq!(world.runner.count("agent start"), 0);

        // A second pane in the same directory gets its own thread directory.
        let second = adopt(&world.ctx(), "demo", "w5:p2", "Second", None, lane()).unwrap();
        assert_eq!(
            Path::new(&second.thread_dir),
            std::fs::canonicalize(thread::threads_dir(&project).join("t-0002"))
                .unwrap()
                .as_path()
        );
        assert!(second.agent_name.is_empty());
        assert_ne!(t.thread_dir, second.thread_dir);
        let _ = project;
    }

    #[test]
    fn adopting_a_starting_agent_waits_until_ready_before_prompting() {
        let (world, _project, cwd) = world_with_agent("starting", "my-agent");
        world.runner.on(
            "agent wait",
            ok(&format!(
                r#"{{"result":{{"agent":{}}}}}"#,
                agent_json("w5", "w5:t1", "w5:p1", &cwd, "my-agent", "idle")
            )),
        );
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));

        let t = adopt(&world.ctx(), "demo", "w5:p1", "Starting", None, lane()).unwrap();
        assert!(!t.prompt_pending);
        let calls = world.runner.calls.borrow();
        let waited = calls
            .iter()
            .position(|call| call.display().contains("agent wait"))
            .unwrap();
        let prompted = calls
            .iter()
            .position(|call| call.display().contains("agent prompt"))
            .unwrap();
        assert!(waited < prompted);
        assert!(
            calls[waited]
                .display()
                .contains("--until idle --until done")
        );
        assert!(
            calls[prompted]
                .display()
                .contains("--wait --until working --until blocked")
        );
    }

    #[test]
    fn adopting_an_agent_that_never_becomes_ready_reports_failure_and_keeps_brief_pending() {
        let (world, project, _cwd) = world_with_agent("starting", "my-agent");
        world.runner.on(
            "agent wait",
            crate::runner::fake::fail(
                1,
                r#"{"error":{"code":"timeout","message":"timed out waiting for agent status"}}"#,
            ),
        );

        let error = adopt(&world.ctx(), "demo", "w5:p1", "Starting", None, lane())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("herdr reported status `starting`"),
            "{error}"
        );
        assert!(
            error.contains("timed out waiting for agent status"),
            "{error}"
        );
        let t = thread::load(&project, "t-0001").unwrap();
        assert_eq!(t.status, Status::Open);
        assert!(t.prompt_pending);
        assert!(Path::new(&t.thread_dir).join("brief.md").is_file());
        assert_eq!(world.runner.count("agent prompt"), 0);
    }

    #[test]
    fn a_stalled_adopt_prompt_is_reported_and_stays_pending() {
        let (world, project, _cwd) = world_with_agent("idle", "my-agent");
        world.runner.on_fn(
            |cmd| cmd.display().contains("agent prompt"),
            |_| {
                Ok(crate::runner::fake::fail(
                    1,
                    r#"{"error":{"code":"agent_prompt_stalled","message":"agent prompt produced no observed working state"}}"#,
                ))
            },
        );

        let error = adopt(&world.ctx(), "demo", "w5:p1", "Ready", None, lane())
            .unwrap_err()
            .to_string();
        assert!(error.contains("agent_prompt_stalled"), "{error}");
        let t = thread::load(&project, "t-0001").unwrap();
        assert_eq!(t.status, Status::Open);
        assert!(t.prompt_pending);
    }

    #[test]
    fn refusals_happen_before_anything_is_created() {
        let (world, project, _) = world_with_agent("idle", "my-agent");
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
        let ctx = world.ctx();
        // No detected agent in that pane.
        assert!(
            adopt(&ctx, "demo", "w9:p9", "x", None, lane())
                .unwrap_err()
                .to_string()
                .contains("no agent is detected")
        );
        // Already a thread.
        adopt(&ctx, "demo", "w5:p1", "first", None, lane()).unwrap();
        assert!(
            adopt(&ctx, "demo", "w5:p1", "again", None, lane())
                .unwrap_err()
                .to_string()
                .contains("already thread t-0001")
        );
        assert_eq!(thread::list(&project).len(), 1);

        // Same pane id recorded by a project in ANOTHER socket is a different pane.
        let other = world.project("other", "b.sock");
        let t = adopt(&ctx, "other", "w5:p1", "other session", None, lane());
        // `other` lives in b.sock; the fake serves the same agent list for it, so
        // the pane is adoptable there: ids are only compared within one socket.
        assert!(t.is_ok(), "{t:?}");
        let _ = other;
    }

    #[test]
    fn adopt_workspace_refuses_without_an_agent_and_creates_nothing() {
        let (world, _, _) = world_with_agent("idle", "my-agent");
        let socket = world.home.path().join("a.sock");
        let args = AdoptWorkspace {
            name: "From Workspace".into(),
            goal: String::new(),
            pane: "w9:p9".into(),
            workspace_cwd: String::new(),
            plain: "The lane does the work.".into(),
            session: SessionFlags {
                session: None,
                socket: Some(socket),
            },
        };
        assert!(adopt_workspace(&world.ctx(), &args).is_err());
        assert!(!world.root.join("from-workspace").exists());
    }

    #[test]
    fn passive_adopt_sends_no_prompt_and_clears_push_link() {
        let (world, _, _) = world_with_agent("idle", "pro");
        let t = adopt(
            &world.ctx(),
            "demo",
            "w5:p1",
            "Pro pane",
            None,
            AdeAdopt {
                plain: "The critic reads the report.".into(),
                role: Some("pro".into()),
                passive: true,
            },
        )
        .unwrap();
        assert!(t.passive);
        assert_eq!(t.role, "pro");
        assert_eq!(world.runner.count("agent prompt"), 0);
        let calls = world.runner.calls.borrow();
        let parent = calls.iter().any(|c| {
            c.display().contains("report-metadata") && c.display().contains("--clear-token parent")
        });
        assert!(parent, "expected parent link removal");
    }
}
