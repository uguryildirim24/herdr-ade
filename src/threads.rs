//! The `thread` subcommands. Each is one deterministic mechanic; the
//! coordinator decides whether, what and where.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::herdr::{Agent, Herdr, Pane};
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::runner::{Cmd, Runner};
use crate::thread::{self, CopyOutcome, Group, Kind, Live, Status, Thread};
use crate::{coordinator, remote, ticker};

const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// The project's session as the binary sees it right now.
pub struct SessionView<'a> {
    pub herdr: Herdr<'a>,
    pub agents: Vec<Agent>,
    pub panes: Vec<Pane>,
}

/// `None` when the project was never opened or its session is unreachable.
pub fn session_view<'a>(ctx: &'a Ctx, project: &Project) -> Option<SessionView<'a>> {
    let record = project.coordinator()?;
    if record.socket.is_empty() || !Path::new(&record.socket).exists() {
        return None;
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, ctx.runner);
    let agents = herdr.agent_list().ok()?;
    let panes = herdr.pane_list().ok()?;
    Some(SessionView {
        herdr,
        agents,
        panes,
    })
}

fn require_session<'a>(ctx: &'a Ctx, project: &Project) -> Result<SessionView<'a>> {
    session_view(ctx, project).with_context(|| {
        format!(
            "the herdr session of `{}` is not reachable; run `open {}` first",
            project.slug, project.slug
        )
    })
}

fn git(runner: &dyn Runner, repo: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let out = runner.run(
        &Cmd::new("git", timeout)
            .args(["-C", repo])
            .args(args.iter().copied()),
    )?;
    if !out.success() {
        bail!("git {}: {}", args.join(" "), out.error_text());
    }
    Ok(out.stdout.trim().to_string())
}

pub fn thread_tokens(thread: &Thread, slug: &str, group: Group) -> Vec<(String, String)> {
    vec![
        ("project".into(), slug.to_string()),
        ("thread".into(), thread.id.clone()),
        ("review".into(), group.token().to_string()),
        ("rank".into(), group.rank().to_string()),
    ]
}

pub fn report_thread_tokens(herdr: &Herdr, thread: &Thread, slug: &str, group: Group) {
    let tokens = thread_tokens(thread, slug, group);
    let pairs: Vec<(&str, &str)> = tokens
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let _ = herdr.on_machine(&thread.machine).pane_report_tokens(
        &thread.pane_id,
        &pairs,
        coordinator::TOKEN_TTL,
    );
}

fn clear_thread_tokens(herdr: &Herdr, thread: &Thread) {
    if !thread.pane_id.is_empty() {
        let _ = herdr
            .on_machine(&thread.machine)
            .pane_clear_tokens(&thread.pane_id, &["project", "thread", "review", "rank"]);
    }
}

pub struct StartArgs {
    pub title: String,
    pub repo: Option<String>,
    pub machine: Option<String>,
    pub agent: Option<String>,
    pub base: Option<String>,
    pub task: String,
}

/// ADE fields for `thread start` (SPEC-ADE D2, D17 item 6). Not added to
/// `StartArgs` so unowned scenario constructors keep compiling.
#[derive(Debug, Clone, Default)]
pub struct AdeStart {
    pub plain: String,
    pub role: Option<String>,
}

/// Birth sentence: required, one sentence, R1–R5 (SPEC-ADE D17 item 6).
pub fn check_birth_plain(text: &str) -> Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("plain_missing");
    }
    let glossary = crate::plain::Glossary::default();
    let result = crate::plain::check(trimmed, &glossary);
    if !result.passed() {
        let detail: Vec<String> = result
            .violations
            .iter()
            .map(|v| format!("{}: {}", v.rule.code(), v.fix))
            .collect();
        bail!("{}", detail.join("; "));
    }
    let sentences: Vec<&str> = trimmed
        .split(['.', '?', '!', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if sentences.len() != 1 {
        bail!("write one sentence of at most 25 words");
    }
    Ok(())
}

/// Creates the workspace or tab, the thread directory and the brief, then
/// returns. The agent is launched by the ticker, so there is one delivery path.
/// Unowned scenario tests still call this legacy path.
#[allow(dead_code)]
pub fn start(ctx: &Ctx, slug: &str, args: StartArgs) -> Result<Thread> {
    start_inner(ctx, slug, args, None)
}

pub fn start_with_ade(ctx: &Ctx, slug: &str, args: StartArgs, ade: AdeStart) -> Result<Thread> {
    start_inner(ctx, slug, args, Some(ade))
}

fn start_inner(ctx: &Ctx, slug: &str, args: StartArgs, ade: Option<AdeStart>) -> Result<Thread> {
    let project = Project::load(&ctx.root, slug)?;
    let status = project.status();
    if status != project::Status::Active {
        bail!("`{slug}` is {status}; `thread start` is refused until it is active again");
    }
    if args.title.trim().is_empty() {
        bail!("--title may not be empty");
    }
    if args.task.trim().is_empty() {
        bail!("the task is empty");
    }
    let (settings, _) = project.read_project_md()?;
    // Without a running ticker nothing launches.
    ticker::start(ctx)?;
    let view = require_session(ctx, &project)?;

    let listed = args
        .repo
        .as_ref()
        .and_then(|repo| settings.repos.iter().find(|r| &r.path == repo));
    let machine = args
        .machine
        .clone()
        .or_else(|| listed.and_then(|r| r.machine.clone()))
        .unwrap_or_default();
    let repo = match (&args.repo, machine.is_empty()) {
        (None, false) => bail!(
            "a remote thread needs --repo: a task with no repository runs as a tab in the project's own workspace, which is local"
        ),
        (None, true) => String::new(),
        // A remote path is stored as it is on its own machine.
        (Some(repo), false) => repo.clone(),
        (Some(repo), true) => {
            let path = std::fs::canonicalize(repo)
                .with_context(|| format!("repository {repo} does not exist"))?
                .to_string_lossy()
                .into_owned();
            if !settings
                .repos
                .iter()
                .any(|r| r.path == path || &r.path == repo)
            {
                eprintln!("warning: {path} is not listed in `repos` in PROJECT.md");
            }
            path
        }
    };
    if !machine.is_empty() && listed.is_none() {
        eprintln!("warning: {repo} on {machine} is not listed in `repos` in PROJECT.md");
    }

    let open_count = thread::list(&project)
        .iter()
        .filter(|t| t.status == Status::Open || t.status == Status::Starting)
        .count();
    if open_count as u32 >= settings.max_parallel_threads {
        eprintln!(
            "warning: {open_count} threads are already open; max_parallel_threads is {}",
            settings.max_parallel_threads
        );
    }

    let mut ade_spec = None;
    let mut ade_plain = String::new();
    let mut ade_role = String::new();
    if let Some(ade) = &ade {
        check_birth_plain(&ade.plain)?;
        if !machine.is_empty() {
            bail!("remote_not_admissible");
        }
        let role = ade
            .role
            .as_deref()
            .filter(|r| !r.is_empty())
            .unwrap_or("lane");
        // Validation happens before any tab or worktree (SPEC-ADE D2).
        let spec = project::resolve_role(&ctx.config_dir, &settings, role)?;
        ade_spec = Some(spec);
        ade_plain = ade.plain.trim().to_string();
        ade_role = role.to_string();
    }

    let agent_kind = if let Some(spec) = &ade_spec {
        spec.kind.clone()
    } else {
        args.agent
            .clone()
            .unwrap_or_else(|| settings.thread_agent.clone())
    };
    let record = thread::allocate(&project, |t| {
        t.title = args.title.trim().to_string();
        t.kind = if repo.is_empty() {
            Kind::Tab
        } else {
            Kind::Worktree
        };
        t.repo = repo.clone();
        t.machine = machine.clone();
        t.agent = agent_kind.clone();
        t.base = args.base.clone().unwrap_or_default();
        if let Some(spec) = &ade_spec {
            t.role = ade_role.clone();
            t.plain = ade_plain.clone();
            t.attempt = 1;
            t.launch = project::launch_recipe(
                spec,
                1,
                String::new(),
                project::policy_hash(&ctx.config_dir),
            );
        }
    })?;
    let id = record.id.clone();
    {
        let _lock = project.lock()?;
        project::write_atomic(&thread::task_path(&project, &id), args.task.as_bytes())?;
    }

    match place_and_brief(ctx, &project, &view, &id, false) {
        Ok(thread) => Ok(thread),
        Err(error) => {
            // Nothing is cleaned up automatically; `thread restart` retries.
            let message = format!("{error:#}");
            let _ = thread::update(&project, &id, |t| {
                t.status = Status::Failed;
                t.error = message.clone();
            });
            Err(error.context(format!(
                "thread {id} failed to start; `thread restart {slug} {id}` retries"
            )))
        }
    }
}

/// Steps 2 to 5 of starting a thread, also used by `thread restart` case (a).
fn place_and_brief(
    ctx: &Ctx,
    project: &Project,
    view: &SessionView,
    id: &str,
    restart: bool,
) -> Result<Thread> {
    let slug = &project.slug;
    let record = thread::load(project, id)?;
    let runner = ctx.runner;

    let placed = match record.kind {
        Kind::Worktree if record.is_ade() && !record.is_remote() => {
            place_ade_worktree(ctx, project, view, &record)?
        }
        Kind::Worktree if record.is_remote() => {
            // The same steps on the thread's own machine: git over ssh, herdr
            // through `--machine`.
            let target = remote::ssh_target(
                runner,
                &ctx.env.herdr_bin(),
                &ctx.config_dir,
                &record.machine,
            )?;
            let (origin, base) = remote::repo_info(runner, &target, &record.repo, &record.base)?;
            let branch = thread::branch_name(slug, id, &record.title);
            let (created, path, cwd) = view.herdr.on_machine(&record.machine).worktree_create(
                &record.repo,
                &branch,
                &base,
                &record.title,
            )?;
            thread::update(project, id, |t| {
                t.origin = origin;
                t.base = base;
                t.branch = branch;
                t.worktree_path = path;
                t.cwd = cwd;
                t.workspace_id = created.workspace_id;
                t.tab_id = created.tab_id;
                t.pane_id = created.pane_id;
            })?
        }
        Kind::Worktree => {
            git(
                runner,
                &record.repo,
                &["rev-parse", "--show-toplevel"],
                GIT_TIMEOUT,
            )
            .with_context(|| format!("{} is not a git repository", record.repo))?;
            let origin = git(
                runner,
                &record.repo,
                &["remote", "get-url", "origin"],
                GIT_TIMEOUT,
            )
            .unwrap_or_default();
            if !origin.is_empty()
                && let Err(error) = git(runner, &record.repo, &["fetch", "origin"], FETCH_TIMEOUT)
            {
                eprintln!("warning: {error:#}");
            }
            let base = if record.base.is_empty() {
                git(
                    runner,
                    &record.repo,
                    &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
                    GIT_TIMEOUT,
                )
                .or_else(|_| {
                    git(
                        runner,
                        &record.repo,
                        &["rev-parse", "--abbrev-ref", "HEAD"],
                        GIT_TIMEOUT,
                    )
                })
                .and_then(|base| match base.as_str() {
                    "HEAD" => git(runner, &record.repo, &["rev-parse", "HEAD"], GIT_TIMEOUT),
                    _ => Ok(base),
                })?
            } else {
                record.base.clone()
            };
            let branch = thread::branch_name(slug, id, &record.title);
            let (created, path, cwd) =
                view.herdr
                    .worktree_create(&record.repo, &branch, &base, &record.title)?;
            // Recorded immediately, so a command killed midway still leaves a
            // record `thread restart` can act on.
            thread::update(project, id, |t| {
                t.origin = origin;
                t.base = base;
                t.branch = branch;
                t.worktree_path = path;
                t.cwd = cwd;
                t.workspace_id = created.workspace_id;
                t.tab_id = created.tab_id;
                t.pane_id = created.pane_id;
            })?
        }
        Kind::Tab if record.is_ade() => place_ade_tab(ctx, project, view, &record)?,
        Kind::Tab => place_tab(project, view, &record)?,
        Kind::Adopted => bail!("an adopted thread is not placed by the binary"),
    };
    write_brief(ctx, project, &placed, restart)?;
    if placed.is_ade() {
        let hash = thread::sha256_hex(
            std::fs::read(Path::new(&placed.thread_dir).join("brief.md"))
                .unwrap_or_default()
                .as_slice(),
        );
        thread::update(project, id, |t| t.launch.brief_hash = hash)?;
    }
    finish_placement(project, view, id)
}

/// The thread directory, the git exclude and `brief.md`, on the thread's own
/// machine. The brief never refers to a path on another machine.
fn write_brief(ctx: &Ctx, project: &Project, placed: &Thread, restart: bool) -> Result<()> {
    if !placed.is_remote() {
        return write_brief_local(ctx, project, placed, restart);
    }
    let dir = thread::thread_dir(&placed.cwd, &project.slug, &placed.id);
    let with_dir = Thread {
        thread_dir: dir.clone(),
        ..placed.clone()
    };
    let task = std::fs::read_to_string(thread::task_path(project, &placed.id)).unwrap_or_default();
    let brief = thread::brief_for(project, &with_dir, &task, restart)?;
    let target = remote::ssh_target(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        &placed.machine,
    )?;
    remote::write_brief(ctx.runner, &target, &placed.cwd, &dir, &brief)?;
    thread::update(project, &placed.id, |t| t.thread_dir = dir)?;
    Ok(())
}

fn place_ade_worktree(
    ctx: &Ctx,
    project: &Project,
    view: &SessionView,
    record: &Thread,
) -> Result<Thread> {
    let runner = ctx.runner;
    git(
        runner,
        &record.repo,
        &["rev-parse", "--show-toplevel"],
        GIT_TIMEOUT,
    )
    .with_context(|| format!("{} is not a git repository", record.repo))?;
    let origin = git(
        runner,
        &record.repo,
        &["remote", "get-url", "origin"],
        GIT_TIMEOUT,
    )
    .unwrap_or_default();
    let base = if record.base.is_empty() {
        git(
            runner,
            &record.repo,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
            GIT_TIMEOUT,
        )
        .or_else(|_| {
            git(
                runner,
                &record.repo,
                &["rev-parse", "--abbrev-ref", "HEAD"],
                GIT_TIMEOUT,
            )
        })
        .and_then(|base| match base.as_str() {
            "HEAD" => git(runner, &record.repo, &["rev-parse", "HEAD"], GIT_TIMEOUT),
            _ => Ok(base),
        })?
    } else {
        record.base.clone()
    };
    let branch = thread::branch_name(&project.slug, &record.id, &record.title);
    let task = std::fs::read_to_string(thread::task_path(project, &record.id)).unwrap_or_default();
    let planned = Path::new(&record.repo).join(".worktrees").join(&record.id);
    let stub = Thread {
        thread_dir: thread::thread_dir(&planned.to_string_lossy(), &project.slug, &record.id),
        ..record.clone()
    };
    let brief = thread::brief_for(project, &stub, &task, false)?;
    let committed = format!("plain: {}\n\n{brief}", record.plain);
    let rel = format!("tasks/{}.md", record.id);
    let sha = crate::git::commit_file_from_parent(
        runner,
        &record.repo,
        &base,
        &rel,
        committed.as_bytes(),
        &format!("docs(tasks): {}", record.id),
    )?;
    let brief_hash = thread::sha256_hex(committed.as_bytes());
    if let Err(error) = crate::git::exclude_plugin_paths(runner, &record.repo) {
        eprintln!("warning: {error:#}");
    }
    let path = match crate::git::worktree_add(runner, &record.repo, &record.id, &branch, &sha) {
        Ok(path) => path,
        Err(error) => {
            let _ = thread::update(project, &record.id, |t| {
                t.partial = Some("worktree_add".into());
                t.launch.brief_hash = brief_hash.clone();
                t.base = sha.clone();
                t.branch = branch.clone();
            });
            return Err(error);
        }
    };
    let cwd = path.to_string_lossy().into_owned();
    let coordinator = project
        .coordinator()
        .context("the project has never been opened")?;
    let spec = crate::contracts::RoleSpec {
        kind: record.launch.kind.clone(),
        args: record.launch.args.clone(),
        env: record.launch.env.clone(),
        ready_timeout_ms: record.launch.ready_timeout_ms,
    };
    let env = project::tab_env(
        &project.slug,
        &record.id,
        record.attempt.max(1),
        &brief_hash,
        &spec,
    );
    match view
        .herdr
        .tab_create_env(&coordinator.workspace_id, &path, &record.id, false, &env)
    {
        Ok(created) => {
            let pane_cwd = view
                .herdr
                .pane_cwd(&created.pane_id)
                .unwrap_or_else(|_| cwd.clone());
            thread::update(project, &record.id, |t| {
                t.origin = origin;
                t.base = sha;
                t.branch = branch;
                t.worktree_path = cwd.clone();
                t.cwd = if pane_cwd.is_empty() { cwd } else { pane_cwd };
                t.workspace_id = created.workspace_id;
                t.tab_id = created.tab_id;
                t.pane_id = created.pane_id;
                t.launch.brief_hash = brief_hash;
                t.partial = None;
            })
        }
        Err(error) => {
            let _ = thread::update(project, &record.id, |t| {
                t.partial = Some("tab_create".into());
                t.origin = origin;
                t.base = sha;
                t.branch = branch;
                t.worktree_path = cwd.clone();
                t.cwd = cwd;
                t.launch.brief_hash = brief_hash;
            });
            Err(anyhow::anyhow!("{error}"))
        }
    }
}

fn place_ade_tab(
    _ctx: &Ctx,
    project: &Project,
    view: &SessionView,
    record: &Thread,
) -> Result<Thread> {
    let coordinator = project
        .coordinator()
        .context("the project has never been opened")?;
    if !view.panes.iter().any(|p| {
        p.workspace_id == coordinator.workspace_id && coordinator::pane_matches(&coordinator, p)
    }) {
        bail!(
            "the project's workspace is not open; run `open {}` first",
            project.slug
        );
    }
    let folder = if record.worktree_path.is_empty() {
        let folder = project.dir().join("threads").join(&record.id);
        {
            let _lock = project.lock()?;
            if !folder.is_dir() {
                std::fs::create_dir(&folder)
                    .with_context(|| format!("could not create {}", folder.display()))?;
            }
        }
        std::fs::canonicalize(&folder)?
    } else {
        Path::new(&record.worktree_path).to_path_buf()
    };
    let spec = crate::contracts::RoleSpec {
        kind: record.launch.kind.clone(),
        args: record.launch.args.clone(),
        env: record.launch.env.clone(),
        ready_timeout_ms: record.launch.ready_timeout_ms,
    };
    let brief_hash = if record.launch.brief_hash.is_empty() {
        "0".to_string()
    } else {
        record.launch.brief_hash.clone()
    };
    let env = project::tab_env(
        &project.slug,
        &record.id,
        record.attempt.max(1),
        &brief_hash,
        &spec,
    );
    let created = view
        .herdr
        .tab_create_env(&coordinator.workspace_id, &folder, &record.id, false, &env)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let cwd = view.herdr.pane_cwd(&created.pane_id).unwrap_or_default();
    let cwd = if cwd.is_empty() {
        folder.to_string_lossy().into_owned()
    } else {
        cwd
    };
    thread::update(project, &record.id, |t| {
        t.cwd = cwd;
        t.workspace_id = created.workspace_id;
        t.tab_id = created.tab_id;
        t.pane_id = created.pane_id;
        if t.worktree_path.is_empty() && t.kind == Kind::Worktree {
            t.worktree_path = folder.to_string_lossy().into_owned();
        }
    })
}

fn place_tab(project: &Project, view: &SessionView, record: &Thread) -> Result<Thread> {
    let coordinator = project
        .coordinator()
        .context("the project has never been opened")?;
    if !view.panes.iter().any(|p| {
        p.workspace_id == coordinator.workspace_id && coordinator::pane_matches(&coordinator, p)
    }) {
        bail!(
            "the project's workspace is not open; run `open {}` first",
            project.slug
        );
    }
    let folder = project.dir().join("threads").join(&record.id);
    {
        let _lock = project.lock()?;
        if !folder.is_dir() {
            std::fs::create_dir(&folder)
                .with_context(|| format!("could not create {}", folder.display()))?;
        }
    }
    let folder = std::fs::canonicalize(&folder)?;
    let created =
        view.herdr
            .tab_create(&coordinator.workspace_id, &folder, &record.title, false)?;
    let cwd = view.herdr.pane_cwd(&created.pane_id).unwrap_or_default();
    let cwd = if cwd.is_empty() {
        folder.to_string_lossy().into_owned()
    } else {
        cwd
    };
    thread::update(project, &record.id, |t| {
        t.cwd = cwd;
        t.workspace_id = created.workspace_id;
        t.tab_id = created.tab_id;
        t.pane_id = created.pane_id;
    })
}

/// Creates the thread directory, keeps it out of git, writes `brief.md`.
fn write_brief_local(ctx: &Ctx, project: &Project, placed: &Thread, restart: bool) -> Result<()> {
    let dir = thread::thread_dir(&placed.cwd, &project.slug, &placed.id);
    let with_dir = Thread {
        thread_dir: dir.clone(),
        ..placed.clone()
    };
    let task = std::fs::read_to_string(thread::task_path(project, &placed.id)).unwrap_or_default();
    let brief = thread::brief_for(project, &with_dir, &task, restart)?;

    std::fs::create_dir_all(Path::new(&dir).join("library"))
        .with_context(|| format!("could not create {dir}"))?;
    if placed.kind != Kind::Tab {
        exclude_from_git(ctx.runner, &placed.cwd)?;
    }
    project::write_atomic(&Path::new(&dir).join("brief.md"), brief.as_bytes())?;
    thread::update(project, &placed.id, |t| t.thread_dir = dir)?;
    Ok(())
}

/// Adds `.herdr-project/` to the repository's `info/exclude` if it is not
/// already listed, so nothing in the thread directory is ever committed.
pub fn exclude_from_git(runner: &dyn Runner, cwd: &str) -> Result<()> {
    let Ok(path) = git(
        runner,
        cwd,
        &["rev-parse", "--git-path", "info/exclude"],
        GIT_TIMEOUT,
    ) else {
        return Ok(()); // not inside a git repository
    };
    let path = Path::new(cwd).join(path);
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if current.lines().any(|line| line.trim() == ".herdr-project/") {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(".herdr-project/\n");
    std::fs::write(&path, text).with_context(|| format!("could not update {}", path.display()))
}

/// Step 5: hand the thread to the ticker's launch step.
fn finish_placement(project: &Project, view: &SessionView, id: &str) -> Result<Thread> {
    let thread = thread::update(project, id, |t| {
        t.agent_name = thread::agent_name(&project.slug, &t.id);
        t.prompt_pending = true;
        t.launch_attempts = 0;
        t.status = Status::Open;
        t.error.clear();
        t.last_state.clear();
        t.last_state_change = project::now();
    })?;
    report_thread_tokens(&view.herdr, &thread, &project.slug, Group::Working);
    Ok(thread)
}

#[derive(Debug, PartialEq)]
pub enum RestartPlan {
    /// (a) nothing was created: run the create step again.
    Create,
    /// (c) the recorded pane is alive at a shell prompt: reuse it.
    ReusePane,
    /// (e) open the existing worktree, or a new tab in `threads/<id>/`.
    Reopen,
}

/// What `thread restart` does, from what the record shows was reached.
pub fn restart_plan(
    thread: &Thread,
    live: &Live,
    branch_exists: bool,
    now: jiff::Timestamp,
) -> Result<RestartPlan> {
    match thread.kind {
        Kind::Adopted => bail!("an adopted thread cannot be restarted; adopt a new pane instead"),
        Kind::Worktree | Kind::Tab => {}
    }
    if thread.status == Status::Resolved {
        bail!("{} is resolved; `thread resolve --reopen` first", thread.id);
    }
    if thread.status == Status::Starting
        && thread::seconds_since(&thread.created, now) < thread::STARTING_TIMEOUT_SECS
    {
        bail!("{} is still starting", thread.id);
    }
    // (d)
    if live.agent_state.is_some() {
        bail!("{} is running: its pane has an agent in it", thread.id);
    }
    if live.pane_exists
        && thread.prompt_pending
        && thread.launch_attempts < thread::MAX_LAUNCH_ATTEMPTS
        && thread.status == Status::Open
    {
        bail!(
            "{} is being launched by the ticker (attempt {} of {})",
            thread.id,
            thread.launch_attempts,
            thread::MAX_LAUNCH_ATTEMPTS
        );
    }
    if thread.kind == Kind::Worktree && thread.worktree_path.is_empty() {
        if branch_exists {
            // (b)
            bail!(
                "{}: no worktree was recorded but its branch already exists. A half-made worktree needs a human look: run `thread resolve`, then start a new thread.",
                thread.id
            );
        }
        return Ok(RestartPlan::Create);
    }
    if thread.kind == Kind::Tab && thread.pane_id.is_empty() {
        return Ok(RestartPlan::Create);
    }
    if live.pane_exists {
        return Ok(RestartPlan::ReusePane);
    }
    Ok(RestartPlan::Reopen)
}

/// Agents and panes of the server a thread lives in: the project's session,
/// or its machine's through `herdr --machine`.
fn lists_for(view: &SessionView, record: &Thread) -> Result<(Vec<Agent>, Vec<Pane>)> {
    if !record.is_remote() {
        return Ok((view.agents.clone(), view.panes.clone()));
    }
    let herdr = view.herdr.on_machine(&record.machine);
    let unreachable = |e: crate::herdr::HerdrError| {
        anyhow::anyhow!("machine `{}` is unreachable: {e}", record.machine)
    };
    Ok((
        herdr.agent_list().map_err(unreachable)?,
        herdr.pane_list().map_err(unreachable)?,
    ))
}

pub fn restart(ctx: &Ctx, slug: &str, id: &str) -> Result<Thread> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    ticker::start(ctx)?;
    let view = require_session(ctx, &project)?;
    let (agents, panes) = lists_for(&view, &record)?;
    let now = jiff::Timestamp::now();
    let live = thread::live_state(&record, &agents, &panes, now);
    let branch_exists = record.kind == Kind::Worktree && record.worktree_path.is_empty() && {
        let branch = thread::branch_name(slug, id, &record.title);
        if record.is_remote() {
            let target = remote::ssh_target(
                ctx.runner,
                &ctx.env.herdr_bin(),
                &ctx.config_dir,
                &record.machine,
            )?;
            remote::branch_exists(ctx.runner, &target, &record.repo, &branch)?
        } else {
            git(
                ctx.runner,
                &record.repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
                GIT_TIMEOUT,
            )
            .is_ok()
        }
    };

    let plan = restart_plan(&record, &live, branch_exists, now)?;
    if record.is_ade() {
        thread::update(&project, id, |t| {
            t.attempt = t.attempt.max(1).saturating_add(1);
            t.launch.attempt = t.attempt;
        })?;
    }
    match plan {
        RestartPlan::Create => return place_and_brief(ctx, &project, &view, id, true),
        RestartPlan::ReusePane => {}
        RestartPlan::Reopen => {
            let record = thread::load(&project, id)?;
            if record.is_ade() {
                place_ade_tab(ctx, &project, &view, &record)?;
            } else {
                match record.kind {
                    Kind::Worktree => {
                        let (created, path, cwd) = view
                            .herdr
                            .on_machine(&record.machine)
                            .worktree_open(&record.repo, &record.worktree_path, &record.title)?;
                        thread::update(&project, id, |t| {
                            t.worktree_path = path;
                            t.cwd = cwd;
                            t.workspace_id = created.workspace_id;
                            t.tab_id = created.tab_id;
                            t.pane_id = created.pane_id;
                        })?;
                    }
                    _ => {
                        place_tab(&project, &view, &record)?;
                    }
                }
            }
        }
    }
    let placed = thread::load(&project, id)?;
    write_brief(ctx, &project, &placed, true)?;
    finish_placement(&project, &view, id)
}

/// Sends a follow-up. The one sender that does not use the ready-for-a-prompt
/// predicate: agents queue a message that arrives while they work.
pub fn prompt(ctx: &Ctx, slug: &str, id: &str, text: &str) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if text.trim().is_empty() {
        bail!("the text is empty");
    }
    if record.status == Status::Resolved {
        bail!("{id} is resolved");
    }
    if record.prompt_pending {
        bail!("{id} has not received its brief yet; try again once it has started");
    }
    let view = require_session(ctx, &project)?;
    let (agents, _) = lists_for(&view, &record)?;
    let state = prompt_state(&record, &agents)?;
    view.herdr
        .on_machine(&record.machine)
        .agent_prompt(&record.pane_id, text.trim())
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(state)
}

/// The state a follow-up may be sent in, or the refusal.
pub fn prompt_state(record: &Thread, agents: &[Agent]) -> Result<String> {
    let agent = agents
        .iter()
        .find(|a| thread::agent_matches(record, a))
        .with_context(|| format!("no agent is detected in {}'s pane; text is never typed at a bare shell prompt (try `thread restart`)", record.id))?;
    match agent.agent_status.as_str() {
        "blocked" => bail!(
            "agent_blocked: {} is waiting on the user in its pane ({})",
            record.id,
            record.pane_id
        ),
        "unknown" => bail!("{}'s agent state is unknown; not sending", record.id),
        state => Ok(state.to_string()),
    }
}

pub fn ack(ctx: &Ctx, slug: &str, id: &str) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::update(&project, id, |t| {
        t.acked_report_hash = t.report_hash.clone()
    })?;
    if record.report_hash.is_empty() {
        println!("{id} has no report yet; nothing to acknowledge");
    } else {
        println!("{id}: report acknowledged");
    }
    Ok(())
}

#[derive(Default)]
pub struct ResolveArgs {
    pub reopen: bool,
    pub remove_worktree: bool,
    pub skip_copy: bool,
    pub discard_uncopied: bool,
}

pub fn resolve(ctx: &Ctx, slug: &str, id: &str, args: &ResolveArgs) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if args.reopen {
        if record.status != Status::Resolved {
            bail!("{id} is not resolved");
        }
        thread::update(&project, id, |t| {
            t.status = Status::Open;
            t.resolved_reason.clear();
        })?;
        println!(
            "{id} is open again. Nothing was started; `thread restart {slug} {id}` brings its agent back."
        );
        return Ok(());
    }
    if args.skip_copy && args.remove_worktree {
        bail!("--skip-copy cannot be combined with --remove-worktree");
    }
    if args.remove_worktree && record.kind != Kind::Worktree {
        bail!(
            "--remove-worktree is only for worktree threads; {id} is a {:?} thread",
            record.kind
        );
    }

    // Every path that resolves a thread performs a final copy first.
    if !args.skip_copy {
        let copied = final_copy(ctx, &project, &record);
        match &copied.outcome {
            CopyOutcome::Complete => {}
            CopyOutcome::Partial(notes) => {
                println!("the final copy was partial:");
                for note in notes {
                    println!("  - {note}");
                }
                if args.remove_worktree && !args.discard_uncopied {
                    bail!(
                        "refusing --remove-worktree: removing the worktree would delete what was not copied. Pass --discard-uncopied to accept that loss."
                    );
                }
            }
            CopyOutcome::Failed(error) => {
                bail!(
                    "the final copy failed ({error}); not resolving. `--skip-copy` resolves without it."
                );
            }
        }
    }

    if args.remove_worktree {
        remove_worktree(ctx, &project, &record)?;
        // The record says what exists: `delete` lists leftovers from it.
        thread::update(&project, id, |t| t.worktree_path.clear())?;
    }
    let resolved = thread::update(&project, id, |t| {
        t.status = Status::Resolved;
        t.resolved_reason = "manual".into();
        t.prompt_pending = false;
    })?;
    if let Some(view) = session_view(ctx, &project) {
        clear_thread_tokens(&view.herdr, &resolved);
    }
    println!("{id} resolved.");
    if !args.remove_worktree {
        match resolved.kind {
            Kind::Worktree if resolved.worktree_path.is_empty() => {
                println!("No worktree was recorded for it, so there is nothing to close or remove.")
            }
            Kind::Worktree => println!(
                "Its pane, workspace, worktree ({}) and branch ({}) were left alone. Close the workspace in herdr, or run `thread resolve {slug} {id} --remove-worktree`.",
                resolved.worktree_path, resolved.branch
            ),
            _ => println!("Its pane and tab were left alone; close them in herdr."),
        }
    } else {
        println!(
            "The worktree {} was removed; the branch {} was kept.",
            record.worktree_path, resolved.branch
        );
    }
    Ok(())
}

/// The final report and library copy, storing the new report hash.
pub fn final_copy(ctx: &Ctx, project: &Project, record: &Thread) -> thread::Copied {
    let copied = if record.is_remote() {
        match remote::ssh_target(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            &record.machine,
        ) {
            Ok(target) => thread::copy_home_remote(project, record, true, ctx.runner, &target),
            Err(error) => thread::Copied {
                outcome: CopyOutcome::Failed(format!("{error:#}")),
                report_hash: None,
            },
        }
    } else {
        thread::copy_home_local(project, record, true, ctx.runner)
    };
    if let Some(hash) = &copied.report_hash
        && *hash != record.report_hash
    {
        let _ = thread::update(project, &record.id, |t| {
            t.report_hash = hash.clone();
            t.last_report_change = project::now();
        });
    }
    copied
}

/// Never forces. herdr's or git's refusal (for example uncommitted changes) is
/// reported unchanged.
fn remove_worktree(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if record.is_ade() && !record.is_remote() {
        return remove_ade_worktree(ctx, project, record);
    }
    if record.worktree_path.is_empty() {
        bail!("{} has no recorded worktree", record.id);
    }
    let view = require_session(ctx, project)?;
    let (_, panes) = lists_for(&view, record)?;
    let workspace_open = panes.iter().any(|p| {
        p.workspace_id == record.workspace_id
            && Path::new(&p.cwd).starts_with(&record.worktree_path)
    });
    if workspace_open {
        return view
            .herdr
            .on_machine(&record.machine)
            .worktree_remove(&record.workspace_id)
            .map_err(|error| anyhow::anyhow!("{error}"));
    }
    if record.is_remote() {
        let target = remote::ssh_target(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            &record.machine,
        )?;
        let script = format!(
            "cd {} && git worktree remove {}",
            remote::quote(&record.repo),
            remote::quote(&record.worktree_path)
        );
        let out = remote::ssh(ctx.runner, &target, &script, None, Duration::from_secs(20))?;
        if !out.success() {
            bail!("{}", out.error_text());
        }
        return Ok(());
    }
    git(
        ctx.runner,
        &record.repo,
        &["worktree", "remove", &record.worktree_path],
        Duration::from_secs(20),
    )
    .map(|_| ())
}

fn remove_ade_worktree(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if record.worktree_path.is_empty() {
        bail!("{} has no recorded worktree", record.id);
    }
    let view = require_session(ctx, project)?;
    let (agents, panes) = lists_for(&view, record)?;
    let live = thread::live_state(record, &agents, &panes, jiff::Timestamp::now());
    if live.agent_state.as_deref() == Some("working") {
        bail!("{} is working; not removing the worktree", record.id);
    }
    if let Err(error) = crate::git::worktree_remove(ctx.runner, &record.repo, &record.worktree_path)
    {
        let _ = thread::update(project, &record.id, |t| {
            t.partial = Some("worktree_remove".into());
        });
        return Err(error);
    }
    if !record.tab_id.is_empty() {
        if let Err(error) = view.herdr.tab_close(&record.tab_id) {
            let _ = thread::update(project, &record.id, |t| {
                t.partial = Some("tab_close".into());
            });
            return Err(anyhow::anyhow!("{error}"));
        }
    }
    Ok(())
}

/// Lineage repair for ADE threads (SPEC-ADE D3). Wired from A1's ticker.
pub fn tick(t: &mut crate::ticker::Ticker<'_>) -> anyhow::Result<()> {
    let Some(herdr) = t.herdr else {
        return Ok(());
    };
    let coordinator = match t.project.coordinator() {
        Some(c) => c,
        None => return Ok(()),
    };
    for record in thread::list(t.project) {
        if !record.is_ade() || record.status == Status::Resolved {
            continue;
        }
        let Some(agent) = t.agents.iter().find(|a| thread::agent_matches(&record, a)) else {
            continue;
        };
        let process = herdr
            .pane_process_info(&record.pane_id)
            .ok()
            .and_then(|info| info.identity());
        if record.identity.process.is_some() {
            if !thread::identity_verifies(&record, agent, process.as_ref()) {
                let _ = crate::inbox::write(
                    t.project,
                    "lineage-mismatch",
                    &record.id,
                    "the live process does not match the stored identity; parent was not repaired",
                    "",
                );
                continue;
            }
        } else if record.kind != Kind::Adopted {
            continue;
        }
        if agent.parent() != Some(coordinator.pane_id.as_str()) {
            let _ = herdr.pane_set_parent(&record.pane_id, &coordinator.pane_id);
        }
    }
    Ok(())
}

/// A thread with its live state and group, for `thread list`, `thread show`
/// and the overview.
pub struct Row {
    pub thread: Thread,
    pub group: Group,
    pub note: String,
}

pub fn rows(ctx: &Ctx, project: &Project) -> Vec<Row> {
    let view = session_view(ctx, project);
    let now = jiff::Timestamp::now();
    thread::list(project)
        .into_iter()
        .map(|t| row(&t, view.as_ref(), now))
        .collect()
}

fn row(t: &Thread, view: Option<&SessionView>, now: jiff::Timestamp) -> Row {
    // Before the first poll a thread that is waiting for its launch is Working.
    let recorded = Group::from_token(&t.last_group).unwrap_or(if t.prompt_pending {
        Group::Working
    } else {
        Group::Idle
    });
    if t.status == Status::Resolved {
        return Row {
            thread: t.clone(),
            group: Group::Resolved,
            note: t.resolved_reason.clone(),
        };
    }
    let Some(view) = view else {
        // Records are still printed; panes are not treated as gone.
        return Row {
            thread: t.clone(),
            group: recorded,
            note: "session unreachable".into(),
        };
    };
    if t.is_remote() {
        // Remote state is what the ticker last polled; the CLI makes no ssh call.
        let state = if t.last_state.is_empty() {
            "not polled yet"
        } else {
            &t.last_state
        };
        return Row {
            thread: t.clone(),
            group: recorded,
            note: format!("{state}, on {}", t.machine),
        };
    }
    let live = thread::live_state(t, &view.agents, &view.panes, now);
    // A report the ticker has not hashed yet still counts, as it does for the ticker.
    let fresh = Thread {
        report_hash: thread::local_report_hash(t).unwrap_or_else(|| t.report_hash.clone()),
        ..t.clone()
    };
    let group = thread::group(&fresh, &live, now);
    let note = if t.status == Status::Failed {
        format!("failed: {}", t.error)
    } else if !live.pane_exists {
        "pane closed".to_string()
    } else {
        live.agent_state.unwrap_or_else(|| "no agent".into())
    };
    Row {
        thread: t.clone(),
        group,
        note,
    }
}

pub fn print_list(ctx: &Ctx, slug: &str) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    for row in rows(ctx, &project) {
        println!(
            "{}\t{}\t{}\t{}",
            row.thread.id,
            row.group.label(),
            row.note,
            row.thread.title
        );
    }
    Ok(())
}

pub fn print_show(ctx: &Ctx, slug: &str, id: &str) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    let view = session_view(ctx, &project);
    let row = row(&record, view.as_ref(), jiff::Timestamp::now());
    println!("group = {:?}", row.group.label());
    println!("live = {:?}", row.note);
    print!("{}", toml::to_string(&record)?);
    let report = thread::home_report_path(&project, id);
    if report.is_file() {
        println!("# home copy of the report: {}", report.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> jiff::Timestamp {
        "2026-09-17T12:00:00Z".parse().unwrap()
    }

    fn worktree_thread() -> Thread {
        Thread {
            id: "t-0001".into(),
            kind: Kind::Worktree,
            status: Status::Open,
            created: "2026-09-17T10:00:00Z".into(),
            worktree_path: "/wt".into(),
            pane_id: "w2:p1".into(),
            ..Thread::default()
        }
    }

    fn gone() -> Live {
        Live {
            pane_exists: false,
            agent_state: None,
            state_secs: 0,
        }
    }

    fn shell() -> Live {
        Live {
            pane_exists: true,
            agent_state: None,
            state_secs: 0,
        }
    }

    #[test]
    fn restart_case_a_nothing_created() {
        let t = Thread {
            status: Status::Failed,
            worktree_path: String::new(),
            ..worktree_thread()
        };
        assert_eq!(
            restart_plan(&t, &gone(), false, now()).unwrap(),
            RestartPlan::Create
        );
    }

    #[test]
    fn restart_case_b_branch_without_worktree_needs_a_human() {
        let t = Thread {
            status: Status::Failed,
            worktree_path: String::new(),
            ..worktree_thread()
        };
        let error = restart_plan(&t, &gone(), true, now())
            .unwrap_err()
            .to_string();
        assert!(error.contains("thread resolve"), "{error}");
    }

    #[test]
    fn restart_case_c_reuses_a_pane_at_a_shell_prompt() {
        assert_eq!(
            restart_plan(&worktree_thread(), &shell(), false, now()).unwrap(),
            RestartPlan::ReusePane
        );
    }

    #[test]
    fn restart_case_d_refuses_a_running_thread() {
        let running = Live {
            pane_exists: true,
            agent_state: Some("working".into()),
            state_secs: 0,
        };
        assert!(restart_plan(&worktree_thread(), &running, false, now()).is_err());
    }

    #[test]
    fn restart_case_e_reopens_the_worktree() {
        assert_eq!(
            restart_plan(&worktree_thread(), &gone(), false, now()).unwrap(),
            RestartPlan::Reopen
        );
        let tab = Thread {
            kind: Kind::Tab,
            worktree_path: String::new(),
            ..worktree_thread()
        };
        assert_eq!(
            restart_plan(&tab, &gone(), false, now()).unwrap(),
            RestartPlan::Reopen
        );
    }

    #[test]
    fn restart_refuses_a_launch_in_progress_adopted_resolved_and_young_starting() {
        let launching = Thread {
            prompt_pending: true,
            launch_attempts: 1,
            ..worktree_thread()
        };
        assert!(restart_plan(&launching, &shell(), false, now()).is_err());
        let exhausted = Thread {
            prompt_pending: true,
            launch_attempts: 3,
            ..worktree_thread()
        };
        assert_eq!(
            restart_plan(&exhausted, &shell(), false, now()).unwrap(),
            RestartPlan::ReusePane
        );

        let adopted = Thread {
            kind: Kind::Adopted,
            ..worktree_thread()
        };
        assert!(restart_plan(&adopted, &gone(), false, now()).is_err());
        let resolved = Thread {
            status: Status::Resolved,
            ..worktree_thread()
        };
        assert!(restart_plan(&resolved, &gone(), false, now()).is_err());

        let young = Thread {
            status: Status::Starting,
            created: "2026-09-17T11:59:00Z".into(),
            worktree_path: String::new(),
            ..worktree_thread()
        };
        assert!(restart_plan(&young, &gone(), false, now()).is_err());
        let stale = Thread {
            created: "2026-09-17T11:00:00Z".into(),
            ..young
        };
        assert_eq!(
            restart_plan(&stale, &gone(), false, now()).unwrap(),
            RestartPlan::Create
        );
    }

    fn agent(state: &str) -> Agent {
        Agent {
            pane_id: "w2:p1".into(),
            agent_status: state.into(),
            ..Agent::default()
        }
    }

    #[test]
    fn prompt_refusals_and_sending_while_working() {
        let t = Thread {
            agent_name: String::new(),
            kind: Kind::Adopted,
            ..worktree_thread()
        };
        assert!(
            prompt_state(&t, &[])
                .unwrap_err()
                .to_string()
                .contains("bare shell prompt")
        );
        assert!(prompt_state(&t, &[agent("unknown")]).is_err());
        assert!(
            prompt_state(&t, &[agent("blocked")])
                .unwrap_err()
                .to_string()
                .contains("agent_blocked")
        );
        assert_eq!(prompt_state(&t, &[agent("working")]).unwrap(), "working");
        assert_eq!(prompt_state(&t, &[agent("idle")]).unwrap(), "idle");
    }

    #[test]
    fn token_values_and_ranks() {
        let tokens = thread_tokens(&worktree_thread(), "demo", Group::WaitingOnYou);
        assert_eq!(
            tokens,
            vec![
                ("project".to_string(), "demo".to_string()),
                ("thread".to_string(), "t-0001".to_string()),
                ("review".to_string(), "waiting-on-you".to_string()),
                ("rank".to_string(), "2".to_string()),
            ]
        );
    }

    #[test]
    fn exclude_is_added_once() {
        let repo = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .output()
                .unwrap()
        };
        run(&["init", "-q"]);
        let cwd = repo.path().to_string_lossy().into_owned();
        exclude_from_git(&crate::runner::RealRunner, &cwd).unwrap();
        exclude_from_git(&crate::runner::RealRunner, &cwd).unwrap();
        let text = std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
        assert_eq!(text.matches(".herdr-project/").count(), 1);
        std::fs::create_dir_all(repo.path().join(".herdr-project/x")).unwrap();
        std::fs::write(repo.path().join(".herdr-project/x/report.md"), "r").unwrap();
        assert!(String::from_utf8_lossy(&run(&["status", "--porcelain"]).stdout).is_empty());
    }

    #[test]
    fn birth_sentence_is_required_and_checked() {
        let err = check_birth_plain("").unwrap_err().to_string();
        assert!(err.contains("plain_missing"), "{err}");
        assert!(check_birth_plain("bisimulation quotient").is_err());
        check_birth_plain("The lane does the work.").unwrap();
    }

    struct GitReal<'a> {
        fake: &'a crate::runner::fake::FakeRunner,
    }

    impl crate::runner::Runner for GitReal<'_> {
        fn run(&self, cmd: &crate::runner::Cmd) -> anyhow::Result<crate::runner::Output> {
            if cmd.program == "git" {
                crate::runner::RealRunner.run(cmd)
            } else {
                self.fake.run(cmd)
            }
        }

        fn socket_request(
            &self,
            socket: &Path,
            line: &str,
            timeout: Duration,
        ) -> anyhow::Result<String> {
            self.fake.socket_request(socket, line, timeout)
        }
    }

    fn init_repo(path: &Path) {
        let repo_s = path.to_string_lossy().into_owned();
        let run = |args: &[&str]| {
            crate::runner::RealRunner
                .run(
                    &crate::runner::Cmd::new("git", Duration::from_secs(5))
                        .args(["-C", &repo_s])
                        .args(args.iter().copied()),
                )
                .unwrap()
        };
        assert!(
            crate::runner::RealRunner
                .run(
                    &crate::runner::Cmd::new("git", Duration::from_secs(5))
                        .args(["init", "-b", "main", &repo_s])
                )
                .unwrap()
                .success()
        );
        let _ = run(&["config", "user.email", "ade@test"]);
        let _ = run(&["config", "user.name", "ade"]);
        std::fs::write(path.join("README"), "x\n").unwrap();
        assert!(run(&["add", "README"]).success());
        assert!(run(&["commit", "-m", "init"]).success());
    }

    #[test]
    fn ade_start_uses_git_worktree_and_tab_env_then_parent_launch() {
        use crate::runner::fake::ok;
        use crate::scenarios::{World, pane_json};

        let world = World::new();
        let project = world.project("demo", "a.sock");
        let repo = world.home.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let repo_s = std::fs::canonicalize(&repo)
            .unwrap()
            .to_string_lossy()
            .into_owned();

        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        world.runner.on(
            "HERDR_ADE_LAUNCH",
            ok(r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","cwd":"/wt"}}}"#),
        );
        world.runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","name":"hp-demo-t-0001","tokens":{"parent":"w1:p1"}}}}"#),
        );
        world.runner.on(
            "process-info",
            ok(r#"{"result":{"process_info":{"pane_id":"w1:p2","foreground_processes":[{"pid":42,"name":"claude","argv0":"/bin/claude"}]}}}"#),
        );
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));

        let split = GitReal {
            fake: &world.runner,
        };
        let ctx = crate::paths::Ctx {
            env: &world.env,
            root: world.root.clone(),
            config_dir: world.home.path().join("cfg"),
            runner: &split,
            detached_ticker: false,
        };
        let started = start_with_ade(
            &ctx,
            "demo",
            StartArgs {
                title: "Fix login".into(),
                repo: Some(repo_s.clone()),
                machine: None,
                agent: None,
                base: None,
                task: "Do the thing.".into(),
            },
            AdeStart {
                plain: "The lane does the work.".into(),
                role: None,
            },
        )
        .unwrap();
        assert_eq!(started.role, "lane");
        assert!(!started.launch.kind.is_empty());
        assert!(!started.launch.brief_hash.is_empty());
        assert_eq!(started.attempt, 1);
        let wt = Path::new(&repo_s).join(".worktrees").join(&started.id);
        assert!(wt.is_dir(), "git worktree should exist");
        let calls = world.runner.calls.borrow();
        assert!(
            calls.iter().any(|c| {
                let line = c.display();
                line.contains("tab create") && line.contains("HERDR_ADE_LAUNCH=")
            }),
            "expected tab create --env HERDR_ADE_LAUNCH"
        );
        assert!(
            !calls
                .iter()
                .any(|c| c.display().contains("worktree create"))
        );
        assert!(!calls.iter().any(|c| c.display().contains("worktree open")));
        drop(calls);

        let kind = started.launch.kind.clone();
        let wt_s = wt.to_string_lossy().into_owned();
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            pane_json("w1", "w1:t2", "w1:p2", &wt_s)
        );
        crate::ticker::tick_project(&ctx, &project).unwrap();
        let calls = world.runner.calls.borrow();
        let launch = calls
            .iter()
            .find(|c| c.display().contains("agent start"))
            .map(|c| c.display())
            .expect("agent start");
        assert!(launch.contains("--parent w1:p1"), "{launch}");
        drop(calls);

        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        let restarted = restart(&ctx, "demo", &started.id).unwrap();
        assert_eq!(restarted.attempt, 2);
        assert_eq!(restarted.launch.kind, kind);
        assert_eq!(restarted.launch.attempt, 2);
        let calls = world.runner.calls.borrow();
        assert!(!calls.iter().any(|c| c.display().contains("worktree open")));
        let tabs = calls
            .iter()
            .filter(|c| c.display().contains("tab create"))
            .count();
        assert!(tabs >= 2, "restart should open a tab, not a workspace");
    }

    #[test]
    fn ade_start_refuses_remote_and_empty_plain() {
        let world = crate::scenarios::World::new();
        let _project = world.project("demo", "a.sock");
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&_project));
        let ctx = world.ctx();
        let missing = start_with_ade(
            &ctx,
            "demo",
            StartArgs {
                title: "X".into(),
                repo: None,
                machine: None,
                agent: None,
                base: None,
                task: "Do the thing.".into(),
            },
            AdeStart {
                plain: String::new(),
                role: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(missing.contains("plain_missing"), "{missing}");

        let remote = start_with_ade(
            &ctx,
            "demo",
            StartArgs {
                title: "X".into(),
                repo: Some("/repo".into()),
                machine: Some("box".into()),
                agent: None,
                base: None,
                task: "Do the thing.".into(),
            },
            AdeStart {
                plain: "The lane does the work.".into(),
                role: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(remote.contains("remote_not_admissible"), "{remote}");
    }

    #[test]
    fn lineage_mismatch_does_not_repair_parent() {
        use crate::runner::fake::ok;
        use crate::scenarios::{World, agent_json, pane_json};

        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("wt");
        std::fs::create_dir(&cwd).unwrap();
        let cwd_s = cwd.to_string_lossy().into_owned();
        world.thread(&project, &cwd, |t| {
            t.role = "lane".into();
            t.plain = "The lane does the work.".into();
            t.launch.kind = "claude".into();
            t.identity.process = Some(crate::contracts::ProcessIdentity {
                pid: 1,
                argv0: "/bin/claude".into(),
            });
            t.identity.pane_id = "w2:p1".into();
            t.identity.cwd = cwd_s.clone();
        });
        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json("w2", "w2:t1", "w2:p1", &cwd_s, "hp-demo-t-0001", "idle")
        );
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            pane_json("w2", "w2:t1", "w2:p1", &cwd_s)
        );
        world.runner.on(
            "process-info",
            ok(r#"{"result":{"process_info":{"pane_id":"w2:p1","foreground_processes":[{"pid":99,"name":"claude","argv0":"/bin/claude"}]}}}"#),
        );
        crate::ticker::tick_project(&world.ctx(), &project).unwrap();
        let inbox = std::fs::read_dir(project.dir().join("inbox"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("lineage-mismatch"))
            .count();
        assert_eq!(inbox, 1);
        let repaired = world.runner.calls.borrow().iter().any(|c| {
            let line = c.display();
            line.contains("report-metadata")
                && line.contains("parent=w1:p1")
                && line.contains("w2:p1")
        });
        assert!(!repaired, "mismatch must not repair parent");
    }
}
