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

/// The project's session as the binary sees it right now.
pub struct SessionView<'a> {
    pub herdr: Herdr<'a>,
    pub agents: Vec<Agent>,
    pub panes: Vec<Pane>,
}

/// `None` when the project was never opened or its session is unreachable.
pub fn session_view<'a>(ctx: &'a Ctx, project: &Project) -> Option<SessionView<'a>> {
    let _scope = crate::ledger::Scope::new(&[project]);
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
    let _ = herdr.on_machine(thread.machine_route()).pane_report_tokens(
        &thread.pane_id,
        &pairs,
        coordinator::TOKEN_TTL,
    );
}

/// The `parent` value a lane's pane carries. On this Mac it is the bare
/// coordinator pane; a box lane names the machine its coordinator lives on,
/// `<label>:<pane>`, the form the fork lane t-0053 introduces (SPEC-remote §6).
pub fn parent_token(record: &Thread, coordinator_pane: &str) -> String {
    if record.is_remote() {
        format!(
            "{}:{coordinator_pane}",
            crate::contracts::MACHINE_LOCAL_LABEL
        )
    } else {
        coordinator_pane.to_string()
    }
}

fn clear_thread_tokens(herdr: &Herdr, thread: &Thread) {
    if !thread.pane_id.is_empty() {
        let _ = herdr
            .on_machine(thread.machine_route())
            .pane_clear_tokens(&thread.pane_id, &["project", "thread", "review", "rank"]);
    }
}

pub struct StartArgs {
    pub title: String,
    pub repo: Option<String>,
    pub machine: Option<String>,
    pub base: Option<String>,
    pub task: String,
    /// The birth sentence (SPEC-ADE D17 item 6).
    pub plain: String,
    /// Internal flow/skill label; never a coordinator model-selection input.
    pub workflow: Option<String>,
}

/// Birth sentence: required, one sentence, R1–R5 except the known-word rule
/// (SPEC-ADE D17 item 6). The sentence is a row on a screen, so it may name a
/// file; `say`, `ask` and their choices keep the known-word rule.
pub fn check_birth_plain(text: &str) -> Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("plain_missing");
    }
    let glossary = crate::plain::Glossary::default();
    let result = crate::plain::check_record(trimmed, &glossary);
    if !result.passed() {
        let detail: Vec<String> = result
            .violations
            .iter()
            .map(|v| format!("{}: {}", v.rule.code(), v.fix))
            .collect();
        bail!("{}", detail.join("; "));
    }
    if crate::plain::sentence_count(trimmed) != 1 {
        bail!("write one sentence of at most 25 words");
    }
    Ok(())
}

/// The open lanes that hold a slot: open or starting, and not finished. A
/// lane whose last agent state is `done` keeps its pane but holds no slot.
pub fn open_lane_count(project: &Project) -> usize {
    thread::list(project)
        .iter()
        .filter(|t| {
            (t.status == Status::Open || t.status == Status::Starting) && t.last_state != "done"
        })
        .count()
}

/// Creates the worktree or tab, the thread directory and the brief, then
/// returns. The agent is launched by the ticker, so there is one delivery path.
pub fn start(ctx: &Ctx, slug: &str, args: StartArgs) -> Result<Thread> {
    start_with_ticker(ctx, slug, args, ticker::start, None)
}

/// Starts a thread while `round advance` holds its lock. This must not wait for
/// a ticker replacement: the running ticker may itself be waiting for that
/// lock. Ordinary starts still replace a stale ticker through [`start`].
pub(crate) fn start_during_advance(ctx: &Ctx, slug: &str, args: StartArgs) -> Result<Thread> {
    start_with_ticker(ctx, slug, args, ticker::ensure, None)
}

pub(crate) fn start_during_advance_bounded(
    ctx: &Ctx,
    slug: &str,
    args: StartArgs,
    source_truncation: serde_json::Value,
) -> Result<Thread> {
    start_with_ticker(ctx, slug, args, ticker::ensure, Some(source_truncation))
}

fn start_with_ticker(
    ctx: &Ctx,
    slug: &str,
    args: StartArgs,
    ensure_ticker: fn(&Ctx<'_>) -> Result<()>,
    source_truncation: Option<serde_json::Value>,
) -> Result<Thread> {
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
    // Without a running ticker nothing launches. The advance path uses the
    // non-blocking ensure; every ordinary start replaces a stale ticker.
    ensure_ticker(ctx)?;
    let view = require_session(ctx, &project)?;

    let listed = args
        .repo
        .as_ref()
        .and_then(|repo| settings.repos.iter().find(|r| &r.path == repo));
    check_birth_plain(&args.plain)?;
    // A box lane needs a repository: no repository means a tab in this Mac's
    // project workspace, which is local (SPEC-remote §4.2).
    let explicit_remote = args
        .machine
        .as_deref()
        .is_some_and(|m| !m.is_empty() && m != crate::contracts::MACHINE_LOCAL);
    if args.repo.is_none() && explicit_remote {
        bail!(
            "a remote thread needs --repo: a task with no repository runs as a tab in the project's own workspace, which is local"
        );
    }
    let role = args
        .workflow
        .as_deref()
        .filter(|r| !r.is_empty())
        .unwrap_or("lane");
    if !matches!(
        role,
        "lane" | "reviewer" | "critic" | "drafter" | "research" | "planner"
    ) {
        bail!("workflow_unknown: `{role}` does not name a lane instruction set");
    }
    // A box lane still commits and pushes from the Mac clone, so every
    // explicit repository is a local path and follows the same allowlist,
    // local and box lanes alike (SPEC-remote §4.2).
    let repo = match &args.repo {
        None => String::new(),
        Some(repo) => {
            let path = std::fs::canonicalize(repo)
                .with_context(|| format!("repository {repo} does not exist"))?
                .to_string_lossy()
                .into_owned();
            if !crate::harness::allowed_repo(&settings, &ctx.config_dir, &path) {
                bail!(
                    "repo_not_listed: {path} is not listed in `repos` in PROJECT.md and is not a harness repository"
                );
            }
            path
        }
    };
    let launch = crate::launch::resolve_launch(
        ctx,
        &project,
        &crate::launch::ResolveInput {
            task: &args.task,
            workflow: role,
            state: crate::launch::repository_state(
                ctx,
                args.repo.as_deref(),
                args.base.as_deref(),
            )?,
            source_truncation: source_truncation.as_ref(),
            ..Default::default()
        },
    )?;
    // The machine is resolved before any tab or worktree exists (SPEC-remote
    // §4.1, d-0005). `--machine` wins and never falls back; a default box
    // start whose box cannot be used falls back to this Mac.
    let explicit_machine = args.machine.as_deref().filter(|m| !m.is_empty());
    let placement = match resolve_placement(
        ctx,
        explicit_machine,
        role,
        &launch,
        args.repo.as_deref(),
        listed,
    ) {
        Ok(placement) => placement,
        Err(error) => {
            crate::launch::ledger(
                &project,
                serde_json::json!({"kind":"placement-refused", "recipe":launch.recipe_id,
                    "explicit":explicit_machine, "error":format!("{error:#}")}),
            )?;
            return Err(error);
        }
    };
    crate::launch::ledger(
        &project,
        serde_json::json!({"kind":"placement", "recipe":launch.recipe_id,
            "machine":placement.ledger_machine(), "reason":placement.reason,
            "tried":placement.tried}),
    )?;
    if placement.fell_back {
        fallback_say(ctx, slug)?;
    }
    let machine = placement.machine.clone();
    let open_count = open_lane_count(&project);
    if open_count as u32 >= settings.max_parallel_threads {
        eprintln!(
            "warning: {open_count} threads are already open; max_parallel_threads is {}",
            settings.max_parallel_threads
        );
    }

    // Recipe readiness ran on the selected machine during placement. A box
    // start resolves its box repository row before a thread record
    // exists, so a missing piece refuses early and leaves no record behind
    // (t-0070).
    if !machine.is_empty() && !repo.is_empty() {
        box_repo_row(&settings, &repo)?;
    }
    let machine_id = placement.machine_id.clone();
    let record = thread::allocate(&project, |t| {
        t.title = args.title.trim().to_string();
        t.kind = if repo.is_empty() {
            Kind::Tab
        } else {
            Kind::Worktree
        };
        t.repo = repo.clone();
        t.machine = machine.clone();
        t.machine_id = machine_id.clone();
        t.agent = launch.kind.clone();
        t.base = args.base.clone().unwrap_or_default();
        t.role = role.to_string();
        t.plain = args.plain.trim().to_string();
        t.attempt = 1;
        t.launch = launch.clone();
    })?;
    let id = record.id.clone();
    {
        let _lock = project.lock()?;
        project::write_atomic(&thread::task_path(&project, &id), args.task.as_bytes())?;
    }

    match place_and_brief(ctx, &project, &view, &id, false) {
        Ok(thread) => {
            refresh_plan(ctx, &project);
            Ok(thread)
        }
        Err(error) => {
            let message = format!("{error:#}");
            let cleanup = fail_start(ctx, &project, &id, &message);
            let error = match cleanup {
                Ok(_) => error,
                Err(cleanup) => error.context(format!("failed-start cleanup: {cleanup:#}")),
            };
            Err(error.context(format!(
                "thread {id} failed to start; `thread restart {slug} {id}` retries"
            )))
        }
    }
}

/// The resolved machine of a new thread: empty for a local thread. The tried
/// rows are durable dispatch evidence, not presentation text inferred later.
#[derive(Debug, Clone, PartialEq, Default)]
struct Placement {
    machine: String,
    machine_id: String,
    reason: String,
    tried: Vec<serde_json::Value>,
    fell_back: bool,
}

impl Placement {
    fn ledger_machine(&self) -> &str {
        if self.machine.is_empty() {
            crate::contracts::MACHINE_LOCAL
        } else {
            &self.machine
        }
    }
}

/// The default machine of a start without `--machine` (SPEC-remote §4.1,
/// d-0005): lane/review work with a dispatch machine, on a
/// repository that has a box clone, runs on that box. Every other start
/// stays on this Mac.
fn default_machine(
    role: &str,
    role_machine: &str,
    repo: Option<&str>,
    listed: Option<&crate::project::Repo>,
) -> Option<String> {
    if !matches!(role, "lane" | "reviewer") || role_machine.is_empty() {
        return None;
    }
    let repo = repo?;
    let has_box = listed.is_some_and(|row| row.box_path.is_some())
        || crate::remote::box_repo_for(repo).is_some();
    has_box.then(|| role_machine.to_string())
}

/// Resolves a start's machine before any tab or worktree exists. Placement is
/// evaluated after the pick: the default box is tried first, then this Mac.
/// An explicit machine is the only candidate and is never silently changed.
fn resolve_placement(
    ctx: &Ctx,
    explicit: Option<&str>,
    role: &str,
    launch: &crate::contracts::Launch,
    repo: Option<&str>,
    listed: Option<&crate::project::Repo>,
) -> Result<Placement> {
    let candidates: Vec<String> = match explicit {
        Some(machine) => vec![machine.to_string()],
        None => match default_machine(role, &launch.machine, repo, listed) {
            Some(machine) if machine == crate::contracts::MACHINE_LOCAL => vec![machine],
            Some(machine) => vec![machine, crate::contracts::MACHINE_LOCAL.to_string()],
            None => vec![crate::contracts::MACHINE_LOCAL.to_string()],
        },
    };
    let mut tried = Vec::new();
    for candidate in candidates {
        let checked = if candidate == crate::contracts::MACHINE_LOCAL {
            crate::doctor::recipe_ready_local(ctx, launch)
                .map(|_| None)
                .map_err(|error| format!("{error:#}"))
        } else {
            match remote::machine_profile(
                ctx.runner,
                &ctx.env.herdr_bin(),
                &ctx.config_dir,
                &candidate,
            ) {
                Err(error) => Err(format!("{error:#}")),
                Ok(profile) if profile.is_local() => crate::doctor::recipe_ready_local(ctx, launch)
                    .map(|_| None)
                    .map_err(|error| format!("{error:#}")),
                Ok(profile) if project::machine_held(&ctx.root, &profile.id) => Err(format!(
                    "machine_held: `{}` is held; run `ha machine release {}` when the fork refresh or resize is done",
                    profile.label, profile.label
                )),
                Ok(profile) => crate::doctor::recipe_ready_on_box(ctx, &profile, launch)
                    .map(|_| Some(profile))
                    .map_err(|error| format!("{error:#}")),
            }
        };
        match checked {
            Ok(profile) => {
                let chosen = profile
                    .as_ref()
                    .map(|profile| profile.label.as_str())
                    .unwrap_or(crate::contracts::MACHINE_LOCAL);
                tried.push(serde_json::json!({"machine":chosen, "ready":true}));
                let reason = if tried.len() == 1 {
                    format!("recipe `{}` is ready on `{chosen}`", launch.recipe_id)
                } else {
                    let earlier = tried[..tried.len() - 1]
                        .iter()
                        .map(|row| {
                            format!(
                                "{}: {}",
                                row["machine"].as_str().unwrap_or("unknown"),
                                row["missing"].as_str().unwrap_or("not ready")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    format!(
                        "recipe `{}` runs on `{chosen}` because {earlier}",
                        launch.recipe_id
                    )
                };
                return Ok(Placement {
                    machine: profile
                        .as_ref()
                        .map(|p| p.label.clone())
                        .unwrap_or_default(),
                    machine_id: profile.map(|p| p.id).unwrap_or_default(),
                    reason,
                    fell_back: explicit.is_none() && tried.len() > 1,
                    tried,
                });
            }
            Err(missing) => tried.push(serde_json::json!({
                "machine":candidate, "ready":false, "missing":missing
            })),
        }
    }
    let details = tried
        .iter()
        .map(|row| {
            format!(
                "{}: {}",
                row["machine"].as_str().unwrap_or("unknown"),
                row["missing"].as_str().unwrap_or("not ready")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    bail!(
        "recipe_unavailable: recipe `{}` cannot run; tried {details}",
        launch.recipe_id
    )
}

/// The one plain line when a default box start falls back to this Mac
/// (SPEC-remote §4.1, d-0005).
fn fallback_say(ctx: &Ctx, slug: &str) -> Result<()> {
    crate::ask::say(
        ctx,
        slug,
        "the box was not ready, so this lane runs here",
        None,
    )
}

/// A `kind = "pi"` launch is refused unless its provider is ready (SPEC-pi
/// §3.4, T11): never a lane that waits for a first prompt it cannot answer.
pub fn pi_ready(ctx: &Ctx, launch: &crate::contracts::Launch) -> Result<()> {
    let provider = crate::pi::launch::flag_value(&launch.args, "--provider")
        .context("pi_args_forbidden: a pi launch names no --provider")?;
    crate::pi_ade::check_with(ctx.runner, &ctx.root, &provider)
        .map(|_| ())
        .with_context(|| format!("pi_not_ready: provider {provider}"))
}

/// Recipe readiness on a box is owned by the doctor probes.
fn box_launch_ready(
    ctx: &Ctx,
    profile: &crate::contracts::MachineProfile,
    launch: &crate::contracts::Launch,
) -> Result<()> {
    crate::doctor::recipe_ready_on_box(ctx, profile, launch)
        .with_context(|| format!("recipe `{}` on `{}`", launch.recipe_id, profile.label))
}

/// Box pi readiness: the check runs on the box, never against the Mac login
/// (SPEC-remote §4.1, SPEC-pi §3.4).
pub fn box_pi_ready(ctx: &Ctx, machine: &str, launch: &crate::contracts::Launch) -> Result<()> {
    let profile =
        remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, machine)?;
    box_launch_ready(ctx, &profile, launch)
}

/// Steps 2 to 5 of starting a thread, also used by `thread restart` case (a).
fn place_and_brief(
    ctx: &Ctx,
    project: &Project,
    view: &SessionView,
    id: &str,
    restart: bool,
) -> Result<Thread> {
    let record = thread::load(project, id)?;

    let placed = match record.kind {
        Kind::Worktree if record.is_remote() => {
            place_box_worktree(ctx, project, view, &record, restart)?
        }
        Kind::Worktree => place_ade_worktree(ctx, project, view, &record)?,
        Kind::Tab if record.is_remote() => {
            bail!("a box lane needs a repository; a task with no repository runs on this Mac")
        }
        Kind::Tab => place_ade_tab(ctx, project, view, &record)?,
        Kind::Adopted => bail!("an adopted thread is not placed by the binary"),
    };
    write_brief(ctx, project, &placed)?;
    finish_placement(project, view, id)
}

/// The thread directory is local bookkeeping; on a box lane the committed
/// brief travels by git (D9) and there is nothing to write here.
fn write_brief(ctx: &Ctx, project: &Project, placed: &Thread) -> Result<()> {
    if placed.is_remote() {
        return Ok(());
    }
    prepare_local_dir(ctx, project, placed)
}

/// The box clone path and publish URL a box start needs for `repo`, each
/// reported by name when it is missing. A row in `PROJECT.md` whose
/// `box_path` is set but whose `publish_url` is not names the missing URL; a
/// repo with neither falls back to the built-in harness map, or keeps the
/// mapping message (t-0070).
fn box_repo_row(settings: &crate::project::Settings, repo: &str) -> Result<(String, String)> {
    let row = settings.repos.iter().find(|r| r.path == repo);
    match (
        row.and_then(|r| r.box_path.clone()),
        row.and_then(|r| r.publish_url.clone()),
    ) {
        (Some(box_path), Some(publish_url)) => Ok((box_path, publish_url)),
        (Some(_), None) => bail!(
            "box_publish_url_missing: `{repo}` has a box_path in PROJECT.md but no `publish_url` in that row; add the URL the box fetches the lane branch from (the remote the branch is pushed to) before the first box start"
        ),
        (None, Some(_)) => bail!(
            "box_path_missing: `{repo}` has a publish_url in PROJECT.md but no `box_path` in that row; add the box clone path before the first box start"
        ),
        (None, None) => {
            let map = crate::remote::box_repo_for(repo).with_context(|| {
                format!(
                    "box_repo_unmapped: {repo} has no Mac-to-box row; add one before the first box start"
                )
            })?;
            Ok((map.box_path.to_string(), map.publish_url.to_string()))
        }
    }
}

/// The box start side (SPEC-remote §4.2 steps 2–5): commit the brief `B` on
/// the Mac integration branch, push only the lane branch to the URL-matched
/// remote, one ssh call to fetch and create the box worktree, create the box
/// tab through machine routing, then write the lane card. The brief is never
/// copied; it travels by git (D9).
fn place_box_worktree(
    ctx: &Ctx,
    project: &Project,
    view: &SessionView,
    record: &Thread,
    restart: bool,
) -> Result<Thread> {
    let runner = ctx.runner;
    let (settings, _) = project.read_project_md()?;
    let label = crate::project::display_name(&settings.name, &project.slug);
    let (box_repo, publish_url) = box_repo_row(&settings, &record.repo)?;
    // Both clones must name the configured publish URL. The push still uses
    // the URL itself; finding the matching remote only validates this clone.
    let _ = remote::remote_for_url(runner, &record.repo, &publish_url)?;
    let profile = remote::machine_profile(
        runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?;
    let target = profile.target.clone();
    let box_worktree = format!("{box_repo}/.worktrees/{}", record.id);
    let branch = if record.branch.is_empty() {
        thread::branch_name(&project.slug, &record.id, &record.title)
    } else {
        record.branch.clone()
    };
    let dir = thread::thread_dir(&box_worktree, &project.slug, &record.id);

    // A restart reuses the brief commit already on the record; a first start
    // commits it (D9). The brief is never rewritten.
    let reusable = restart && !record.base.is_empty() && !record.launch.brief_hash.is_empty();
    let (base, brief_hash) = if reusable {
        if record.failure_event.is_empty() {
            push_branch(runner, &record.repo, &publish_url, &branch, &record.base)?;
        }
        (record.base.clone(), record.launch.brief_hash.clone())
    } else {
        let task =
            std::fs::read_to_string(thread::task_path(project, &record.id)).unwrap_or_default();
        let stub = Thread {
            thread_dir: dir.clone(),
            ..record.clone()
        };
        let brief = thread::brief_for(project, &stub, &task, restart)?;
        let committed = format!("plain: {}\n\n{brief}", record.plain);
        let rel = format!("tasks/{}.md", record.id);
        let brief_hash = thread::sha256_hex(committed.as_bytes());
        let integration = integration_branch(runner, record)?;
        let repo_lock = crate::git::lock(runner, &record.repo)?;
        if let Err(error) = crate::git::exclude_plugin_paths_locked(runner, &record.repo) {
            eprintln!("warning: {error:#}");
        }
        let head =
            crate::git::rev_parse(runner, &record.repo, &format!("refs/heads/{integration}"))?;
        let sha = crate::git::commit_files_locked(
            runner,
            Path::new(&record.repo),
            &integration,
            &[(rel.as_str(), committed.as_str())],
            &format!("docs(tasks): {}", record.id),
            &head,
            &repo_lock.common_dir.join("herdr-ade-tmp"),
        )?;
        ensure_branch(runner, &record.repo, &branch, &sha)?;
        drop(repo_lock);
        push_branch(runner, &record.repo, &publish_url, &branch, &sha)?;
        (sha, brief_hash)
    };

    thread::update(project, &record.id, |t| {
        t.base = base.clone();
        t.branch = branch.clone();
        t.worktree_path = box_worktree.clone();
        t.thread_dir = dir.clone();
        t.launch.brief_hash = brief_hash.clone();
        t.partial = Some("worktree_add".into());
    })?;

    // Starts for one box repository serialize on the Mac (SPEC-remote §4.2
    // step 3).
    let _box_lock = project::box_lock(&ctx.root, &profile.id, &box_repo)?;

    if record.failure_event.is_empty() {
        remote::provision(
            runner,
            &target,
            &remote::Provision {
                box_repo: &box_repo,
                worktree: &box_worktree,
                branch: &branch,
                base: &base,
                publish_url: &publish_url,
            },
        )?;
    }

    // Step 4: route by the stable profile id. `workspace create` already
    // creates a first tab, so that pane is the lane instead of adding a
    // second tab beside an unused shell. A restart always gets a fresh
    // workspace; it never adopts whatever survived an earlier attempt.
    let herdr = view.herdr.on_machine(&profile.id);
    let spec = crate::contracts::RoleSpec {
        kind: record.launch.kind.clone(),
        args: record.launch.args.clone(),
        env: record.launch.env.clone(),
        ready_timeout_ms: record.launch.ready_timeout_ms,
    };
    let attempt = record.attempt.max(1);
    let env = project::tab_env(
        &project.slug,
        &record.id,
        attempt,
        &brief_hash,
        &record.machine,
        &spec,
    );
    // One project owns one workspace on this machine. Starts can provision
    // repositories independently, but find-or-create is serialized so two
    // simultaneous lanes cannot both observe "missing" and create duplicates.
    let _workspace_lock = project::remote_workspace_lock(&ctx.root, &project.slug, &profile.id)?;
    let matching: Vec<_> = herdr
        .workspace_list()
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .into_iter()
        .filter(|workspace| workspace.label == label)
        .collect();
    if matching.len() > 1 {
        bail!(
            "remote_workspace_duplicate: machine `{}` has {} workspaces labelled `{label}`",
            record.machine,
            matching.len()
        );
    }
    let (created, first_tab) = match matching.first() {
        Some(workspace) => (
            herdr
                .tab_create_env(
                    &workspace.workspace_id,
                    Path::new(&box_worktree),
                    &record.id,
                    false,
                    &env,
                )
                .map_err(|error| anyhow::anyhow!("{error}"))?,
            false,
        ),
        None => (
            herdr
                .workspace_create_env(Path::new(&box_worktree), &label, false, &env)
                .map_err(|error| anyhow::anyhow!("{error}"))?,
            true,
        ),
    };
    let cwd = herdr
        .pane_cwd(&created.pane_id)
        .unwrap_or_else(|_| box_worktree.clone());
    let cwd = if cwd.is_empty() {
        box_worktree.clone()
    } else {
        cwd
    };
    // Persist ownership before any later call can fail. Failed-start cleanup
    // can now close this exact workspace instead of leaking an unrecorded one.
    thread::update(project, &record.id, |t| {
        t.cwd = cwd.clone();
        t.workspace_id = created.workspace_id.clone();
        t.tab_id = created.tab_id.clone();
        t.pane_id = created.pane_id.clone();
        t.partial = Some("lane_card".into());
    })?;
    if first_tab {
        herdr
            .tab_rename(&created.tab_id, &record.id)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    // The box lane nests under its coordinator from its first second: the
    // machine-qualified parent token is written before the ticker starts the
    // agent, which passes no `--parent` for a box lane.
    if let Some(coord) = project.coordinator() {
        herdr
            .pane_set_parent(&created.pane_id, &parent_token(record, &coord.pane_id))
            .map_err(|error| {
                anyhow::anyhow!(
                    "could not link box pane {} to its coordinator: {error}",
                    created.pane_id
                )
            })?;
    }

    // Step 5: the lane card, now that the pane id exists.
    let recipient = project
        .coordinator()
        .map(|c| {
            let attempt = c.attempt();
            crate::contracts::Recipient {
                pane: c.pane_id,
                coordinator_attempt: attempt,
            }
        })
        .unwrap_or_default();
    let start_line = thread::launch_prompt(
        "",
        &project.slug,
        &Thread {
            machine: record.machine.clone(),
            ..record.clone()
        },
    );
    let card = crate::contracts::LaneCard {
        project: project.slug.clone(),
        thread: record.id.clone(),
        attempt,
        brief_hash: brief_hash.clone(),
        role: record.role.clone(),
        kind: record.launch.kind.clone(),
        pane_id: created.pane_id.clone(),
        machine_label: record.machine.clone(),
        machine_id: record.machine_id.clone(),
        box_repo: box_repo.clone(),
        box_worktree: box_worktree.clone(),
        brief_commit: base.clone(),
        branch: branch.clone(),
        publish_url: publish_url.clone(),
        recipient,
        start_line,
        created: crate::project::now(),
    };
    let card_path = crate::contracts::box_lane_card(&project.slug, &record.id);
    remote::provision_card(
        runner,
        &target,
        &project.slug,
        &card_path,
        &toml::to_string(&card)?,
    )?;

    thread::update(project, &record.id, |t| {
        t.cwd = cwd.clone();
        t.worktree_path = box_worktree.clone();
        t.workspace_id = created.workspace_id.clone();
        t.tab_id = created.tab_id.clone();
        t.pane_id = created.pane_id.clone();
        t.partial = None;
    })
}

/// The integration branch the brief commits on: `--base`, else the branch the
/// repository has checked out (D9).
fn integration_branch(runner: &dyn Runner, record: &Thread) -> Result<String> {
    if !record.base.is_empty() {
        return Ok(record.base.clone());
    }
    git(
        runner,
        &record.repo,
        &["symbolic-ref", "--short", "HEAD"],
        GIT_TIMEOUT,
    )
    .context(
        "integration_branch_required: the repository is on a detached HEAD; pass --base <branch>",
    )
}

/// Creates the lane branch at `sha`, tolerating a retry that left it at the
/// same commit. Never moves an existing ref (D9).
fn ensure_branch(runner: &dyn Runner, repo: &str, branch: &str, sha: &str) -> Result<()> {
    if let Some(existing) = crate::git::branch_head(runner, repo, branch)? {
        if existing == sha {
            return Ok(());
        }
        bail!("lane branch {branch} already exists at {existing}, not {sha}");
    }
    let out =
        runner.run(&Cmd::new("git", GIT_TIMEOUT).args(["-C", repo, "branch", branch, sha]))?;
    if !out.success() {
        bail!("git branch {branch}: {}", out.error_text());
    }
    Ok(())
}

/// Pushes the lane branch by URL, never by remote name and never with force
/// (SPEC-remote §4.2 step 2).
fn push_branch(runner: &dyn Runner, repo: &str, url: &str, branch: &str, sha: &str) -> Result<()> {
    let out = runner.run(&Cmd::new("git", Duration::from_secs(60)).args([
        "-C",
        repo,
        "push",
        "--quiet",
        url,
        &format!("{sha}:refs/heads/{branch}"),
    ]))?;
    if !out.success() {
        bail!("push of {branch} to {url}: {}", out.error_text());
    }
    Ok(())
}

/// SPEC-ADE D4 and D9, in order: under one repository lock, keep the plugin's
/// folders out of git, commit the brief `tasks/<id>.md` on the integration
/// branch (item 55), record its hash, then `git worktree add` the lane from
/// that commit so the brief is in its checkout; then the tab, whose
/// `HERDR_ADE_LAUNCH` carries the same hash as the record.
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
    // The integration branch is a local branch: `--base`, else the branch
    // the repository has checked out. A remote-tracking ref or a bare sha
    // cannot take the brief commit (D9).
    let integration = if record.base.is_empty() {
        git(
            runner,
            &record.repo,
            &["symbolic-ref", "--short", "HEAD"],
            GIT_TIMEOUT,
        )
        .context("integration_branch_required: the repository is on a detached HEAD; pass --base <branch>")?
    } else {
        record.base.clone()
    };
    if crate::git::branch_head(runner, &record.repo, &integration)?.is_none() {
        return Err(crate::refusal::error(format!(
            "integration_branch_required: `{integration}` is not a local branch"
        )));
    }
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
    let brief_hash = thread::sha256_hex(committed.as_bytes());

    let placed = {
        let repo_lock = crate::git::lock(runner, &record.repo)?;
        if let Err(error) = crate::git::exclude_plugin_paths_locked(runner, &record.repo) {
            eprintln!("warning: {error:#}");
        }
        let head =
            crate::git::rev_parse(runner, &record.repo, &format!("refs/heads/{integration}"))?;
        let sha = crate::git::commit_files_locked(
            runner,
            Path::new(&record.repo),
            &integration,
            &[(rel.as_str(), committed.as_str())],
            &format!("docs(tasks): {}", record.id),
            &head,
            &repo_lock.common_dir.join("herdr-ade-tmp"),
        )?;
        // The hash is on the record before the lane branch exists (D9).
        thread::update(project, &record.id, |t| {
            t.launch.brief_hash = brief_hash.clone();
            t.thread_dir = stub.thread_dir.clone();
            t.base = sha.clone();
            t.branch = branch.clone();
            t.partial = Some("worktree_add".into());
        })?;
        let path = crate::git::worktree_add(runner, &record.repo, &record.id, &branch, &sha)?;
        (sha, path)
    };
    let (sha, path) = placed;
    let cwd = path.to_string_lossy().into_owned();
    thread::update(project, &record.id, |t| {
        t.worktree_path = cwd.clone();
        t.partial = Some("tab_create".into());
    })?;
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
        "",
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
                t.origin = origin;
                t.cwd = cwd;
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
    // No repository, so no committed brief: the brief is written once into
    // the thread directory and its hash fixed before the tab exists.
    let brief_hash = if record.launch.brief_hash.is_empty() {
        let task =
            std::fs::read_to_string(thread::task_path(project, &record.id)).unwrap_or_default();
        let dir = thread::thread_dir(&folder.to_string_lossy(), &project.slug, &record.id);
        let stub = Thread {
            thread_dir: dir.clone(),
            ..record.clone()
        };
        let brief = format!(
            "plain: {}\n\n{}",
            record.plain,
            thread::brief_for(project, &stub, &task, false)?
        );
        std::fs::create_dir_all(&dir).with_context(|| format!("could not create {dir}"))?;
        project::write_atomic(&Path::new(&dir).join("brief.md"), brief.as_bytes())?;
        let hash = thread::sha256_hex(brief.as_bytes());
        thread::update(project, &record.id, |t| {
            t.launch.brief_hash = hash.clone();
            t.thread_dir = dir;
        })?;
        hash
    } else {
        record.launch.brief_hash.clone()
    };
    let env = project::tab_env(
        &project.slug,
        &record.id,
        record.attempt.max(1),
        &brief_hash,
        "",
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

/// Creates the recorded thread directory with its library and keeps it out
/// of git.
fn prepare_local_dir(ctx: &Ctx, project: &Project, placed: &Thread) -> Result<()> {
    let dir = if placed.thread_dir.is_empty() {
        thread::thread_dir(&placed.cwd, &project.slug, &placed.id)
    } else {
        placed.thread_dir.clone()
    };
    std::fs::create_dir_all(Path::new(&dir).join("library"))
        .with_context(|| format!("could not create {dir}"))?;
    if placed.kind != Kind::Tab {
        exclude_from_git(ctx.runner, &placed.cwd)?;
    }
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
    let thread = thread::update_checked(project, id, |t| {
        if t.status == Status::Resolved {
            bail!("placement_stale: thread was resolved during placement");
        }
        t.agent_name = thread::agent_name(&project.slug, &t.id);
        t.prompt_pending = true;
        t.launch_attempts = 0;
        t.escalation_pending = false;
        t.status = Status::Open;
        t.error.clear();
        t.last_state.clear();
        t.last_state_change = project::now();
        Ok(())
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
    let herdr = view.herdr.on_machine(record.machine_route());
    let unreachable = |e: crate::herdr::HerdrError| {
        anyhow::anyhow!("machine `{}` is unreachable: {e}", record.machine)
    };
    Ok((
        herdr.agent_list().map_err(unreachable)?,
        herdr.pane_list().map_err(unreachable)?,
    ))
}

/// Resume a persisted escalation without re-picking or touching worktree files.
pub fn place_escalation(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if record.launch_attempts >= thread::MAX_LAUNCH_ATTEMPTS {
        thread::update(project, &record.id, |t| {
            t.escalation_pending = false;
            t.error = "escalation_placement_exhausted: could not open the replacement lane".into();
        })?;
        bail!("escalation_placement_exhausted");
    }
    thread::update(project, &record.id, |t| t.launch_attempts += 1)?;
    let view = require_session(ctx, project)?;
    let herdr = view.herdr.on_machine(record.machine_route());
    if !record.tab_id.is_empty() {
        // Only this lane's failed attempt is stopped, never a coordinator.
        let panes = herdr.pane_list()?;
        if let Some(pane) = panes.iter().find(|p| p.pane_id == record.pane_id) {
            if !thread::pane_matches(record, pane) {
                bail!("escalation_identity_mismatch: old pane was reused");
            }
            close_pane(ctx, project, record)?;
        }
    }
    if record.is_remote() {
        let profile = remote::machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            record.machine_route(),
        )?;
        box_launch_ready(ctx, &profile, &record.launch)?;
        place_and_brief(ctx, project, &view, &record.id, true)?;
    } else {
        if record.launch.kind == "pi" {
            pi_ready(ctx, &record.launch)?;
        }
        place_ade_tab(ctx, project, &view, record)?;
        finish_placement(project, &view, &record.id)?;
    }
    thread::update(project, &record.id, |t| t.escalation_pending = false)?;
    Ok(())
}

pub fn restart(ctx: &Ctx, slug: &str, id: &str) -> Result<Thread> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    ticker::start(ctx)?;
    let view = require_session(ctx, &project)?;
    let (agents, panes) = lists_for(&view, &record)?;
    let now = jiff::Timestamp::now();
    let live = thread::live_state(&record, &agents, &panes, now);
    let role = if record.role.is_empty() {
        "lane"
    } else {
        record.role.as_str()
    };
    let skill_hash = thread::sha256_hex(crate::lane::skill_text(role).as_bytes());
    if record.is_remote() {
        // A box lane restarts from its brief: the same refusals, then the
        // exact start line again (SPEC-remote §6). The new attempt gets its
        // own workspace; the brief commit is reused, never rewritten.
        let _ = restart_plan(&record, &live, false, now)?;
        thread::update(&project, id, |t| {
            t.attempt = t.attempt.max(1).saturating_add(1);
            t.launch.attempt = t.attempt;
            t.launch.skill_hash = skill_hash.clone();
        })?;
        if !record.tab_id.is_empty() {
            close_pane(ctx, &project, &record)?;
        }
        return place_and_brief(ctx, &project, &view, id, true);
    }
    let branch_exists = record.kind == Kind::Worktree && record.worktree_path.is_empty() && {
        let branch = thread::branch_name(slug, id, &record.title);
        crate::git::branch_head(ctx.runner, &record.repo, &branch)?.is_some()
    };

    let plan = restart_plan(&record, &live, branch_exists, now)?;
    thread::update(&project, id, |t| {
        t.attempt = t.attempt.max(1).saturating_add(1);
        t.launch.attempt = t.attempt;
        t.launch.skill_hash = skill_hash.clone();
    })?;
    match plan {
        RestartPlan::Create => return place_and_brief(ctx, &project, &view, id, true),
        RestartPlan::ReusePane | RestartPlan::Reopen => {
            // The live pane is a bare shell whose HERDR_ADE_LAUNCH names the
            // previous attempt; the new attempt gets its own tab (D14).
            if plan == RestartPlan::ReusePane {
                view.herdr
                    .tab_close(&record.tab_id)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            place_ade_tab(ctx, &project, &view, &thread::load(&project, id)?)?;
        }
    }
    let placed = thread::load(&project, id)?;
    write_brief(ctx, &project, &placed)?;
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
        .on_machine(record.machine_route())
        .agent_prompt(&record.pane_id, text.trim())
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    // A pi lane reports `blocked` for its own recorded error; a prompt
    // resumes it, so that error is cleared.
    if record.agent == "pi" && state == "blocked" && !record.error.is_empty() {
        thread::update(&project, id, |t| t.error.clear())?;
    }
    Ok(state)
}

/// The state a follow-up may be sent in, or the refusal.
pub fn prompt_state(record: &Thread, agents: &[Agent]) -> Result<String> {
    let agent = agents
        .iter()
        .find(|a| thread::agent_matches(record, a))
        .with_context(|| format!("no agent is detected in {}'s pane; text is never typed at a bare shell prompt (try `thread restart`)", record.id))?;
    match agent.agent_status.as_str() {
        "blocked" if record.agent != "pi" || record.error.is_empty() => bail!(
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
    /// Leave the lane's pane and tab open (the idle agent still runs).
    pub keep_pane: bool,
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
        refresh_plan(ctx, &project);
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
    crate::round::require_resolvable(&project, id)?;

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
    let pane_closed = if args.keep_pane {
        false
    } else {
        close_pane(ctx, &project, &resolved)?
    };
    remove_scratch_session(ctx, &resolved)?;
    println!("{id} resolved.");
    if args.keep_pane {
        println!(
            "Its pane and tab were left open (--keep-pane); close them in herdr when you are done."
        );
    } else if pane_closed {
        println!("Its pane and tab were closed.");
    } else {
        println!("Its pane and tab were already gone.");
    }
    if !args.remove_worktree {
        match resolved.kind {
            Kind::Worktree if resolved.worktree_path.is_empty() => {
                println!("No worktree was recorded for it, so there is nothing to close or remove.")
            }
            Kind::Worktree => println!(
                "Its worktree ({}) and branch ({}) were left alone. Run `thread resolve {slug} {id} --remove-worktree` to remove the worktree.",
                resolved.worktree_path, resolved.branch
            ),
            _ => {}
        }
    } else {
        println!(
            "The worktree {} was removed; the branch {} was kept.",
            record.worktree_path, resolved.branch
        );
    }
    refresh_plan(ctx, &project);
    Ok(())
}

/// The shared plan refresh at a thread lifecycle change. A refresh failure is
/// reported on its own line; it never fails the lifecycle operation
/// (SPEC-talk §6.5).
pub fn refresh_plan(ctx: &Ctx, project: &Project) {
    if let Err(e) = crate::plan::refresh(ctx, project) {
        eprintln!("note: the plan refresh failed: {e:#}");
    }
}

/// Deletes a lane's isolated throwaway session if it exists. Scratch work is
/// never hosted in the watched project session, locally or on a lane machine.
pub(crate) fn remove_scratch_session(ctx: &Ctx, record: &Thread) -> Result<()> {
    let name = format!("scratch-{}", record.id);
    let bin = ctx.env.herdr_bin();
    if !record.is_remote() {
        let sessions = crate::herdr::session_list(&bin, ctx.runner)?;
        let Some(session) = sessions.iter().find(|session| session.name == name) else {
            return Ok(());
        };
        if session.running {
            let out = ctx.runner.run(
                &Cmd::new(&bin, Duration::from_secs(20)).args(["session", "stop", &name, "--json"]),
            )?;
            if !out.success() {
                bail!(
                    "could not stop scratch session {name}: {}",
                    out.error_text()
                );
            }
        }
        let out = ctx.runner.run(
            &Cmd::new(&bin, Duration::from_secs(20)).args(["session", "delete", &name, "--json"]),
        )?;
        if !out.success() {
            bail!(
                "could not delete scratch session {name}: {}",
                out.error_text()
            );
        }
        return Ok(());
    }

    let profile =
        remote::machine_profile(ctx.runner, &bin, &ctx.config_dir, record.machine_route())?;
    let quoted = remote::quote(&name);
    let script = format!(
        "state=$(herdr session list --json | python3 -c 'import json,sys; n=sys.argv[1]; rows=json.load(sys.stdin).get(\"sessions\",[]); r=next((r for r in rows if r.get(\"name\")==n),None); print(\"missing\" if r is None else (\"running\" if r.get(\"running\") else \"stopped\"))' {quoted}); case \"$state\" in running) herdr session stop {quoted} --json >/dev/null; herdr session delete {quoted} --json >/dev/null;; stopped) herdr session delete {quoted} --json >/dev/null;; missing) :;; *) exit 1;; esac"
    );
    let out = remote::ssh(
        ctx.runner,
        &profile.target,
        &script,
        None,
        Duration::from_secs(30),
    )?;
    if !out.success() {
        bail!(
            "could not delete scratch session {name} on {}: {}",
            record.machine,
            out.error_text()
        );
    }
    Ok(())
}

/// Marks a start failed, removes its Working metadata, and closes everything
/// the attempt opened. The failed state is durable even when cleanup itself
/// reports an error, so no view can keep presenting the attempt as Working.
pub(crate) fn fail_start(ctx: &Ctx, project: &Project, id: &str, reason: &str) -> Result<Thread> {
    let failed = thread::update(project, id, |t| {
        t.status = Status::Failed;
        t.prompt_pending = false;
        t.error = reason.to_string();
        t.last_group = Group::WaitingOnYou.token().to_string();
    })?;
    if !failed.tab_id.is_empty() {
        let view = session_view(ctx, project)
            .context("failed-start cleanup could not reach the session; the attempt stays bound")?;
        clear_thread_tokens(&view.herdr, &failed);
    }
    close_pane(ctx, project, &failed)?;
    Ok(failed)
}

/// Close the thread's pane and tab. A dedicated lane workspace is closed as
/// one unit, but a workspace containing another pane, tab, or agent is shared
/// and only this lane's tab is closed. A tab herdr no longer knows, or a
/// session it cannot reach, has nothing to close and is not an error.
pub(crate) fn close_pane(ctx: &Ctx, project: &Project, record: &Thread) -> Result<bool> {
    if record.tab_id.is_empty() {
        return Ok(false);
    }
    let Some(view) = session_view(ctx, project) else {
        return Ok(false);
    };
    let herdr = view.herdr.on_machine(record.machine_route());
    let panes = herdr
        .pane_list()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let agents = herdr
        .agent_list()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let owns_pane = panes.iter().any(|pane| thread::pane_matches(record, pane));
    let holds_something_else = panes
        .iter()
        .any(|pane| pane.workspace_id == record.workspace_id && pane.pane_id != record.pane_id)
        || agents.iter().any(|agent| {
            agent.workspace_id == record.workspace_id && !thread::agent_matches(record, agent)
        });
    // A local project workspace remains owned by its coordinator even when
    // that coordinator's pane is temporarily absent. An adopted workspace was
    // not created by ADE, so resolving its thread must not close it either.
    let is_project_workspace = !record.is_remote()
        && project
            .coordinator()
            .is_some_and(|coordinator| coordinator.workspace_id == record.workspace_id);
    let owns_workspace = record.kind != Kind::Adopted && !is_project_workspace;
    let result = if owns_workspace && owns_pane && !holds_something_else {
        herdr.workspace_close(&record.workspace_id)
    } else {
        herdr.tab_close(&record.tab_id)
    };
    match result {
        Ok(()) => Ok(true),
        Err(error) if matches!(error.code.as_str(), "tab_not_found" | "workspace_not_found") => {
            Ok(false)
        }
        Err(error) => Err(anyhow::anyhow!("{error}")),
    }
}

/// The final report and library copy, storing the new report hash. A box
/// lane's report and library arrive through the Mac courier (the second lane),
/// never through a second copy path; until then the copy is partial.
pub fn final_copy(ctx: &Ctx, project: &Project, record: &Thread) -> thread::Copied {
    let copied = if record.is_remote() {
        imported_report(project, record)
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

/// A box lane's report arrives as the courier's imported artifact
/// (SPEC-remote §4.3). Once the current attempt has a sealed `done` whose
/// artifact is on the Mac and hashes to its name, the copy is complete: no box
/// path is copied and there is no second transport. The D4 removal gate reads
/// this same artifact.
fn imported_report(project: &Project, record: &Thread) -> thread::Copied {
    let attempt = record.attempt.max(1);
    let hash = crate::events::list(project)
        .into_iter()
        .filter(|event| event.thread == record.id && event.attempt == attempt)
        .find_map(|event| event.payload.done)
        .map(|done| done.artifact);
    match hash {
        Some(hash) => match std::fs::read(crate::events::artifact_path(project, &hash)) {
            Ok(bytes) if thread::sha256_hex(&bytes) == hash => thread::Copied {
                outcome: CopyOutcome::Complete,
                report_hash: Some(hash),
            },
            _ => thread::Copied {
                outcome: CopyOutcome::Partial(vec![format!(
                    "the sealed report artifact {hash} is not on the Mac yet"
                )]),
                report_hash: None,
            },
        },
        None => thread::Copied {
            outcome: CopyOutcome::Partial(vec![
                "a box lane's report arrives through the Mac courier; no sealed done yet".into(),
            ]),
            report_hash: None,
        },
    }
}

/// Never forces. herdr's or git's refusal (for example uncommitted changes) is
/// reported unchanged.
fn remove_worktree(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if !record.is_remote() {
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
            .on_machine(record.machine_route())
            .worktree_remove(&record.workspace_id)
            .map_err(|error| anyhow::anyhow!("{error}"));
    }
    let target = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?
    .target;
    let script = format!(
        "cd {} && git worktree remove {}",
        remote::quote(&record.repo),
        remote::quote(&record.worktree_path)
    );
    let out = remote::ssh(ctx.runner, &target, &script, None, Duration::from_secs(20))?;
    if !out.success() {
        bail!("{}", out.error_text());
    }
    Ok(())
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
    removal_gate(project, &view.herdr, record, &panes)?;
    if let Err(error) = crate::git::worktree_remove(ctx.runner, &record.repo, &record.worktree_path)
    {
        let _ = thread::update(project, &record.id, |t| {
            t.partial = Some("worktree_remove".into());
        });
        return Err(error);
    }
    let own_tab = panes
        .iter()
        .any(|p| p.tab_id == record.tab_id && p.pane_id == record.pane_id);
    if own_tab && let Err(error) = view.herdr.tab_close(&record.tab_id) {
        let _ = thread::update(project, &record.id, |t| {
            t.partial = Some("tab_close".into());
        });
        return Err(anyhow::anyhow!("{error}"));
    }
    Ok(())
}

/// The D4 gate before a lane's worktree goes: the lane released it (a sealed
/// `done` for the current attempt whose artifact still hashes to its name,
/// or the thread was resolved before), no program runs in it, and the tab
/// that would close is the one the record created.
fn removal_gate(
    project: &Project,
    herdr: &Herdr,
    record: &Thread,
    panes: &[crate::herdr::Pane],
) -> Result<()> {
    let attempt = record.attempt.max(1);
    let done = crate::events::list(project)
        .into_iter()
        .filter(|e| e.thread == record.id && e.attempt == attempt)
        .find_map(|e| e.payload.done);
    match done {
        Some(done) => {
            let path = crate::round::artifacts_dir(project).join(&done.artifact);
            let bytes = std::fs::read(&path).map_err(|e| {
                anyhow::anyhow!("artifact_missing: {} ({e}); not removing", path.display())
            })?;
            if thread::sha256_hex(&bytes) != done.artifact {
                bail!(
                    "artifact_mismatch: {} does not hash to its name; not removing",
                    path.display()
                );
            }
        }
        None if record.status == Status::Resolved => {}
        None => bail!(
            "worktree_not_released: {} has no sealed done for attempt {attempt}; not removing",
            record.id
        ),
    }
    for pane in panes
        .iter()
        .filter(|p| Path::new(&p.cwd).starts_with(&record.worktree_path))
    {
        let busy = herdr
            .pane_process_info(&pane.pane_id)
            .map_err(|e| anyhow::anyhow!("pane {}: {e}", pane.pane_id))?
            .foreground_processes
            .iter()
            .any(|p| !matches!(p.name.as_str(), "zsh" | "-zsh" | "bash" | "sh" | "fish"));
        if busy {
            bail!(
                "worktree_in_use: a program runs in {} (pane {}); not removing",
                record.worktree_path,
                pane.pane_id
            );
        }
    }
    if panes
        .iter()
        .any(|p| p.tab_id == record.tab_id && p.pane_id != record.pane_id)
    {
        bail!(
            "wrong_tab: tab {} holds a pane that is not {}; not removing",
            record.tab_id,
            record.pane_id
        );
    }
    Ok(())
}

/// Lineage repair (SPEC-ADE D3); lineage is local.
pub fn tick(project: &Project, herdr: &Herdr, agents: &[Agent]) -> Result<()> {
    let coordinator = match project.coordinator() {
        Some(c) => c,
        None => return Ok(()),
    };
    for record in thread::list(project) {
        if record.is_remote() || record.status == Status::Resolved {
            continue;
        }
        let Some(agent) = agents.iter().find(|a| thread::agent_matches(&record, a)) else {
            continue;
        };
        // An unverified pane is never reparented (D3).
        let Some(_) = &record.identity.process else {
            continue;
        };
        let live = herdr
            .pane_process_info(&record.pane_id)
            .map(|info| info.identities())
            .unwrap_or_default();
        let mismatch = !thread::identity_verifies(&record, agent, &live);
        if mismatch != record.lineage_mismatch {
            thread::update(project, &record.id, |t| t.lineage_mismatch = mismatch)?;
        }
        if mismatch {
            continue;
        }
        if agent.parent() != Some(coordinator.pane_id.as_str()) {
            let _ = herdr.pane_set_parent(&record.pane_id, &coordinator.pane_id);
        }
    }
    Ok(())
}

/// The rounds that currently carry `thread`. The durable manifest is the
/// authority, never branch names or commits; an abandoned round releases all
/// of its members (SPEC-talk §6.5).
pub fn carrying_rounds(project: &Project, thread: &str) -> Vec<String> {
    crate::round::list(project)
        .into_iter()
        .filter(|round| round.carries(thread))
        .map(|round| round.round)
        .collect()
}

/// True when the round's merge reached the checkpointed phase: its required
/// work landed (SPEC-talk §6.5).
pub fn round_landed(project: &Project, round: &str) -> bool {
    crate::round::read_merge(project, round)
        .ok()
        .flatten()
        .is_some_and(|m| m.phase == crate::contracts::MergePhase::Checkpointed)
}

/// A thread with its live state and group, for `thread list`, `thread show`
/// and the overview.
pub struct Row {
    pub thread: Thread,
    pub group: Group,
    pub note: String,
}

pub fn rows(ctx: &Ctx, project: &Project) -> Vec<Row> {
    let _scope = crate::ledger::Scope::new(&[project]);
    let view = session_view(ctx, project);
    let now = jiff::Timestamp::now();
    thread::list(project)
        .into_iter()
        .map(|t| row(&t, view.as_ref(), now))
        .collect()
}

fn row(t: &Thread, view: Option<&SessionView>, now: jiff::Timestamp) -> Row {
    // Remote records use their last poll, except that durable lifecycle state
    // wins: in particular a failed start can never remain Working.
    let recorded = thread::recorded_group(t, now);
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
        // An identifier-shaped token is still refused; only the known-word
        // rule is relaxed for a thread sentence.
        assert!(check_birth_plain("It touches src/plain.rs there.").is_err());
        // A file name the record needs is allowed.
        check_birth_plain("It changes config.toml today.").unwrap();
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
        world.add_repo(&project, &repo_s);

        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        // Registered first: the first matching rule answers.
        world.runner.on(
            "agent start --help",
            ok("      --kind <KIND>\n          [possible values: pi, claude, cursor, agy]\n"),
        );
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
        std::fs::create_dir_all(world.home.path().join("cfg")).unwrap();
        std::fs::write(
            world.home.path().join("cfg/config.toml"),
            "[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n",
        )
        .unwrap();

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
        let started = start(
            &ctx,
            "demo",
            StartArgs {
                title: "Fix login".into(),
                repo: Some(repo_s.clone()),
                machine: None,
                base: None,
                task: "Do the thing.".into(),
                plain: "The lane does the work.".into(),
                workflow: None,
            },
        )
        .unwrap();
        assert_eq!(started.role, "lane");
        assert!(!started.launch.kind.is_empty());
        assert!(!started.launch.brief_hash.is_empty());
        assert_eq!(started.attempt, 1);
        let wt = Path::new(&repo_s).join(".worktrees").join(&started.id);
        assert!(wt.is_dir(), "git worktree should exist");
        // D9: the brief is a commit on the integration branch, the lane
        // branches from it, and its hash is on the record and in the env.
        let git_out = |args: &[&str]| {
            crate::runner::RealRunner
                .run(
                    &crate::runner::Cmd::new("git", Duration::from_secs(5))
                        .args(["-C", &repo_s])
                        .args(args.iter().copied()),
                )
                .unwrap()
                .stdout
                .trim()
                .to_string()
        };
        let rel = format!("tasks/{}.md", started.id);
        let committed = git_out(&["show", &format!("main:{rel}")]);
        assert!(
            committed.starts_with("plain: The lane does the work."),
            "{committed}"
        );
        assert_eq!(git_out(&["rev-parse", "main"]), started.base);
        assert!(wt.join(&rel).is_file(), "the brief is in the lane checkout");
        assert_eq!(
            started.launch.brief_hash,
            crate::thread::sha256_hex(format!("{committed}\n").as_bytes())
        );
        let exclude =
            std::fs::read_to_string(Path::new(&repo_s).join(".git/info/exclude")).unwrap();
        assert!(exclude.lines().any(|l| l == ".worktrees/"), "{exclude}");
        let calls = world.runner.calls.borrow();
        let env = format!(
            "HERDR_ADE_LAUNCH=demo/{}/1/{}",
            started.id, started.launch.brief_hash
        );
        assert!(
            calls.iter().any(|c| {
                let line = c.display();
                line.contains("tab create") && line.contains(&env)
            }),
            "expected tab create --env {env}"
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
            .find(|c| c.display().contains("agent start") && !c.display().contains("--help"))
            .map(|c| c.display())
            .expect("agent start");
        assert!(launch.contains("--parent w1:p1"), "{launch}");
        drop(calls);

        // A1 H2: the ready lane is primed once with its role skill and the
        // committed brief, never with the pre-ADE `brief.md` line.
        *world.agents.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::agent_json("w1", "w1:t2", "w1:p2", &wt_s, "hp-demo-t-0001", "idle")
        );
        crate::ticker::tick_project(&ctx, &project).unwrap();
        crate::ticker::tick_project(&ctx, &project).unwrap();
        let calls = world.runner.calls.borrow();
        let prompts: Vec<String> = calls
            .iter()
            .filter(|c| c.display().contains("agent prompt") && c.display().contains("w1:p2"))
            .map(|c| c.args.last().cloned().unwrap_or_default())
            .collect();
        assert_eq!(prompts.len(), 1, "{prompts:?}");
        assert!(
            prompts[0].ends_with(&format!(
                " skill lane, then read tasks/{}.md and do what it says.",
                started.id
            )),
            "{}",
            prompts[0]
        );
        drop(calls);
        *world.agents.borrow_mut() = "[]".into();

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
    fn a_harness_repo_starts_from_a_project_that_does_not_list_it() {
        use crate::runner::fake::ok;
        use crate::scenarios::World;

        let world = World::new();
        let project = world.project("demo", "a.sock");
        let harness = world.home.path().join("harness");
        std::fs::create_dir(&harness).unwrap();
        init_repo(&harness);
        let harness_s = std::fs::canonicalize(&harness)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        std::fs::create_dir_all(world.home.path().join("cfg")).unwrap();
        std::fs::write(
            world.home.path().join("cfg/config.toml"),
            format!(
                "[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n\n[harness]\nrepos = [{{ path = \"{harness_s}\" }}]\n"
            ),
        )
        .unwrap();

        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        world.runner.on(
            "agent start --help",
            ok("      --kind <KIND>\n          [possible values: pi, claude, cursor, agy]\n"),
        );
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
        let args = |repo: String| StartArgs {
            title: "Fix login".into(),
            repo: Some(repo),
            machine: None,
            base: None,
            task: "Do the thing.".into(),
            plain: "The lane does the work.".into(),
            workflow: None,
        };

        let other = world.home.path().join("other");
        std::fs::create_dir(&other).unwrap();
        init_repo(&other);
        let other_s = std::fs::canonicalize(&other)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let error = start(&ctx, "demo", args(other_s.clone()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("repo_not_listed"), "{error}");
        let mut remote_args = args(other_s);
        remote_args.machine = Some("box".into());
        let error = start(&ctx, "demo", remote_args).unwrap_err().to_string();
        assert!(error.contains("repo_not_listed"), "{error}");
        assert!(thread::list(&project).is_empty());

        let started = start(&ctx, "demo", args(harness_s.clone())).unwrap();
        assert_eq!(started.repo, harness_s);
        assert!(thread::list(&project).iter().any(|t| t.id == started.id));
    }

    #[test]
    fn ade_start_refuses_an_empty_plain() {
        let world = crate::scenarios::World::new();
        let _project = world.project("demo", "a.sock");
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&_project));
        let ctx = world.ctx();
        let missing = start(
            &ctx,
            "demo",
            StartArgs {
                title: "X".into(),
                repo: None,
                machine: None,
                base: None,
                task: "Do the thing.".into(),
                plain: String::new(),
                workflow: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(missing.contains("plain_missing"), "{missing}");
    }

    #[test]
    fn a_held_machine_refuses_a_new_box_start() {
        let root = tempfile::tempdir().unwrap();
        assert!(!project::machine_held(root.path(), "oci"));
        project::machine_hold(root.path(), "oci").unwrap();
        assert!(project::machine_held(root.path(), "oci"));
        assert!(project::machine_release(root.path(), "oci").unwrap());
        assert!(!project::machine_held(root.path(), "oci"));
        assert!(!project::machine_release(root.path(), "oci").unwrap());
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
        assert_eq!(inbox, 0);
        assert!(thread::load(&project, "t-0001").unwrap().lineage_mismatch);
        let digest = crate::coordinator::digest(&world.ctx(), &project, "ha")
            .unwrap()
            .0;
        assert!(digest.contains("lineage-mismatch"), "{digest}");
        let repaired = world.runner.calls.borrow().iter().any(|c| {
            let line = c.display();
            line.contains("report-metadata")
                && line.contains("parent=w1:p1")
                && line.contains("w2:p1")
        });
        assert!(!repaired, "mismatch must not repair parent");
    }

    // ---- default placement on the box (SPEC-remote §4.1, d-0005) ----

    /// A fixture whose project lists its repository with a box clone and a
    /// local bare remote the lane branch can publish to, so a box start needs
    /// no network.
    fn box_fixture() -> (crate::round::testkit::Fx, String) {
        let fx = crate::round::testkit::fixture();
        let remote = fx.world.home.path().join("remote.git");
        let status = std::process::Command::new("git")
            .args(["init", "--bare", "-q", &remote.to_string_lossy()])
            .status()
            .unwrap();
        assert!(status.success());
        let remote = remote.to_string_lossy().into_owned();
        crate::round::testkit::git(&fx.repo, &["remote", "add", "box", &remote]);
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos = vec![crate::project::Repo {
            path: fx.repo.to_string_lossy().into_owned(),
            box_path: Some("/home/ubuntu/projects/repo".into()),
            publish_url: Some(remote.clone()),
            ..Default::default()
        }];
        let text = format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap());
        std::fs::write(fx.project.project_md(), text).unwrap();
        (fx, remote)
    }

    fn write_config(fx: &crate::round::testkit::Fx, text: &str) {
        let cfg = fx.world.home.path().join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("config.toml"), text).unwrap();
    }

    /// The profile, provisioning and create fakes a box start needs.
    fn stub_box(fx: &crate::round::testkit::Fx) {
        use crate::runner::fake::ok;
        fx.world.runner.on(
            "machine list --json",
            ok(r#"[{"id":"oci-id","label":"oci","target":"oci-pi","session":"default","enabled":true}]"#),
        );
        fx.world.runner.on(
            "agent start --help",
            ok("      --kind <KIND>\n          [possible values: pi, claude, cursor, agy]\n"),
        );
        let workspace_exists = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = workspace_exists.clone();
        fx.world.runner.on_fn(
            |cmd| cmd.display().contains("workspace list"),
            move |_| {
                Ok(ok(if seen.get() {
                    r#"{"result":{"workspaces":[{"workspace_id":"w1","label":"Demo"}]}}"#
                } else {
                    r#"{"result":{"workspaces":[]}}"#
                }))
            },
        );
        fx.world.runner.on_fn(
            |cmd| cmd.display().contains("workspace create"),
            move |_| {
                workspace_exists.set(true);
                Ok(ok(r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","cwd":"/box/wt"}}}"#))
            },
        );
        fx.world.runner.on("tab rename", ok(r#"{"result":{}}"#));
        // Local fallbacks still create a tab in the coordinator workspace.
        fx.world.runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t3","pane_id":"w1:p3","cwd":"/box/wt"}}}"#),
        );
        // `provision` verifies the fetched commit is the one the Mac pushed:
        // answer with the base the script names. The card write needs nothing.
        fx.world.runner.on_fn(
            |cmd| cmd.program == "ssh",
            |cmd| {
                let script = cmd.args.last().cloned().unwrap_or_default();
                let base = script
                    .split("FETCH_HEAD)\" = ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or("");
                Ok(ok(&format!("{base}\n")))
            },
        );
    }

    const LANE_CONFIG: &str = "[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n[dispatch]\nmachine = \"oci\"\n";

    fn start_args(repo: Option<String>, machine: Option<String>) -> StartArgs {
        StartArgs {
            title: "Fix login".into(),
            repo,
            machine,
            base: None,
            task: "Do the thing.".into(),
            plain: "The lane does the work.".into(),
            workflow: None,
        }
    }

    fn say_lines(project: &Project) -> Vec<String> {
        crate::talk::read(project)
            .lines
            .iter()
            .filter_map(|line| match &line.entry {
                crate::talk::Entry::Say { what, .. } => Some(what.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn only_lane_and_reviewer_default_to_the_box() {
        let row = crate::project::Repo {
            path: "/r".into(),
            box_path: Some("/box/r".into()),
            ..Default::default()
        };
        assert_eq!(
            default_machine("lane", "oci", Some("/r"), Some(&row)),
            Some("oci".into())
        );
        assert_eq!(
            default_machine("reviewer", "oci", Some("/r"), Some(&row)),
            Some("oci".into())
        );
        assert_eq!(
            default_machine("research", "oci", Some("/r"), Some(&row)),
            None
        );
        assert_eq!(default_machine("lane", "", Some("/r"), Some(&row)), None);
        let plain = crate::project::Repo {
            path: "/r".into(),
            ..Default::default()
        };
        assert_eq!(
            default_machine("lane", "oci", Some("/r"), Some(&plain)),
            None
        );
        assert_eq!(default_machine("lane", "oci", None, Some(&row)), None);
    }

    #[test]
    fn box_lanes_share_one_project_workspace_and_take_separate_tabs() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, LANE_CONFIG);
        stub_box(&fx);
        let started = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        assert_eq!(started.machine, "oci");
        assert_eq!(started.machine_id, "oci-id");
        assert!(
            started
                .worktree_path
                .starts_with("/home/ubuntu/projects/repo/.worktrees/")
        );
        assert!(
            say_lines(&fx.project).is_empty(),
            "a box start says nothing"
        );
        let second = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        assert_eq!(second.workspace_id, started.workspace_id);
        let calls = fx.world.runner.calls.borrow();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.display().contains("workspace create"))
                .count(),
            1
        );
        assert!(calls.iter().any(|call| {
            let line = call.display();
            line.contains("workspace create")
                && line.contains("--label Demo")
                && line.contains(&format!("HERDR_ADE_LAUNCH=demo/{}/1/", started.id))
                && line.contains(&format!("--cwd {}", started.worktree_path))
        }));
        assert!(calls.iter().any(|call| {
            call.display()
                .contains(&format!("tab rename w1:t2 {}", started.id))
        }));
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.display().contains("tab create"))
                .count(),
            1,
            "only the later lane adds a tab to the shared workspace"
        );
    }

    #[test]
    fn a_research_pick_uses_the_mac_when_the_box_has_no_agy_and_records_why() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, LANE_CONFIG);
        fx.world.runner.on_fn(
            |cmd| {
                cmd.program == "ssh"
                    && cmd
                        .args
                        .last()
                        .is_some_and(|script| script.contains("command -v agy"))
            },
            |_| {
                Ok(crate::runner::fake::fail(
                    127,
                    "agy is missing from the lane PATH",
                ))
            },
        );
        stub_box(&fx);
        let mut args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
        args.task = "+++\nproduct = \"web-research\"\n+++\nCompare the published results.".into();
        let started = start(&fx.world.ctx(), "demo", args).unwrap();
        assert_eq!(started.launch.recipe_id, "agy_gemini_flash");
        assert!(
            started.machine.is_empty(),
            "research must stay off this box"
        );
        let ledger =
            std::fs::read_to_string(fx.project.state_dir().join("dispatch.jsonl")).unwrap();
        let placement = ledger
            .lines()
            .rfind(|line| line.contains("\"kind\":\"placement\""))
            .unwrap();
        assert!(placement.contains("\"machine\":\"local\""), "{placement}");
        assert!(
            placement.contains("agy is missing from the lane PATH"),
            "{placement}"
        );
        assert!(placement.contains("agy_gemini_flash"), "{placement}");
    }

    #[test]
    fn a_pi_pick_still_uses_a_ready_box() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, LANE_CONFIG);
        let task = "Run the bounded coding task.";
        let policy_path = fx.world.home.path().join("cfg/routing.json");
        let mut policy: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&policy_path).unwrap()).unwrap();
        policy["pins"][crate::thread::sha256_hex(task.as_bytes())] =
            serde_json::json!("pi_opencode_deepseek");
        std::fs::write(&policy_path, policy.to_string()).unwrap();
        stub_box(&fx);
        let mut args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
        args.task = task.into();
        let started = start(&fx.world.ctx(), "demo", args).unwrap();
        assert_eq!(started.launch.recipe_id, "pi_opencode_deepseek");
        assert_eq!(started.machine, "oci");
        assert_eq!(started.machine_id, "oci-id");
    }

    #[test]
    fn an_explicit_machine_that_cannot_run_the_pick_is_refused() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, LANE_CONFIG);
        fx.world.runner.on_fn(
            |cmd| {
                cmd.program == "ssh"
                    && cmd
                        .args
                        .last()
                        .is_some_and(|script| script.contains("command -v agy"))
            },
            |_| {
                Ok(crate::runner::fake::fail(
                    127,
                    "agy is missing from the lane PATH",
                ))
            },
        );
        stub_box(&fx);
        let mut args = start_args(
            Some(fx.repo.to_string_lossy().into_owned()),
            Some("oci".into()),
        );
        args.task = "+++\nproduct = \"web-research\"\n+++\nCompare the published results.".into();
        let error = start(&fx.world.ctx(), "demo", args)
            .unwrap_err()
            .to_string();
        assert!(error.contains("recipe_unavailable"), "{error}");
        assert!(error.contains("agy_gemini_flash"), "{error}");
        assert!(error.contains("oci"), "{error}");
        assert!(
            error.contains("agy is missing from the lane PATH"),
            "{error}"
        );
        assert!(thread::list(&fx.project).is_empty());
        let ledger =
            std::fs::read_to_string(fx.project.state_dir().join("dispatch.jsonl")).unwrap();
        assert!(ledger.contains("placement-refused"), "{ledger}");
    }

    #[test]
    fn no_ready_machine_names_the_recipe_and_every_machine_tried() {
        let home = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(home.path(), &[]);
        let runner = crate::runner::fake::FakeRunner::new();
        runner.on(
            "machine list --json",
            crate::runner::fake::ok(
                r#"[{"id":"oci-id","label":"oci","target":"oci-pi","session":"default","enabled":true}]"#,
            ),
        );
        runner.on_fn(
            |cmd| cmd.program == "ssh",
            |_| {
                Ok(crate::runner::fake::fail(
                    127,
                    "agy is missing from the lane PATH",
                ))
            },
        );
        runner.on_fn(
            |cmd| cmd.program == "agy",
            |_| Ok(crate::runner::fake::fail(1, "agy is not signed in here")),
        );
        let ctx = Ctx {
            env: &env,
            root: home.path().join("root"),
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let launch = crate::contracts::Launch {
            kind: "agy".into(),
            recipe_id: "agy_gemini_flash".into(),
            machine: "oci".into(),
            ready_timeout_ms: 30_000,
            ..Default::default()
        };
        let row = crate::project::Repo {
            path: "/repo".into(),
            box_path: Some("/box/repo".into()),
            ..Default::default()
        };
        let error = resolve_placement(&ctx, None, "lane", &launch, Some("/repo"), Some(&row))
            .unwrap_err()
            .to_string();
        assert!(error.contains("agy_gemini_flash"), "{error}");
        assert!(error.contains("oci") && error.contains("local"), "{error}");
        assert!(
            error.contains("lane PATH") && error.contains("not signed in"),
            "{error}"
        );
    }

    #[test]
    fn box_repo_row_names_each_missing_piece() {
        let mut settings = crate::project::Settings {
            repos: vec![crate::project::Repo {
                path: "/r".into(),
                box_path: Some("/box/r".into()),
                publish_url: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        let e = box_repo_row(&settings, "/r").unwrap_err().to_string();
        assert!(
            e.contains("box_publish_url_missing") && e.contains("publish_url"),
            "{e}"
        );

        settings.repos[0].box_path = None;
        settings.repos[0].publish_url = Some("https://example/r.git".into());
        let e = box_repo_row(&settings, "/r").unwrap_err().to_string();
        assert!(
            e.contains("box_path_missing") && e.contains("box_path"),
            "{e}"
        );

        // A repo with neither, and not in the built-in list, keeps the
        // mapping message; a built-in harness repo resolves from the map.
        settings.repos[0].publish_url = None;
        let e = box_repo_row(&settings, "/other").unwrap_err().to_string();
        assert!(e.contains("box_repo_unmapped"), "{e}");
        let (box_path, url) = box_repo_row(&settings, "/Users/rolfie/projects/herdr").unwrap();
        assert_eq!(box_path, "/home/ubuntu/projects/herdr");
        assert!(url.ends_with("herdr.git"), "{url}");
    }

    #[test]
    fn a_box_repo_without_a_publish_url_names_it_and_leaves_no_thread() {
        let (fx, _remote) = box_fixture();
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos[0].publish_url = None;
        let text = format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap());
        std::fs::write(fx.project.project_md(), text).unwrap();
        write_config(&fx, LANE_CONFIG);
        stub_box(&fx);
        let error = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("box_publish_url_missing"), "{error}");
        assert!(
            error.contains("publish_url") && error.contains("PROJECT.md"),
            "{error}"
        );
        assert!(
            thread::list(&fx.project).is_empty(),
            "a refused box start leaves no thread record"
        );
    }

    #[test]
    fn machine_local_keeps_a_box_repo_lane_on_this_mac() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, LANE_CONFIG);
        stub_box(&fx);
        let started = start(
            &fx.world.ctx(),
            "demo",
            start_args(
                Some(fx.repo.to_string_lossy().into_owned()),
                Some("local".into()),
            ),
        )
        .unwrap();
        assert!(started.machine.is_empty());
        assert!(started.machine_id.is_empty());
        assert!(
            started
                .worktree_path
                .starts_with(&fx.repo.to_string_lossy().to_string())
        );
    }

    #[test]
    fn a_task_with_no_repo_stays_a_local_tab() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, LANE_CONFIG);
        stub_box(&fx);
        *fx.world.panes.borrow_mut() = format!("[{}]", fx.world.coordinator_pane(&fx.project));
        let started = start(&fx.world.ctx(), "demo", start_args(None, None)).unwrap();
        assert_eq!(started.kind, Kind::Tab);
        assert!(started.machine.is_empty());
    }

    #[test]
    fn a_held_box_falls_back_to_this_mac_with_one_line() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, LANE_CONFIG);
        stub_box(&fx);
        let ctx = fx.world.ctx();
        project::machine_hold(&ctx.root, "oci-id").unwrap();
        let started = start(
            &ctx,
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        assert!(started.machine.is_empty());
        assert_eq!(
            say_lines(&fx.project),
            vec!["the box was not ready, so this lane runs here".to_string()]
        );
    }

    #[test]
    fn an_unreachable_box_falls_back_to_this_mac() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, LANE_CONFIG);
        fx.world.runner.on_fn(
            |cmd| {
                cmd.program == "ssh"
                    && cmd
                        .args
                        .last()
                        .is_some_and(|script| script.contains("command -v claude"))
            },
            |_| Ok(crate::runner::fake::fail(255, "connection refused")),
        );
        stub_box(&fx);
        let started = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        assert!(started.machine.is_empty());
        assert_eq!(
            say_lines(&fx.project),
            vec!["the box was not ready, so this lane runs here".to_string()]
        );
    }

    #[test]
    fn an_explicit_held_box_refuses() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, LANE_CONFIG);
        stub_box(&fx);
        let ctx = fx.world.ctx();
        project::machine_hold(&ctx.root, "oci-id").unwrap();
        let error = start(
            &ctx,
            "demo",
            start_args(
                Some(fx.repo.to_string_lossy().into_owned()),
                Some("oci".into()),
            ),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("machine_held"), "{error}");
    }

    #[test]
    fn a_box_reviewer_can_publish_its_verdict_and_seal_without_a_manual_coordinator_push() {
        use crate::contracts::{OpKind, Recipient, Requested};
        use crate::ops;
        use crate::round::testkit::{commit_file, git};

        let (fx, remote) = box_fixture();
        write_config(
            &fx,
            "[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the careful checker\"\n[dispatch]\nmachine = \"oci\"\n",
        );
        stub_box(&fx);
        let ctx = fx.world.ctx();
        crate::round::open(
            &ctx,
            "demo",
            crate::round::OpenArgs {
                round: "r1".into(),
                branch: "main".into(),
                plain: Some("The first round lands the shared types.".into()),
                repo: Some(fx.repo.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        for (id, sha) in [fx.lane(1), fx.lane(2)] {
            crate::round::admit(&ctx, "demo", "r1", &id).unwrap();
            fx.seal_done(&id, 1, 1, &sha, &format!("# report {id}\n"));
        }
        crate::round::advance(&ctx, "demo").unwrap();
        let record = crate::round::load(&fx.project, "r1").unwrap();
        let reviewer = record.reviewer.clone().expect("reviewer bound");
        let started = thread::load(&fx.project, &reviewer).unwrap();
        assert_eq!(started.role, "reviewer");
        assert_eq!(started.machine, "oci");
        assert_eq!(started.machine_id, "oci-id");

        // advance already publishes the starting commit, before V exists.
        ops::check_published_ref(
            ctx.runner,
            &fx.repo,
            &started.branch,
            &remote,
            &started.base,
        )
        .unwrap();
        let skill = crate::lane::skill_text(&started.role);
        assert!(skill.contains("publish V on **your own lane branch**"));
        assert!(
            skill.contains("Never push `main`, the integration branch, or another lane's branch")
        );
        let brief = git(
            &fx.repo,
            &["show", &format!("{}:tasks/review-r1.md", started.base)],
        );
        assert!(brief.contains("Follow the reviewer skill's Done instructions"));

        // A separate clone stands in for the box; only herdr/ssh are faked.
        let box_repo = fx.world.home.path().join("box reviewer's clone");
        git(
            &fx.repo,
            &[
                "clone",
                "-q",
                "-b",
                &started.branch,
                &remote,
                box_repo.to_str().unwrap(),
            ],
        );
        git(&box_repo, &["config", "user.name", "Reviewer"]);
        git(&box_repo, &["config", "user.email", "reviewer@example.com"]);
        git(&box_repo, &["config", "commit.gpgsign", "false"]);
        for member in &record.manifest.members {
            let sha = &member.pin.as_ref().unwrap().sha;
            git(&box_repo, &["fetch", "-q", fx.repo.to_str().unwrap(), sha]);
            git(&box_repo, &["merge", "--no-edit", sha]);
        }
        let candidate = git(&box_repo, &["rev-parse", "HEAD"]);
        let verdict = commit_file(
            &box_repo,
            "tasks/reviews/code-r1.md",
            &format!(
                "+++\nverdict = \"MERGE\"\nround = \"r1\"\ncandidate = \"{candidate}\"\nmanifest_hash = \"{}\"\npolicy_hash = \"{}\"\ngates = []\n+++\n\nAll lanes checked.\n",
                record.manifest_hash.as_deref().unwrap(),
                record.policy_hash,
            ),
            "review(r1): verdict",
        );
        assert_ne!(started.base, verdict);
        let error =
            ops::check_published_ref(ctx.runner, &box_repo, &started.branch, &remote, &verdict)
                .unwrap_err()
                .to_string();
        assert!(error.starts_with("published_ref_mismatch:"), "{error}");
        // The reviewer follows the refusal's command. No coordinator push,
        // force, integration branch update or direct event fixture is needed.
        let repair = error
            .split("; run `")
            .nth(1)
            .unwrap()
            .split('`')
            .next()
            .unwrap();
        let repaired = std::process::Command::new("sh")
            .args(["-c", repair])
            .output()
            .unwrap();
        assert!(
            repaired.status.success(),
            "{}",
            String::from_utf8_lossy(&repaired.stderr)
        );
        ops::check_published_ref(ctx.runner, &box_repo, &started.branch, &remote, &verdict)
            .unwrap();
        assert_eq!(git(&fx.repo, &["rev-parse", &started.branch]), started.base);
        assert_eq!(
            git(&fx.repo, &["ls-remote", &remote, "refs/heads/main"]),
            ""
        );

        let box_root = fx.world.home.path().join("box-root");
        let box_project = project::create(&box_root, "demo", "", vec![]).unwrap();
        let op = ops::reserve(
            &box_project,
            ops::Reservation {
                thread: &started.id,
                attempt: started.attempt,
                kind: OpKind::Done,
                recipient: Recipient {
                    pane: "w1:p1".into(),
                    coordinator_attempt: 1,
                },
                round: None,
                requested: Requested::Done {
                    sha: verdict.clone(),
                    report_path: "tasks/reviews/code-r1.md".into(),
                },
                helper_pid: std::process::id(),
            },
        )
        .unwrap();
        ops::stage_done(&box_project, &op.op, &box_repo, ctx.runner).unwrap();
        let event = ops::seal(&box_project, &op.op, |_| Ok(())).unwrap();
        assert_eq!(event.payload.done.unwrap().sha, verdict);
        assert_eq!(crate::events::list(&box_project).len(), 1);
    }
}
