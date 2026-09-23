//! The `thread` subcommands. Each is one deterministic mechanic; the
//! coordinator decides whether, what and where.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::herdr::{Agent, Herdr, Pane};
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::runner::{Cmd, Runner};
use crate::thread::{self, CopyOutcome, FollowUp, FollowUpState, Group, Kind, Status, Thread};
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
    /// An exact recipe Rolf named for this one lane.
    pub recipe: Option<String>,
    /// Verbatim words from a Rolf request attached to `task_id`.
    pub recipe_basis: Option<String>,
    pub task_id: String,
    /// Internal reviewer identity; empty for every non-reviewer start.
    pub review_round: String,
}

/// Internal birth sentence: required and structurally one sentence. Exact and
/// long technical details are retained; the screen wraps or collapses them.
pub fn check_birth_plain(text: &str) -> Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("plain_missing");
    }
    if crate::plain::sentence_count(trimmed) != 1 {
        bail!("write one sentence");
    }
    Ok(())
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

    check_birth_plain(&args.plain)?;
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
    let requested_repo = match args.repo.as_deref() {
        Some(repo) => repo,
        None => match settings.repos.as_slice() {
            [only] => &only.path,
            [] => bail!(
                "repo_required: this project has no listed repository; pass --repo after listing one"
            ),
            _ => bail!("repo_ambiguous: this project lists several repositories; pass --repo"),
        },
    };
    let repo = std::fs::canonicalize(requested_repo)
        .with_context(|| format!("repository {requested_repo} does not exist"))?
        .to_string_lossy()
        .into_owned();
    if !crate::harness::allowed_repo(&settings, &ctx.config_dir, &repo)? {
        bail!(
            "repo_not_listed: {repo} is not listed in `repos` in PROJECT.md and is not a harness repository"
        );
    }
    let listed = settings.repos.iter().find(|row| {
        std::fs::canonicalize(&row.path).is_ok_and(|path| path.to_string_lossy() == repo)
    });
    let recipe_request = match &args.recipe {
        Some(recipe) => Some(crate::launch::authorize_explicit_recipe(
            ctx,
            &project,
            &args.task_id,
            &args.task,
            role,
            recipe,
            args.recipe_basis.as_deref().unwrap_or_default(),
        )?),
        None if args.recipe_basis.is_some() => {
            bail!("recipe_basis_without_recipe: --basis is only valid with --recipe")
        }
        None => None,
    };
    let mut launch = crate::launch::resolve_launch(
        ctx,
        &project,
        &crate::launch::ResolveInput {
            task: &args.task,
            workflow: role,
            recipe: args.recipe.as_deref(),
            recipe_basis: args.recipe_basis.as_deref().map(str::trim),
            recipe_request: recipe_request.as_deref(),
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
        Some(repo.as_str()),
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
        fallback_say(ctx, slug, &placement)?;
    }
    let machine = placement.machine.clone();
    // Selection initially carries the configured dispatch candidate because
    // placement needs it. The durable lane launch names where this attempt was
    // actually placed, including an explicit or fallback local placement.
    launch.machine = placement.ledger_machine().to_string();

    // Recipe and repository readiness both ran on the selected machine
    // during placement, before a thread record exists.
    let machine_id = placement.machine_id.clone();
    let record = thread::allocate(&project, |t| {
        t.title = args.title.trim().to_string();
        t.kind = Kind::Worktree;
        t.repo = repo.clone();
        t.machine = machine.clone();
        t.placement_reason = placement.reason.clone();
        t.machine_id = machine_id.clone();
        t.agent = launch.kind.clone();
        t.base = args.base.clone().unwrap_or_default();
        t.role = role.to_string();
        t.review_round = args.review_round.clone();
        t.plain = args.plain.trim().to_string();
        t.attempt = 1;
        t.launch = launch.clone();
    })?;
    let id = record.id.clone();
    {
        let _lock = project.lock()?;
        project::write_atomic(
            &thread::task_path_for_write(&project, &id)?,
            args.task.as_bytes(),
        )?;
    }
    // Link before composing the brief so task-scoped notes are available to
    // this first attempt, not only to retries.
    if !args.task_id.is_empty() {
        crate::task::link_attempt(&project, &args.task_id, &id)?;
    }

    match place_and_brief(ctx, &project, &view, &id, false) {
        Ok(thread) => {
            refresh_plan(ctx, &project);
            if thread.is_remote()
                && let Err(error) =
                    ticker::request_remote_poll(&ctx.root, &project, thread.machine_route())
            {
                eprintln!(
                    "note: thread {id} is placed, but the ticker could not be woken for its first box poll: {error:#}"
                );
            }
            Ok(thread::load(&project, &id).unwrap_or(thread))
        }
        Err(error) => {
            let message = format!("{error:#}");
            let cleanup = fail_start(
                ctx,
                &project,
                &id,
                &message,
                crate::contracts::FailureClass::Unknown,
                false,
            );
            let error = match cleanup {
                Ok(_) => error,
                Err(cleanup) => error.context(format!("failed-start cleanup: {cleanup:#}")),
            };
            Err(error.context(format!(
                "thread {id} failed to start; `thread retry {slug} {id} --reason <why>` retries"
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
/// d-0005): lane/review work with a dispatch machine and a repository tries
/// that box. Placement validates the repository mapping before selecting it;
/// every other start stays on this Mac.
fn default_machine(role: &str, role_machine: &str, repo: Option<&str>) -> Option<String> {
    if !matches!(role, "lane" | "reviewer") || role_machine.is_empty() || repo.is_none() {
        return None;
    }
    Some(role_machine.to_string())
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
        None => match default_machine(role, &launch.machine, repo) {
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
            match remote::machine_declaration(&ctx.config_dir, &candidate) {
                Err(error) => Err(format!("{error:#}")),
                Ok(machine) if !machine.runs_kind(&launch.kind) => Err(format!(
                    "machine_kind_unavailable: `{}` does not run adapter kind `{}`",
                    machine.label, launch.kind
                )),
                Ok(_) => match remote::machine_profile(
                    ctx.runner,
                    &ctx.env.herdr_bin(),
                    &ctx.config_dir,
                    &candidate,
                ) {
                    Err(error) => Err(format!("{error:#}")),
                    Ok(profile) if profile.is_local() => {
                        crate::doctor::recipe_ready_local(ctx, launch)
                            .map(|_| None)
                            .map_err(|error| format!("{error:#}"))
                    }
                    Ok(profile) => box_repo_candidate(&ctx.config_dir, &profile.label, repo, listed)
                        .map_err(|error| format!("{error:#}"))
                        .and_then(|_| {
                            if project::machine_held(&ctx.root, &profile.id) {
                                Err(format!(
                                    "machine_held: `{}` is held; run `ha machine release {}` when the fork refresh or resize is done",
                                    profile.label, profile.label
                                ))
                            } else {
                                crate::doctor::recipe_ready_on_box(ctx, &profile, launch)
                                    .map(|_| Some(profile))
                                    .map_err(|error| format!("{error:#}"))
                            }
                        }),
                },
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
fn fallback_say(ctx: &Ctx, slug: &str, placement: &Placement) -> Result<()> {
    let missing = placement
        .tried
        .iter()
        .find_map(|row| row["missing"].as_str())
        .unwrap_or_default();
    let what = if missing.contains("box_publish_url_missing") {
        "the box has no publishing address for this repository, so this lane runs here"
    } else if missing.contains("box_path_missing") {
        "the box has no folder for this repository, so this lane runs here"
    } else if missing.contains("box_repo_unmapped") {
        "this repository has no box location, so this lane runs here"
    } else if missing.contains("machine_kind_unavailable") {
        "the box does not run this kind of helper, so this lane runs here"
    } else {
        "the box was not ready, so this lane runs here"
    };
    let project = Project::load(&ctx.root, slug)?;
    if crate::talk::read(&project)
        .lines
        .iter()
        .rev()
        .find_map(|line| {
            if let crate::talk::Entry::Say { what, means, .. } = &line.entry {
                Some((what.as_str(), means.as_deref()))
            } else {
                None
            }
        })
        == Some((what, None))
    {
        return Ok(());
    }
    crate::ask::say(ctx, slug, what, None).map(|_| ())
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

/// Recipe readiness on a remote machine, measured there rather than against
/// this machine's executable or login state.
pub(crate) fn box_launch_ready_for(
    ctx: &Ctx,
    machine: &str,
    launch: &crate::contracts::Launch,
) -> Result<()> {
    let profile =
        remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, machine)?;
    box_launch_ready(ctx, &profile, launch)
}

/// Steps 2 to 5 of starting a thread, also used when recovery must place it.
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

/// Materializes the frozen, content-addressed brief in the lane's ignored
/// runtime folder. The product repository never tracks it.
fn write_brief(ctx: &Ctx, project: &Project, placed: &Thread) -> Result<()> {
    if placed.is_remote() {
        return Ok(());
    }
    prepare_local_dir(ctx, project, placed)?;
    if placed.kind == Kind::Worktree {
        let path = Path::new(&placed.thread_dir).join("brief.md");
        if !path.is_file() {
            let bytes = thread::artifact(project, &placed.launch.brief_hash)?;
            project::write_atomic(&path, &bytes)?;
        }
    }
    Ok(())
}

/// The box clone path and publish URL a box start needs for `repo`, each
/// reported by name when it is missing. A row in `PROJECT.md` whose
/// `box_path` is set but whose `publish_url` is not names the missing URL; a
/// repo with neither falls back to the built-in harness map, or keeps the
/// mapping message (t-0070).
fn box_repo_candidate(
    config_dir: &Path,
    machine: &str,
    repo: Option<&str>,
    row: Option<&crate::project::Repo>,
) -> Result<(String, String)> {
    let repo = repo.context("box_repo_unmapped: a box lane needs a repository")?;
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
            let map = crate::remote::box_repo_for(config_dir, machine, repo)?.with_context(|| {
                format!(
                    "box_repo_unmapped: {repo} has no Mac-to-box row; add one before the first box start"
                )
            })?;
            Ok((map.box_path.unwrap(), map.publish_url.unwrap()))
        }
    }
}

pub(crate) fn box_repo_row(
    config_dir: &Path,
    settings: &crate::project::Settings,
    machine: &str,
    repo: &str,
) -> Result<(String, String)> {
    box_repo_candidate(
        config_dir,
        machine,
        repo.into(),
        settings.repos.iter().find(|r| r.path == repo),
    )
}

/// The box start side: freeze the brief as a project artifact, branch from an
/// exact integration commit, provision the checkout, and materialize the brief
/// only in the checkout's ignored runtime folder.
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
    let profile = remote::machine_profile(
        runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?;
    let (box_repo, publish_url) =
        box_repo_row(&ctx.config_dir, &settings, &profile.label, &record.repo)?;
    // Both clones must name the configured publish URL. The push still uses
    // the URL itself; finding the matching remote only validates this clone.
    let _ = remote::remote_for_url(runner, &record.repo, &publish_url)?;
    let target = profile.target.clone();
    let box_worktree = format!("{box_repo}/.worktrees/{}", record.id);
    let branch = if record.branch.is_empty() {
        thread::branch_name(&project.slug, &record.id, &record.title)
    } else {
        record.branch.clone()
    };
    let dir = thread::thread_dir(&box_worktree, &project.slug, &record.id);

    // A restart reuses the exact frozen artifact and code base. A first start
    // records both before any remote process exists.
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
        let frozen = format!("plain: {}\n\n{brief}", record.plain);
        let brief_hash = thread::store_artifact(project, frozen.as_bytes())?;
        let integration = integration_branch(runner, record)?;
        let repo_lock = crate::git::lock(runner, &record.repo)?;
        if let Err(error) = crate::git::exclude_plugin_paths_locked(runner, &record.repo) {
            eprintln!("warning: {error:#}");
        }
        let head =
            crate::git::rev_parse(runner, &record.repo, &format!("refs/heads/{integration}"))?;
        ensure_branch(runner, &record.repo, &branch, &head)?;
        drop(repo_lock);
        push_branch(runner, &record.repo, &publish_url, &branch, &head)?;
        (head, brief_hash)
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

    let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    if record.failure_event.is_empty() {
        remote::provision(
            runner,
            &target,
            &remote::Provision {
                path: &machine.path,
                box_repo: &box_repo,
                worktree: &box_worktree,
                branch: &branch,
                base: &base,
                publish_url: &publish_url,
            },
        )?;
    }
    let frozen = thread::artifact(project, &brief_hash)?;
    remote::write_runtime_file(
        runner,
        &target,
        &format!("{dir}/brief.md"),
        &String::from_utf8(frozen).context("brief artifact is not UTF-8")?,
        &brief_hash,
    )?;

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
        Some(&machine),
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
    let remote_prefix = format!("{} --root {}", machine.ade_bin, machine.root);
    let start_line = thread::launch_prompt(
        &remote_prefix,
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
    let card_path = format!(
        "{}/{}/.state/lanes/{}.toml",
        machine.root, project.slug, record.id
    );
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

/// The integration branch whose exact head becomes the lane's code base:
/// `--base`, else the branch the repository has checked out.
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

/// Places a code lane from an exact integration commit. Its frozen brief is a
/// project artifact and is materialized later under `.herdr-project`, never
/// committed to the code repository.
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
    // the repository has checked out. A remote-tracking ref or bare sha is
    // not a mutable integration branch.
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
    let frozen = format!("plain: {}\n\n{brief}", record.plain);
    let brief_hash = thread::store_artifact(project, frozen.as_bytes())?;

    let placed = {
        let _repo_lock = crate::git::lock(runner, &record.repo)?;
        if let Err(error) = crate::git::exclude_plugin_paths_locked(runner, &record.repo) {
            eprintln!("warning: {error:#}");
        }
        let head =
            crate::git::rev_parse(runner, &record.repo, &format!("refs/heads/{integration}"))?;
        // The artifact hash and exact code base are durable before the lane
        // branch or worktree exists.
        thread::update(project, &record.id, |t| {
            t.launch.brief_hash = brief_hash.clone();
            t.thread_dir = stub.thread_dir.clone();
            t.base = head.clone();
            t.branch = branch.clone();
            t.partial = Some("worktree_add".into());
        })?;
        let path = crate::git::worktree_add(runner, &record.repo, &record.id, &branch, &head)?;
        (head, path)
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
        None,
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
    ctx: &Ctx,
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
        thread::threads_dir_for_write(project)?.join(&record.id)
    } else {
        Path::new(&record.worktree_path).to_path_buf()
    };
    let managed = record.kind == Kind::Tab;
    let (folder, brief_hash, base) = if managed {
        let task =
            std::fs::read_to_string(thread::task_path(project, &record.id)).unwrap_or_default();
        let stub = Thread {
            worktree_path: folder.to_string_lossy().into_owned(),
            thread_dir: folder.to_string_lossy().into_owned(),
            ..record.clone()
        };
        let brief = format!(
            "plain: {}\n\n{}",
            record.plain,
            thread::brief_for(project, &stub, &task, false)?
        );
        prepare_managed_git_folder(ctx.runner, &folder, &brief)?
    } else {
        (
            folder,
            record.launch.brief_hash.clone(),
            record.base.clone(),
        )
    };
    let folder_text = folder.to_string_lossy().into_owned();
    if managed {
        thread::update(project, &record.id, |t| {
            t.worktree_path = folder_text.clone();
            t.thread_dir = folder_text.clone();
            t.branch = "main".into();
            t.base = base.clone();
            t.launch.brief_hash = brief_hash.clone();
        })?;
    }
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
        None,
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
        if managed {
            t.worktree_path = folder_text.clone();
            t.thread_dir = folder_text;
            t.branch = "main".into();
            t.base = base;
            t.launch.brief_hash = brief_hash;
        }
    })
}

/// Makes the project-owned git folder used by a thread with no code
/// repository. Its first commit contains only `brief.md`; the lane creates and
/// commits report or deliverable files there before calling `done`.
pub(crate) fn prepare_managed_git_folder(
    runner: &dyn Runner,
    folder: &Path,
    brief: &str,
) -> Result<(std::path::PathBuf, String, String)> {
    std::fs::create_dir_all(folder)
        .with_context(|| format!("could not create {}", folder.display()))?;
    let folder = std::fs::canonicalize(folder)?;
    let folder_text = folder.to_string_lossy().into_owned();
    let brief_path = folder.join("brief.md");

    let head = if folder.join(".git").is_dir() {
        git(
            runner,
            &folder_text,
            &["for-each-ref", "--format=%(objectname)", "refs/heads/main"],
            GIT_TIMEOUT,
        )?
        .lines()
        .next()
        .filter(|head| !head.is_empty())
        .map(str::to_string)
    } else {
        None
    };
    if let Some(head) = head {
        let tracked = git(
            runner,
            &folder_text,
            &["ls-tree", "--name-only", "HEAD", "--", "brief.md"],
            GIT_TIMEOUT,
        )?;
        if tracked != "brief.md" || !brief_path.is_file() {
            bail!(
                "managed_folder_invalid: {} has a first commit without brief.md",
                folder.display()
            );
        }
        let bytes = std::fs::read(&brief_path)?;
        return Ok((folder, thread::sha256_hex(&bytes), head));
    }

    project::write_atomic(&brief_path, brief.as_bytes())?;
    if !folder.join(".git").is_dir() {
        git(
            runner,
            &folder_text,
            &["init", "-q", "-b", "main"],
            GIT_TIMEOUT,
        )?;
    }
    git(
        runner,
        &folder_text,
        &["add", "--", "brief.md"],
        GIT_TIMEOUT,
    )?;
    git(
        runner,
        &folder_text,
        &[
            "-c",
            "user.name=herdr-ade",
            "-c",
            "user.email=herdr-ade@localhost",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "docs: thread brief",
        ],
        GIT_TIMEOUT,
    )?;
    let head = git(runner, &folder_text, &["rev-parse", "HEAD"], GIT_TIMEOUT)?;
    Ok((folder, thread::sha256_hex(brief.as_bytes()), head))
}

/// Creates the recorded thread directory and keeps it out of git. Its report
/// and library folders appear only when their writers use them.
fn prepare_local_dir(ctx: &Ctx, project: &Project, placed: &Thread) -> Result<()> {
    let dir = if placed.thread_dir.is_empty() {
        thread::thread_dir(&placed.cwd, &project.slug, &placed.id)
    } else {
        placed.thread_dir.clone()
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {dir}"))?;
    if placed.kind != Kind::Tab && !managed_git_folder(project, placed) {
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
        t.startup_wait_started.clear();
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
        crate::doctor::recipe_ready_local(ctx, &record.launch)?;
        if record.kind == Kind::Worktree && record.worktree_path.is_empty()
            || record.kind == Kind::Tab && record.pane_id.is_empty()
        {
            place_and_brief(ctx, project, &view, &record.id, true)?;
        } else {
            place_ade_tab(ctx, project, &view, record)?;
            let placed = thread::load(project, &record.id)?;
            write_brief(ctx, project, &placed)?;
            finish_placement(project, &view, &record.id)?;
        }
    }
    thread::update(project, &record.id, |t| t.escalation_pending = false)?;
    Ok(())
}

/// Typed recovery result shared by human and JSON rendering.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RetryOutcome {
    pub thread: String,
    pub attempt: u32,
    pub pane_id: String,
    pub recipe: String,
    /// The pane was still showing the old startup block when the ready
    /// window had expired. Keep this visible on the recovery result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// Start the same task as a new bounded recovery attempt. Unlike the removed
/// `restart` command this deliberately replaces a live, blocked, or stuck
/// process. Its durable failure class decides whether recovery stays on the
/// same recipe, advances failed-work fallback routing, or waits for evidence.
pub fn retry(ctx: &Ctx, slug: &str, id: &str, reason: &str) -> Result<RetryOutcome> {
    retry_with_ticker(ctx, slug, id, reason, ticker::start, true)
}

/// Round recovery already holds the advance lock, so it must not replace and
/// wait for a ticker which may itself be waiting for that lock.
pub(crate) fn retry_during_advance(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    reason: &str,
) -> Result<RetryOutcome> {
    retry_with_ticker(ctx, slug, id, reason, ticker::ensure, false)
}

fn retry_with_ticker(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    reason: &str,
    ensure_ticker: fn(&Ctx<'_>) -> Result<()>,
    coordinator_unknown: bool,
) -> Result<RetryOutcome> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    // A crash after the attempt transition but before placement resumes the
    // same selected attempt. It must not spend another routing recovery.
    if record.escalation_pending {
        ensure_ticker(ctx)?;
        place_escalation(ctx, &project, &record)?;
        let placed = thread::load(&project, id)?;
        return Ok(RetryOutcome {
            thread: placed.id,
            attempt: placed.attempt,
            pane_id: placed.pane_id,
            recipe: placed.launch.recipe_id,
            screen: None,
        });
    }
    if record.kind == Kind::Adopted {
        bail!("retry_adopted: an adopted process has no launch recipe; use `thread rebind`");
    }
    if record.status == Status::Resolved {
        bail!("retry_resolved: {id} is resolved");
    }
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("retry_reason_missing: say why the attempt is being replaced");
    }
    let view = require_session(ctx, &project)?;
    let herdr = view.herdr.on_machine(record.machine_route());
    refuse_busy_retry(&herdr, &record)?;
    let screen = if record.error.starts_with("agent_not_ready:") {
        same_startup_screen(&herdr, &record)?
    } else if !record.startup_wait_started.is_empty() {
        Some(startup_screen(&herdr, &record.pane_id))
    } else {
        None
    };
    let task =
        std::fs::read_to_string(thread::task_path(&project, id)).context("retry_brief_missing")?;
    // Select before stopping anything: an exhausted policy leaves the current
    // process untouched.
    let input = crate::launch::ResolveInput {
        task: &task,
        workflow: if record.role.is_empty() {
            "lane"
        } else {
            &record.role
        },
        previous: Some(&record.launch),
        failure: Some(reason),
        source_truncation: record.launch.source_truncation.as_ref(),
        ..Default::default()
    };
    let selected = if coordinator_unknown {
        crate::launch::resolve_coordinator_retry(ctx, &project, &input, record.failure_class)
    } else {
        crate::launch::resolve_failure(ctx, &project, &input, record.failure_class)
    };
    let mut launch = selected?;
    launch.attempt = record.attempt.max(1).saturating_add(1);
    launch.brief_hash = record.launch.brief_hash.clone();

    let selected_recipe = launch.recipe_id.clone();
    thread::update_checked(&project, id, |t| {
        if t.attempt != record.attempt || t.pane_id != record.pane_id {
            bail!("retry_stale: thread changed while its replacement was prepared");
        }
        t.attempt = launch.attempt;
        t.agent = launch.kind.clone();
        let machine = if t.machine.is_empty() {
            "local"
        } else {
            &t.machine
        };
        t.placement_reason = format!(
            "retry on `{machine}` with recipe `{}`; machine kept from the previous attempt",
            launch.recipe_id
        );
        t.launch = launch;
        t.status = Status::Failed;
        t.prompt_pending = false;
        t.launch_attempts = 0;
        t.startup_wait_started.clear();
        t.bootstrap.clear();
        t.error.clear();
        t.last_failure = reason.to_string();
        t.cleanup_pending = false;
        t.cleanup_reason.clear();
        t.escalation_pending = true;
        Ok(())
    })?;

    ensure_ticker(ctx)?;
    place_escalation(ctx, &project, &thread::load(&project, id)?)?;
    let placed = thread::load(&project, id)?;
    Ok(RetryOutcome {
        thread: placed.id,
        attempt: placed.attempt,
        pane_id: placed.pane_id,
        recipe: selected_recipe,
        screen,
    })
}

/// Resume after a server/session interruption. This is not failed-work
/// recovery: it preserves the selected recipe and does not consume routing's
/// provider-failure budget. `pickup --start` is its only caller.
pub(crate) fn resume_interrupted(ctx: &Ctx, slug: &str, id: &str) -> Result<Thread> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if record.kind == Kind::Adopted || record.status == Status::Resolved {
        bail!("resume_refused: {id} has no resumable launch");
    }
    thread::update_checked(&project, id, |t| {
        if t.attempt != record.attempt {
            bail!("resume_stale: thread changed while pickup was recovering it");
        }
        t.attempt = t.attempt.max(1).saturating_add(1);
        t.launch.attempt = t.attempt;
        t.status = Status::Failed;
        t.prompt_pending = false;
        t.launch_attempts = 0;
        t.bootstrap.clear();
        Ok(())
    })?;
    ticker::start(ctx)?;
    let view = require_session(ctx, &project)?;
    let current = thread::load(&project, id)?;
    if current.is_remote()
        || current.worktree_path.is_empty() && current.kind == Kind::Worktree
        || current.pane_id.is_empty() && current.kind == Kind::Tab
    {
        return place_and_brief(ctx, &project, &view, id, true);
    }
    place_ade_tab(ctx, &project, &view, &current)?;
    let placed = thread::load(&project, id)?;
    write_brief(ctx, &project, &placed)?;
    finish_placement(&project, &view, id)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RebindOutcome {
    pub thread: String,
    pub pane_id: String,
    pub agent: String,
    pub state: String,
}

/// Point an existing thread at the live process already doing its work.
pub fn rebind(ctx: &Ctx, slug: &str, id: &str, pane_id: &str) -> Result<RebindOutcome> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if record.status == Status::Resolved {
        bail!("rebind_resolved: {id} is resolved");
    }
    let view = require_session(ctx, &project)?;
    let herdr = view.herdr.on_machine(record.machine_route());
    let agents = herdr
        .agent_list()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let agent = agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)
        .with_context(|| format!("rebind_no_agent: no live agent is in pane {pane_id}"))?;
    let managed = managed_git_folder(&project, &record);
    let expected_cwd = if record.worktree_path.is_empty() {
        record.cwd.as_str()
    } else {
        record.worktree_path.as_str()
    };
    let cwd_matches =
        expected_cwd.is_empty() || agent.cwd == expected_cwd || managed && agent.cwd == record.cwd;
    if !cwd_matches {
        let expected = if managed && record.cwd != expected_cwd {
            format!("{expected_cwd} or {}", record.cwd)
        } else {
            expected_cwd.to_string()
        };
        bail!(
            "rebind_identity_mismatch: pane {pane_id} runs in {}, expected {expected}",
            agent.cwd
        );
    }
    if !record.agent_name.is_empty() && agent.name != record.agent_name {
        bail!(
            "rebind_identity_mismatch: pane {pane_id} has agent `{}`, expected `{}`",
            agent.name,
            record.agent_name
        );
    }
    for other in thread::list(&project) {
        if other.id != id && other.status != Status::Resolved && other.pane_id == pane_id {
            bail!(
                "rebind_pane_owned: pane {pane_id} already belongs to {}",
                other.id
            );
        }
    }
    let process = herdr
        .pane_process_info(pane_id)
        .ok()
        .and_then(|info| info.identity(&agent.agent));
    if process.is_none() && record.identity.process.is_some() {
        bail!("rebind_identity_unknown: process identity is unavailable for pane {pane_id}");
    }
    let socket = project.coordinator().map(|c| c.socket).unwrap_or_default();
    let rebound = thread::update_checked(&project, id, |t| {
        if t.attempt != record.attempt {
            bail!("rebind_stale: thread changed during identity verification");
        }
        t.workspace_id = agent.workspace_id.clone();
        t.tab_id = agent.tab_id.clone();
        t.pane_id = agent.pane_id.clone();
        t.cwd = agent.cwd.clone();
        t.agent = agent.agent.clone();
        t.agent_name = agent.name.clone();
        t.status = Status::Open;
        t.prompt_pending = false;
        t.error.clear();
        t.cleanup_pending = false;
        t.cleanup_reason.clear();
        thread::bind_identity(t, &socket, agent, process.clone());
        Ok(())
    })?;
    let _ = herdr.pane_set_parent(
        pane_id,
        &project.coordinator().map(|c| c.pane_id).unwrap_or_default(),
    );
    report_thread_tokens(&herdr, &rebound, slug, Group::Working);
    Ok(RebindOutcome {
        thread: id.to_string(),
        pane_id: pane_id.to_string(),
        agent: agent.agent.clone(),
        state: agent.agent_status.clone(),
    })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CancelOutcome {
    pub thread: String,
    pub state: String,
    pub reason: String,
    pub pane: String,
    pub worktree: String,
    pub worktree_reason: Option<String>,
}

/// Stop a thread and retry any cleanup which an earlier cancellation could
/// not complete. The resolved record is written before external cleanup, so
/// no ticker can relaunch it while its session is unreachable.
pub fn cancel(ctx: &Ctx, slug: &str, id: &str, reason: &str) -> Result<CancelOutcome> {
    let project = Project::load(&ctx.root, slug)?;
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("cancel_reason_missing: say why {id} is being stopped");
    }
    let before = thread::load(&project, id)?;
    let recorded_reason = if before.cancellation_reason.is_empty() {
        reason.to_string()
    } else {
        before.cancellation_reason.clone()
    };
    let record = thread::update(&project, id, |t| {
        t.status = Status::Resolved;
        t.resolved_reason = "cancelled".into();
        t.cancellation_reason = recorded_reason.clone();
        t.prompt_pending = false;
        t.cleanup_pending = !t.tab_id.is_empty();
        t.cleanup_reason = "cancelled".into();
    })?;
    if let Some(view) = session_view(ctx, &project) {
        clear_thread_tokens(&view.herdr, &record);
    }

    let (pane, close_error) = if record.tab_id.is_empty() {
        ("already_gone".to_string(), None)
    } else {
        match close_pane(ctx, &project, &record) {
            Ok(true) => ("closed".to_string(), None),
            Ok(false) => ("already_gone".to_string(), None),
            Err(error) => ("cleanup_pending".to_string(), Some(format!("{error:#}"))),
        }
    };
    if close_error.is_none() {
        thread::update(&project, id, |t| {
            t.cleanup_pending = false;
            t.cleanup_reason.clear();
        })?;
    }

    let mut worktree = "not_applicable".to_string();
    let mut worktree_reason = close_error;
    if removable_folder(&project, &record) {
        if !worktree_exists(ctx, &project, &record)? {
            thread::update(&project, id, |t| t.worktree_path.clear())?;
            worktree = "removed".into();
        } else if pane == "cleanup_pending" {
            worktree = "kept".into();
        } else {
            let inspection = inspect_worktree_for_removal(ctx, &project, &record)?;
            let kept_reason = if !inspection.dirty.is_empty() {
                Some(format!(
                    "worktree_dirty: uncommitted changes in {}; not removing ({})",
                    record.worktree_path,
                    inspection.dirty.join(", ")
                ))
            } else {
                inspection.ignored_reason(&record.worktree_path)
            };
            if let Some(reason) = kept_reason {
                worktree = "kept".into();
                worktree_reason = Some(reason);
            } else {
                match remove_worktree(ctx, &project, &record) {
                    Ok(()) => {
                        thread::update(&project, id, |t| t.worktree_path.clear())?;
                        worktree = "removed".into();
                    }
                    Err(error) => {
                        worktree = "kept".into();
                        worktree_reason = Some(format!("{error:#}"));
                    }
                }
            }
        }
    }
    if pane != "cleanup_pending" {
        remove_finished_build_folder(ctx, &project, &record)?;
        remove_scratch_session(ctx, &record)?;
    }
    refresh_plan(ctx, &project);
    Ok(CancelOutcome {
        thread: id.to_string(),
        state: if pane == "cleanup_pending" {
            "cleanup_pending"
        } else {
            "cancelled"
        }
        .into(),
        reason: recorded_reason,
        pane,
        worktree,
        worktree_reason,
    })
}

/// Resolve through the same path as `thread resolve`, but make external
/// cleanup failure durable instead of failing the operation which ended the
/// round. The ticker can then retry the whole final-copy and cleanup path.
pub(crate) fn resolve_automatically(
    ctx: &Ctx,
    project: &Project,
    id: &str,
    reason: &str,
) -> ResolveOutcome {
    // Record the terminal lifecycle before touching Herdr or a worktree. A
    // process death at any later instruction leaves a ticker-visible retry.
    let before = thread::load(project, id).unwrap_or_default();
    if let Err(error) = thread::update(project, id, |t| {
        t.status = Status::Resolved;
        t.resolved_reason = "cleanup pending".into();
        t.prompt_pending = false;
        t.cleanup_pending = true;
        t.cleanup_reason = reason.to_string();
    }) {
        let detail = format!("could not record pending cleanup: {error:#}");
        refresh_plan(ctx, project);
        return ResolveOutcome {
            thread: id.to_string(),
            state: "cleanup_pending".into(),
            final_copy: "pending".into(),
            copy_notes: vec![detail.clone()],
            pane: "cleanup_pending".into(),
            worktree: if before.worktree_path.is_empty() {
                "not_recorded"
            } else {
                "kept"
            }
            .into(),
            worktree_path: before.worktree_path,
            worktree_reason: Some(detail),
            branch: before.branch,
        };
    }
    let attempted = resolve(ctx, &project.slug, id, &ResolveArgs::default());
    match attempted {
        Ok(outcome) => {
            let _ = thread::update(project, id, |t| {
                t.resolved_reason = reason.to_string();
                t.cleanup_pending = false;
                t.cleanup_reason.clear();
            });
            outcome
        }
        Err(error) => {
            let detail = format!("{error:#}");
            let before = thread::load(project, id).unwrap_or_default();
            refresh_plan(ctx, project);
            ResolveOutcome {
                thread: id.to_string(),
                state: "cleanup_pending".into(),
                final_copy: "pending".into(),
                copy_notes: vec![detail.clone()],
                pane: "cleanup_pending".into(),
                worktree: if before.worktree_path.is_empty() {
                    "not_recorded"
                } else {
                    "kept"
                }
                .into(),
                worktree_path: before.worktree_path,
                worktree_reason: Some(detail),
                branch: before.branch,
            }
        }
    }
}

/// Retry every durable cleanup left by cancellation or automatic resolution.
/// Closed rounds are also reconciled so a process death between closing the
/// round and marking its first thread cannot strand an unmarked cleanup.
pub(crate) fn retry_pending_cleanup(ctx: &Ctx, project: &Project) -> Result<()> {
    for record in thread::list(project)
        .into_iter()
        .filter(|record| record.cleanup_pending)
    {
        if !record.cancellation_reason.is_empty() {
            cancel(ctx, &project.slug, &record.id, &record.cancellation_reason)?;
        } else {
            let reason = if record.cleanup_reason.is_empty() {
                "automatic"
            } else {
                &record.cleanup_reason
            };
            resolve_automatically(ctx, project, &record.id, reason);
        }
    }

    for round in crate::round::checked_list(project)?
        .into_iter()
        .filter(|round| round.cleanup_pending)
    {
        let reason = if round.phase == crate::contracts::RoundPhase::Merged {
            "merged"
        } else {
            "cancelled"
        };
        let mut ids: Vec<_> = round
            .manifest
            .members
            .into_iter()
            .map(|member| member.thread)
            .collect();
        if let Some(reviewer) = round.reviewer
            && !ids.contains(&reviewer)
        {
            ids.push(reviewer);
        }
        for id in ids {
            if thread::load(project, &id).is_ok_and(|record| record.status != Status::Resolved) {
                resolve_automatically(ctx, project, &id, reason);
            }
        }
        crate::round::finish_cleanup_marker(project, &round.round)?;
    }
    Ok(())
}

/// A report-only code lane has nothing to review or land. Once its sealed
/// report is available, close it immediately; changed lanes remain visible so
/// the coordinator can put them in a round.
pub(crate) fn resolve_report_only(ctx: &Ctx, project: &Project) {
    let events = crate::events::list(project);
    for record in thread::list(project) {
        if record.status == Status::Resolved
            || record.role == "reviewer"
            || record.base.is_empty()
            || !crate::threads::carrying_rounds(project, &record.id).is_empty()
        {
            continue;
        }
        let unchanged = events
            .iter()
            .filter(|event| event.thread == record.id && event.attempt == record.attempt.max(1))
            .max_by(|left, right| (&left.created, &left.id).cmp(&(&right.created, &right.id)))
            .and_then(|event| event.payload.done.as_ref())
            .is_some_and(|done| done.sha == record.base);
        if unchanged {
            resolve_automatically(ctx, project, &record.id, "report-only");
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "delivery")]
pub enum PromptOutcome {
    Queued { attempt: u32 },
    Sent { attempt: u32, agent_state: String },
}

fn awaiting_follow_up(record: &Thread) -> bool {
    let attempt = record.attempt.max(1);
    record.follow_ups.iter().any(|follow_up| {
        follow_up.attempt == attempt
            && matches!(
                follow_up.state,
                FollowUpState::Queued | FollowUpState::Uncertain
            )
    })
}

fn awaiting_bootstrap(record: &Thread) -> bool {
    record.kind != Kind::Adopted
        && !record.launch.kind.is_empty()
        && record.bootstrap != "acknowledged"
}

/// Box lane state has no Mac round records. Publish the barriers before the
/// correction prompt can reach its pane, or refuse to send the prompt.
pub(crate) fn sync_box_corrections(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if !record.is_remote() || record.machine_route() == crate::contracts::MACHINE_LOCAL {
        return Ok(());
    }
    let events = crate::ops::correction_barriers(project, &record.id)?;
    if events.is_empty() {
        return Ok(());
    }
    let bin = ctx.env.herdr_bin();
    let profile =
        remote::machine_profile(ctx.runner, &bin, &ctx.config_dir, record.machine_route())?;
    let machine = remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    let dir = format!("{}/{}/.state/corrections", machine.root, project.slug);
    let path = format!("{dir}/{}.toml", record.id);
    let content = toml::to_string(&crate::ops::BoxCorrections {
        thread: record.id.clone(),
        attempt: record.attempt.max(1),
        events,
    })?;
    let script = format!(
        "set -e; mkdir -p {dir}; tmp=$(mktemp {dir}/.correction.XXXXXXXX); trap 'rm -f \"$tmp\"' EXIT; cat > \"$tmp\"; mv \"$tmp\" {path}",
        dir = remote::quote(&dir),
        path = remote::quote(&path),
    );
    let out = remote::ssh(
        ctx.runner,
        &profile.target,
        &script,
        Some(&content),
        std::time::Duration::from_secs(30),
    )?;
    if !out.success() {
        bail!(
            "could not publish correction barrier for {}: {}",
            record.id,
            out.error_text()
        );
    }
    Ok(())
}

fn latest_waiting_event_id(
    events: &[crate::contracts::Event],
    thread: &str,
    attempt: u32,
) -> Option<String> {
    events
        .iter()
        .filter(|event| {
            event.thread == thread && event.attempt == attempt && event.payload.waiting.is_some()
        })
        .max_by(|left, right| (&left.created, &left.id).cmp(&(&right.created, &right.id)))
        .map(|event| event.id.clone())
}

pub(crate) fn record_answered_wait(
    project: &Project,
    thread_id: &str,
    attempt: u32,
    waiting: &str,
) -> Result<()> {
    if waiting.is_empty() {
        return Ok(());
    }
    thread::update_checked(project, thread_id, |thread| {
        if thread.attempt.max(1) != attempt {
            bail!("prompt_attempt_changed: {thread_id} moved past attempt {attempt}");
        }
        thread.answered_waiting_event = waiting.to_string();
        Ok(())
    })?;
    Ok(())
}

/// Sends a follow-up. The one sender that does not use the ready-for-a-prompt
/// predicate: agents queue a message that arrives while they work.
pub fn prompt(ctx: &Ctx, slug: &str, id: &str, text: &str) -> Result<PromptOutcome> {
    let project = Project::load(&ctx.root, slug)?;
    let mut record = thread::load(&project, id)?;
    let text = text.trim();
    if text.is_empty() {
        bail!("the text is empty");
    }
    match record.status {
        Status::Resolved => return Err(crate::refusal::error(format!("{id} is resolved"))),
        Status::Failed => return Err(crate::refusal::error(format!("{id} is gone"))),
        Status::Starting | Status::Open => {}
    }
    // The brief and every follow-up have one ordered delivery path. Once one
    // message is queued, later messages join it until the ticker drains them.
    if record.status == Status::Starting
        || record.prompt_pending
        || awaiting_bootstrap(&record)
        || awaiting_follow_up(&record)
    {
        let events_before_send = crate::round::sealed_events(&project)?;
        // Keep the queued message invisible to the ticker until every round
        // it affects is durably held. Working also closes the gap between this
        // thread update and that hold for an already accepted reviewer.
        let previous_group = record.last_group.clone();
        thread::update(&project, id, |thread| {
            thread.last_group = Group::Working.token().to_string();
        })?;
        if let Err(error) = crate::round::hold_for_follow_up(ctx, &project, id, &events_before_send)
        {
            thread::update(&project, id, |thread| {
                thread.last_group = previous_group;
            })?;
            return Err(error);
        }
        let mut queued = false;
        record = thread::update_checked(&project, id, |thread| {
            match thread.status {
                Status::Resolved => {
                    return Err(crate::refusal::error(format!("{id} is resolved")));
                }
                Status::Failed => return Err(crate::refusal::error(format!("{id} is gone"))),
                Status::Starting | Status::Open => {}
            }
            if thread.status == Status::Starting
                || thread.prompt_pending
                || awaiting_bootstrap(thread)
                || awaiting_follow_up(thread)
            {
                let attempt = thread.attempt.max(1);
                thread.follow_ups.push(FollowUp {
                    attempt,
                    text: text.to_string(),
                    state: FollowUpState::Queued,
                    waiting_event: latest_waiting_event_id(&events_before_send, id, attempt)
                        .unwrap_or_default(),
                });
                queued = true;
            }
            Ok(())
        })?;
        if queued {
            return Ok(PromptOutcome::Queued {
                attempt: record.attempt.max(1),
            });
        }
    }
    let view = require_session(ctx, &project)?;
    let (agents, _) = lists_for(&view, &record)?;
    let kind = if record.launch.kind.is_empty() {
        &record.agent
    } else {
        &record.launch.kind
    };
    let resumable = crate::adapters::declaration(&ctx.config_dir, kind)
        .is_ok_and(|adapter| adapter.blocked_error_resumable);
    let state = prompt_state(&record, &agents, resumable)?;
    let herdr = view.herdr.on_machine(record.machine_route());
    let events_before_send = crate::round::sealed_events(&project)?;
    // Mark the work before invalidating earlier round evidence. The merge
    // boundary sees either this working state or the durable hold. If an
    // already committed intent refuses the hold, restore the prior projection
    // and never deliver the text.
    let previous_group = record.last_group.clone();
    thread::update(&project, id, |thread| {
        thread.last_group = Group::Working.token().to_string();
    })?;
    if let Err(error) = crate::round::hold_for_follow_up(ctx, &project, id, &events_before_send) {
        thread::update(&project, id, |thread| {
            thread.last_group = previous_group;
        })?;
        return Err(error);
    }
    sync_box_corrections(ctx, &project, &record)?;
    // Herdr rejects `agent prompt` for every blocked pane. An adapter-owned
    // error screen is different from an approval dialog: submit through the
    // pane so the adapter's input hook clears its block. A blocked lane with
    // no durable error still refuses in `prompt_state` above.
    if state == "blocked" {
        herdr
            .pane_submit_text(&record.pane_id, text)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        thread::update(&project, id, |t| t.error.clear())?;
    } else {
        herdr
            .agent_prompt(&record.pane_id, text)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    }
    let attempt = record.attempt.max(1);
    let waiting = latest_waiting_event_id(&events_before_send, id, attempt).unwrap_or_default();
    record_answered_wait(&project, id, attempt, &waiting)?;
    Ok(PromptOutcome::Sent {
        attempt,
        agent_state: state,
    })
}

/// The state a follow-up may be sent in, or the refusal.
pub fn prompt_state(
    record: &Thread,
    agents: &[Agent],
    blocked_error_resumable: bool,
) -> Result<String> {
    let agent = agents
        .iter()
        .find(|a| thread::agent_matches(record, a))
        .with_context(|| format!("no agent is detected in {}'s pane; text is never typed at a bare shell prompt (try `thread retry`)", record.id))?;
    match agent.agent_status.as_str() {
        "blocked" if !blocked_error_resumable || record.error.is_empty() => {
            Err(crate::refusal::error(format!(
                "agent_blocked: {} is waiting on the user in its pane ({})",
                record.id, record.pane_id
            )))
        }
        "unknown" => Err(crate::refusal::error(format!(
            "{}'s agent state is unknown; not sending",
            record.id
        ))),
        state => Ok(state.to_string()),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AttestOutcome {
    pub thread: String,
    pub event: String,
    pub artifact: String,
    pub sha: Option<String>,
    pub coordinator: String,
    pub reason: String,
}

/// Seal completion evidence from a resolved lane's preserved report draft.
pub fn attest(ctx: &Ctx, slug: &str, id: &str, reason: &str) -> Result<AttestOutcome> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(crate::refusal::error(
            "attest_reason_missing: --reason is required",
        ));
    }
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if record.status != Status::Resolved {
        return Err(crate::refusal::error(format!(
            "attest_not_resolved: {id} is not resolved"
        )));
    }
    if !record.cancellation_reason.is_empty() {
        return Err(crate::refusal::error(format!(
            "attest_cancelled: {id} was cancelled"
        )));
    }
    let attempt = record.attempt.max(1);
    if crate::round::sealed_events(&project)?
        .iter()
        .any(|event| event.thread == id && event.attempt == attempt && event.payload.done.is_some())
    {
        return Err(crate::refusal::error(format!(
            "attest_already_done: {id} already has sealed done evidence for attempt {attempt}"
        )));
    }

    let draft_path =
        (!record.thread_dir.is_empty()).then(|| Path::new(&record.thread_dir).join("report.md"));
    let historical_path = thread::home_report_path(&project, id);
    let report_path = draft_path
        .iter()
        .chain(std::iter::once(&historical_path))
        .find(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()))
        .ok_or_else(|| {
            crate::refusal::error(format!(
                "attest_report_missing: {} has no preserved report draft",
                id
            ))
        })?;
    let bytes = std::fs::read(report_path)
        .with_context(|| format!("could not read report {}", report_path.display()))?;
    let actual_hash = thread::sha256_hex(&bytes);
    if record.report_hash.is_empty() || actual_hash != record.report_hash {
        return Err(crate::refusal::error(format!(
            "attest_report_mismatch: stored report hashes to {actual_hash}, record names {}",
            if record.report_hash.is_empty() {
                "no hash"
            } else {
                &record.report_hash
            }
        )));
    }

    let git_folder = [&record.worktree_path, &record.cwd]
        .into_iter()
        .filter(|path| !path.is_empty())
        .find(|path| Path::new(path).join(".git").exists());
    let sha = git_folder
        .map(|folder| crate::git::rev_parse(ctx.runner, folder, "HEAD"))
        .transpose()
        .context("attest_git_head: could not read the lane folder's HEAD")?;
    let coordinator = project
        .coordinator()
        .filter(|coordinator| !coordinator.pane_id.is_empty())
        .ok_or_else(|| {
            crate::refusal::error("attest_coordinator_missing: project has no coordinator binding")
        })?;
    let coordinator_name = if coordinator.agent_name.is_empty() {
        coordinator.pane_id.clone()
    } else {
        coordinator.agent_name.clone()
    };
    let artifact = crate::events::store_artifact(&project, &bytes)?;
    let event_id = format!("{id}-{attempt}-attest");
    let event = crate::contracts::Event {
        id: event_id.clone(),
        op: event_id.clone(),
        thread: id.to_string(),
        attempt,
        round: None,
        recipient: crate::contracts::Recipient {
            pane: coordinator.pane_id.clone(),
            coordinator_attempt: coordinator.attempt(),
        },
        created: project::now(),
        payload: crate::contracts::EventPayload {
            done: Some(crate::contracts::DonePayload {
                sha: sha.clone().unwrap_or_default(),
                report_path: crate::events::artifact_path(&project, &artifact)
                    .to_string_lossy()
                    .into_owned(),
                artifact: artifact.clone(),
                attestation: Some(crate::contracts::Attestation {
                    coordinator: coordinator_name.clone(),
                    reason: reason.to_string(),
                }),
            }),
            ..crate::contracts::EventPayload::default()
        },
    };
    crate::events::seal_create_if_absent(&project, &event)?;
    refresh_plan(ctx, &project);
    Ok(AttestOutcome {
        thread: id.to_string(),
        event: event_id,
        artifact,
        sha,
        coordinator: coordinator_name,
        reason: reason.to_string(),
    })
}

pub(crate) fn done_attestation(
    project: &Project,
    record: &Thread,
) -> Option<crate::contracts::Attestation> {
    crate::events::list(project)
        .into_iter()
        .filter(|event| event.thread == record.id && event.attempt == record.attempt.max(1))
        .filter_map(|event| {
            let attestation = event.payload.done?.attestation?;
            Some((event.created, event.id, attestation))
        })
        .max_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)))
        .map(|(_, _, attestation)| attestation)
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
    pub skip_copy: bool,
    pub discard_uncopied: bool,
    /// Leave the lane's pane and tab open (the idle agent still runs).
    pub keep_pane: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResolveOutcome {
    pub thread: String,
    pub state: String,
    pub final_copy: String,
    pub copy_notes: Vec<String>,
    pub pane: String,
    pub worktree: String,
    pub worktree_path: String,
    pub worktree_reason: Option<String>,
    pub branch: String,
}

impl ResolveOutcome {
    pub fn message(&self, slug: &str) -> String {
        if self.state == "open" {
            return format!(
                "{} is open again. Nothing was started; `thread retry {slug} {} --reason <why>` brings its agent back.\n",
                self.thread, self.thread
            );
        }
        let mut message = String::new();
        if self.final_copy == "partial" {
            message.push_str("the final copy was partial:\n");
            for note in &self.copy_notes {
                message.push_str(&format!("  - {note}\n"));
            }
        }
        message.push_str(&format!("{} resolved.\n", self.thread));
        match self.pane.as_str() {
            "kept_open" => message.push_str(
                "Its pane and tab were left open (--keep-pane); close them in herdr when you are done.\n",
            ),
            "closed" => message.push_str("Its pane and tab were closed.\n"),
            "already_gone" => message.push_str("Its pane and tab were already gone.\n"),
            _ => {}
        }
        match self.worktree.as_str() {
            "not_recorded" => message
                .push_str("No worktree was recorded for it, so there is nothing to remove.\n"),
            "removed" => message.push_str(&format!(
                "The worktree {} was removed; the branch {} was kept.\n",
                self.worktree_path, self.branch
            )),
            "kept" => message.push_str(&format!(
                "The worktree {} was kept: {}.\n",
                self.worktree_path,
                self.worktree_reason.as_deref().unwrap_or("reason unknown")
            )),
            _ => {}
        }
        message
    }
}

pub fn resolve(ctx: &Ctx, slug: &str, id: &str, args: &ResolveArgs) -> Result<ResolveOutcome> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if args.reopen {
        if record.status != Status::Resolved {
            bail!("{id} is not resolved");
        }
        let reopened = thread::update(&project, id, |t| {
            t.status = Status::Open;
            t.resolved_reason.clear();
        })?;
        refresh_plan(ctx, &project);
        return Ok(ResolveOutcome {
            thread: id.to_string(),
            state: "open".into(),
            final_copy: "not_run".into(),
            copy_notes: Vec::new(),
            pane: "unchanged".into(),
            worktree: "unchanged".into(),
            worktree_path: reopened.worktree_path,
            worktree_reason: None,
            branch: reopened.branch,
        });
    }
    if args.skip_copy && args.discard_uncopied {
        bail!("--skip-copy cannot be combined with --discard-uncopied");
    }
    crate::round::require_resolvable(&project, id)?;

    let removable = removable_folder(&project, &record);
    let already_removed = removable && !worktree_exists(ctx, &project, &record)?;

    // Every path that resolves a thread performs a final copy first.
    let mut removal_refusal = None;
    let (final_copy, copy_notes) = if args.skip_copy {
        ("skipped".to_string(), Vec::new())
    } else {
        let copied = final_copy(ctx, &project, &record);
        match copied.outcome {
            CopyOutcome::Complete => ("complete".to_string(), Vec::new()),
            CopyOutcome::Partial(notes) => {
                if removable && !already_removed && !args.discard_uncopied {
                    removal_refusal = Some(
                        "copy_incomplete: the worktree was kept because some files were not copied; pass --discard-uncopied to accept that loss"
                            .to_string(),
                    );
                }
                ("partial".to_string(), notes)
            }
            CopyOutcome::Failed(error) => {
                bail!(
                    "the final copy failed ({error}); not resolving. `--skip-copy` resolves without it."
                );
            }
        }
    };

    let mut pane_closed = false;
    let mut worktree_removed = false;
    if already_removed {
        thread::update(&project, id, |t| t.worktree_path.clear())?;
        worktree_removed = true;
    } else if removable {
        if args.keep_pane {
            removal_refusal = Some(
                "worktree_in_use: the worktree was kept because --keep-pane leaves its pane open"
                    .into(),
            );
        }
        if removal_refusal.is_none() {
            removal_refusal = finished_worktree_reason(ctx, &project, &record)?;
        }
        if removal_refusal.is_none() {
            let inspection = inspect_worktree_for_removal(ctx, &project, &record)?;
            if !inspection.dirty.is_empty() {
                return Err(crate::refusal::error(format!(
                    "worktree_dirty: uncommitted changes in {}; not removing ({})",
                    record.worktree_path,
                    inspection.dirty.join(", ")
                )));
            }
            removal_refusal = inspection.ignored_reason(&record.worktree_path);
        }
        if removal_refusal.is_none() {
            removal_in_use_gate(ctx, &project, &record)?;
            // Stop the idle agent before removing its current directory. This
            // also makes the raw box-side `git worktree remove` independent of
            // Herdr workspace ownership.
            pane_closed = close_pane(ctx, &project, &record)?;
            remove_worktree(ctx, &project, &record)?;
            thread::update(&project, id, |t| t.worktree_path.clear())?;
            worktree_removed = true;
        }
    }
    let resolved = thread::update(&project, id, |t| {
        t.status = Status::Resolved;
        t.resolved_reason = "manual".into();
        t.prompt_pending = false;
    })?;
    if let Some(view) = session_view(ctx, &project) {
        clear_thread_tokens(&view.herdr, &resolved);
    }
    let pane_closed = if args.keep_pane || pane_closed {
        pane_closed
    } else {
        close_pane(ctx, &project, &resolved)?
    };
    remove_finished_build_folder(ctx, &project, &resolved)?;
    remove_scratch_session(ctx, &resolved)?;
    let pane = if args.keep_pane {
        "kept_open"
    } else if pane_closed {
        "closed"
    } else {
        "already_gone"
    };
    let (worktree, worktree_reason) = if worktree_removed {
        ("removed", None)
    } else if let Some(reason) = removal_refusal {
        ("kept", Some(reason))
    } else if resolved.kind == Kind::Worktree || managed_git_folder(&project, &resolved) {
        ("not_recorded", None)
    } else {
        ("not_applicable", None)
    };
    refresh_plan(ctx, &project);
    Ok(ResolveOutcome {
        thread: id.to_string(),
        state: "resolved".into(),
        final_copy,
        copy_notes,
        pane: pane.into(),
        worktree: worktree.into(),
        worktree_path: if worktree_removed {
            record.worktree_path
        } else {
            resolved.worktree_path
        },
        worktree_reason,
        branch: resolved.branch,
    })
}

/// The shared plan refresh at a thread lifecycle change. A refresh failure is
/// reported on its own line; it never fails the lifecycle operation
/// (SPEC-talk §6.5).
pub fn refresh_plan(ctx: &Ctx, project: &Project) {
    if let Err(e) = crate::project::refresh_page(project) {
        eprintln!("note: the task list refresh failed: {e:#}");
    }
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
    let machine = remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    let quoted = remote::quote(&name);
    let script = remote::with_path(
        &machine.path,
        &format!(
            "state=$(herdr session list --json | python3 -c 'import json,sys; n=sys.argv[1]; rows=json.load(sys.stdin).get(\"sessions\",[]); r=next((r for r in rows if r.get(\"name\")==n),None); print(\"missing\" if r is None else (\"running\" if r.get(\"running\") else \"stopped\"))' {quoted}); case \"$state\" in running) herdr session stop {quoted} --json >/dev/null; herdr session delete {quoted} --json >/dev/null;; stopped) herdr session delete {quoted} --json >/dev/null;; missing) :;; *) exit 1;; esac"
        ),
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

fn refuse_busy_retry(herdr: &Herdr<'_>, record: &Thread) -> Result<()> {
    let still_starting = (record.status == Status::Starting
        || !record.startup_wait_started.is_empty())
        && (record.startup_wait_started.is_empty()
            || (thread::seconds_since(&record.startup_wait_started, jiff::Timestamp::now()).max(0)
                as u64
                * 1000)
                < record.launch.ready_timeout_ms);
    let agent_state = herdr
        .agent_list()?
        .into_iter()
        .find(|agent| {
            thread::agent_matches(record, agent)
                || (agent.pane_id == record.pane_id
                    && agent.tab_id == record.tab_id
                    && agent.workspace_id == record.workspace_id
                    && agent.cwd == record.cwd)
        })
        .map(|agent| agent.agent_status);
    if still_starting
        || agent_state.as_deref() == Some("working")
        || (record.prompt_pending
            && agent_state
                .as_deref()
                .is_some_and(crate::herdr::ready_state))
    {
        let state = if still_starting {
            "starting"
        } else if agent_state.as_deref() == Some("working") {
            "working"
        } else {
            "ready for its brief"
        };
        let screen = if record.pane_id.is_empty() {
            "no pane yet".into()
        } else {
            startup_screen(herdr, &record.pane_id)
        };
        bail!(
            "retry_refused: {} is still {state}; screen: {screen}. Wait for it to finish or become stuck before retrying",
            record.id
        );
    }
    Ok(())
}

fn same_startup_screen(herdr: &Herdr<'_>, record: &Thread) -> Result<Option<String>> {
    let screen = startup_screen(herdr, &record.pane_id);
    if screen.starts_with("screen unavailable:") {
        bail!(
            "startup_screen_unknown: cannot tell whether {} is still blocked: {screen}",
            record.id
        );
    }
    if record
        .error
        .strip_prefix("agent_not_ready: screen: ")
        .and_then(|text| text.rsplit_once("; herdr: "))
        .is_some_and(|(previous, _)| previous == screen)
    {
        return Ok(Some(screen));
    }
    bail!(
        "startup_screen_changed: {} now shows: {screen}. Check the pane before replacing it",
        record.id
    )
}

/// A startup refusal leaves the pane visible for diagnosis and recovery. No
/// automatic relaunch should erase an interactive question before it is read.
pub(crate) fn startup_screen(herdr: &Herdr<'_>, pane: &str) -> String {
    match herdr.pane_read_text(pane, "visible") {
        Ok(text) => {
            let lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
            let excerpt = lines.take(12).collect::<Vec<_>>().join(" | ");
            if excerpt.is_empty() {
                "no readable screen".into()
            } else {
                excerpt.chars().take(800).collect()
            }
        }
        Err(error) => format!("screen unavailable: {error}"),
    }
}

/// Marks a start failed, removes its Working metadata, and closes everything
/// the attempt opened. The failed state is durable even when cleanup itself
/// reports an error, so no view can keep presenting the attempt as Working.
pub(crate) fn fail_start(
    ctx: &Ctx,
    project: &Project,
    id: &str,
    reason: &str,
    class: crate::contracts::FailureClass,
    recover: bool,
) -> Result<Thread> {
    let provider_kind = None;
    let (recovery, recovery_error) = if !recover {
        (None, None)
    } else {
        let record = thread::load(project, id)?;
        let task = std::fs::read_to_string(thread::task_path(project, id)).unwrap_or_default();
        match crate::launch::resolve_failure(
            ctx,
            project,
            &crate::launch::ResolveInput {
                task: &task,
                workflow: &record.role,
                previous: Some(&record.launch),
                failure: Some(reason),
                source_truncation: record.launch.source_truncation.as_ref(),
                ..Default::default()
            },
            class,
        ) {
            Ok(selected) => (Some(selected), None),
            Err(error) => (None, Some(format!("WAITING: {error:#}"))),
        }
    };
    let failed = thread::update(project, id, |t| {
        t.status = Status::Failed;
        t.prompt_pending = false;
        t.startup_wait_started.clear();
        t.error = recovery_error.clone().unwrap_or_else(|| reason.to_string());
        t.failure_class = class;
        t.provider_failure_kind = provider_kind.clone();
        if let Some(mut selected) = recovery.clone() {
            t.attempt = t.attempt.max(1).saturating_add(1);
            selected.attempt = t.attempt;
            selected.brief_hash = t.launch.brief_hash.clone();
            t.launch = selected;
            // This transition is still evidence about the attempt that just
            // launched. Placement resets the counter for the next attempt.
            t.escalation_pending = true;
        }
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
        bail!(
            "cleanup_session_unreachable: the pane for {} was not closed; retry recovery when `{}` is reachable",
            record.id,
            project.slug
        );
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
    if !owns_pane {
        return Ok(false);
    }
    let result = if owns_workspace && !holds_something_else {
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

/// Finds the final report hash and copies real deliverables, without making a
/// second report. A box lane's sealed artifact arrives through the Mac courier;
/// until then the copy is partial.
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

/// Why a recorded worktree is not finished yet. `None` means one of the
/// durable completion conditions permits removal.
pub(crate) fn finished_worktree_reason(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
) -> Result<Option<String>> {
    if managed_git_folder(project, record) {
        let done = crate::events::list(project)
            .into_iter()
            .filter(|event| event.thread == record.id && event.attempt == record.attempt.max(1))
            .find_map(|event| event.payload.done);
        let Some(done) = done else {
            return Ok(Some(
                "work_not_done: no sealed done event exists for this folder".into(),
            ));
        };
        let head = crate::git::rev_parse(ctx.runner, &record.worktree_path, "HEAD")?;
        return Ok((head != done.sha).then(|| {
            format!(
                "work_not_done: folder HEAD is {head}, not the sealed done commit {}",
                done.sha
            )
        }));
    }
    for round in crate::round::checked_list(project)? {
        let member = round
            .manifest
            .members
            .iter()
            .any(|member| member.thread == record.id);
        if member && round.phase.closed() {
            return Ok(None);
        }
        if round.reviewer.as_deref() == Some(record.id.as_str())
            && (round.verdict.is_some() || round.phase.closed())
        {
            return Ok(None);
        }
    }

    if record.branch.is_empty() {
        return Ok(Some(
            "work_not_done: no lane branch is recorded and no closed round contains the thread"
                .into(),
        ));
    }
    let Some(lane_head) = crate::git::branch_head(ctx.runner, &record.repo, &record.branch)? else {
        return Ok(Some(format!(
            "work_not_done: branch `{}` is missing and no closed round contains the thread",
            record.branch
        )));
    };
    let integration = crate::git::symbolic_head(ctx.runner, &record.repo)?;
    let integration_head = crate::git::branch_head(ctx.runner, &record.repo, &integration)?
        .with_context(|| format!("integration branch `{integration}` is missing"))?;
    if crate::git::is_ancestor(ctx.runner, &record.repo, &lane_head, &integration_head)? {
        Ok(None)
    } else {
        Ok(Some(format!(
            "work_not_done: branch `{}` is not on integration branch `{integration}` and no closed round contains the thread",
            record.branch
        )))
    }
}

pub(crate) fn report_artifact_stored(project: &Project, record: &Thread) -> Result<bool> {
    let attempt = record.attempt.max(1);
    for event in crate::round::sealed_events(project)?
        .into_iter()
        .filter(|event| event.thread == record.id && event.attempt == attempt)
    {
        let Some(done) = event.payload.done else {
            continue;
        };
        let path = crate::events::artifact_path(project, &done.artifact);
        match std::fs::read(&path) {
            Ok(bytes) if thread::sha256_hex(&bytes) == done.artifact => return Ok(true),
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not read report artifact {}", path.display()));
            }
        }
    }
    Ok(false)
}

pub(crate) fn inspect_worktree_for_removal(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
) -> Result<crate::worktrees::Inspection> {
    let report_artifact_stored = report_artifact_stored(project, record)?;
    if managed_git_folder(project, record) {
        let disposable =
            crate::worktrees::disposable(&ctx.config_dir, project, &record.worktree_path)?;
        return crate::worktrees::inspect_local(
            ctx.runner,
            &record.worktree_path,
            &record.worktree_path,
            &disposable,
            report_artifact_stored,
        );
    }
    let disposable = crate::worktrees::disposable(&ctx.config_dir, project, &record.repo)?;
    if !record.is_remote() {
        return crate::worktrees::inspect_local(
            ctx.runner,
            &record.repo,
            &record.worktree_path,
            &disposable,
            report_artifact_stored,
        );
    }
    let profile = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?;
    let machine = remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    crate::worktrees::inspect_remote(
        ctx.runner,
        &profile.target,
        &machine.path,
        &record.worktree_path,
        &disposable,
        report_artifact_stored,
    )
}

/// Whether the recorded checkout still exists. A missing checkout is the
/// desired cleanup state, so its stale Git registration is pruned immediately.
/// A box transport failure remains an error: absence is only accepted after a
/// successful answer from that machine.
fn worktree_exists(ctx: &Ctx, project: &Project, record: &Thread) -> Result<bool> {
    if !record.is_remote() {
        return match std::fs::symlink_metadata(&record.worktree_path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !managed_git_folder(project, record) {
                    crate::git::worktree_prune(ctx.runner, &record.repo)?;
                }
                Ok(false)
            }
            Err(error) => Err(error)
                .with_context(|| format!("could not inspect worktree {}", record.worktree_path)),
        };
    }

    let (settings, _) = project.read_project_md()?;
    let profile = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?;
    let (box_repo, _) = box_repo_row(&ctx.config_dir, &settings, &profile.label, &record.repo)?;
    let machine = remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    let present = "__HERDR_WORKTREE_PRESENT__";
    let removed = "__HERDR_WORKTREE_REMOVED__";
    let script = remote::with_path(
        &machine.path,
        &format!(
            "if [ -e {} ]; then printf '{present}\\n'; else printf '{removed}\\n'; fi",
            remote::quote(&record.worktree_path),
        ),
    );
    let out = remote::ssh(
        ctx.runner,
        &profile.target,
        &script,
        None,
        Duration::from_secs(20),
    )?;
    if !out.success() {
        bail!("box worktree check failed: {}", out.error_text());
    }
    if out.stdout.lines().any(|line| line == present) {
        return Ok(true);
    }
    if !out.stdout.lines().any(|line| line == removed) {
        bail!("box worktree check returned no presence answer");
    }
    let cleanup = remote::with_path(
        &machine.path,
        &format!(
            "cd {} && git worktree prune --expire=now",
            remote::quote(&box_repo),
        ),
    );
    let out = remote::ssh(
        ctx.runner,
        &profile.target,
        &cleanup,
        None,
        Duration::from_secs(20),
    )?;
    if !out.success() {
        bail!("box worktree prune failed: {}", out.error_text());
    }
    Ok(false)
}

/// Never forces. Git's refusal is reported unchanged.
fn remove_worktree(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if managed_git_folder(project, record) {
        match std::fs::remove_dir_all(&record.worktree_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("could not remove managed folder {}", record.worktree_path)
                });
            }
        }
        return Ok(());
    }
    if !record.is_remote() {
        return remove_ade_worktree(ctx, project, record);
    }
    if record.worktree_path.is_empty() {
        bail!("{} has no recorded worktree", record.id);
    }
    let (settings, _) = project.read_project_md()?;
    let profile = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?;
    let (box_repo, _) = box_repo_row(&ctx.config_dir, &settings, &profile.label, &record.repo)?;
    let machine = remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    let script = remote::with_path(
        &machine.path,
        &format!(
            "cd {} && if [ -e {} ]; then git worktree remove {} || {{ [ ! -e {} ] && git worktree prune --expire=now; }}; else git worktree prune --expire=now; fi",
            remote::quote(&box_repo),
            remote::quote(&record.worktree_path),
            remote::quote(&record.worktree_path),
            remote::quote(&record.worktree_path),
        ),
    );
    let out = remote::ssh(
        ctx.runner,
        &profile.target,
        &script,
        None,
        Duration::from_secs(20),
    )?;
    if !out.success() {
        bail!("{}", out.error_text());
    }
    Ok(())
}

fn remove_finished_build_folder(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if !record.is_remote() {
        return Ok(());
    }
    let profile = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?;
    let machine = remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    let build = format!("{}/{}-{}", machine.build, project.slug, record.id);
    let script = remote::with_path(
        &machine.path,
        &format!("rm -rf -- {}", remote::quote(&build)),
    );
    let out = remote::ssh(
        ctx.runner,
        &profile.target,
        &script,
        None,
        Duration::from_secs(20),
    )?;
    if !out.success() {
        bail!(
            "could not remove lane build folder {build}: {}",
            out.error_text()
        );
    }
    Ok(())
}

/// True only for the git folder ADE created inside this project's state.
/// Historical adopted records with no such folder continue to load unchanged.
pub(crate) fn managed_git_folder(project: &Project, record: &Thread) -> bool {
    if !record.repo.is_empty() || record.worktree_path.is_empty() {
        return false;
    }
    let expected = [
        thread::threads_dir(project).join(&record.id),
        project.dir().join("threads").join(&record.id),
    ];
    let actual = Path::new(&record.worktree_path);
    let owned = expected.iter().any(|expected| {
        expected == actual
            || match (
                std::fs::canonicalize(expected),
                std::fs::canonicalize(actual),
            ) {
                (Ok(expected), Ok(actual)) => expected == actual,
                _ => false,
            }
    });
    owned && (actual.join(".git").is_dir() || !actual.exists())
}

fn removable_folder(project: &Project, record: &Thread) -> bool {
    !record.worktree_path.is_empty()
        && (record.kind == Kind::Worktree || managed_git_folder(project, record))
}

fn remove_ade_worktree(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if record.worktree_path.is_empty() {
        bail!("{} has no recorded worktree", record.id);
    }
    if let Err(error) = crate::git::worktree_remove(ctx.runner, &record.repo, &record.worktree_path)
    {
        if !worktree_exists(ctx, project, record)? {
            return Ok(());
        }
        let _ = thread::update(project, &record.id, |t| {
            t.partial = Some("worktree_remove".into());
        });
        return Err(error);
    }
    Ok(())
}

/// The D4 in-use gate before the lane's own idle agent is stopped. Durable
/// completion is checked separately by `finished_worktree_reason`; this gate
/// only prevents closing active work or removing a checkout used elsewhere.
fn removal_in_use_gate(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    let view = require_session(ctx, project)?;
    let (agents, panes) = lists_for(&view, record)?;
    let live = thread::live_state(record, &agents, &panes, jiff::Timestamp::now());
    if record.status != Status::Resolved && live.agent_state.as_deref() == Some("working") {
        bail!(
            "worktree_in_use: {} is working; not removing the worktree",
            record.id
        );
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

    let herdr = view.herdr.on_machine(record.machine_route());
    let own_agent = agents
        .iter()
        .any(|agent| thread::agent_matches(record, agent));
    for pane in panes
        .iter()
        .filter(|pane| Path::new(&pane.cwd).starts_with(&record.worktree_path))
    {
        // The lane's own idle agent is about to be stopped. Any other
        // non-shell process still makes removing the checkout unsafe.
        if own_agent && thread::pane_matches(record, pane) {
            continue;
        }
        let busy = herdr
            .pane_process_info(&pane.pane_id)
            .map_err(|error| anyhow::anyhow!("pane {}: {error}", pane.pane_id))?
            .foreground_processes
            .iter()
            .any(|process| {
                !matches!(
                    process.name.as_str(),
                    "zsh" | "-zsh" | "bash" | "sh" | "fish"
                )
            });
        if busy {
            bail!(
                "worktree_in_use: a program runs in {} (pane {}); not removing",
                record.worktree_path,
                pane.pane_id
            );
        }
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
#[derive(Clone)]
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
            note: if t.cleanup_pending {
                "cleanup pending".into()
            } else {
                t.resolved_reason.clone()
            },
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
        // Remote state is what the ticker last observed; the CLI makes no SSH
        // call and never turns an old state into an untimed current claim.
        let source = if t.observation_source.is_empty() {
            "courier"
        } else {
            &t.observation_source
        };
        let note = if t.last_observed.is_empty() {
            if !t.last_state.is_empty() {
                let next = if t.observation_error.is_empty() {
                    "next check pending".to_string()
                } else {
                    format!(
                        "latest check failed at {} by {source}: {}",
                        t.observation_attempted, t.observation_error
                    )
                };
                format!(
                    "{}; last checked time and source unknown; {next}, on {}",
                    t.last_state, t.machine
                )
            } else if t.observation_error.is_empty() {
                format!("first check pending by {source}, on {}", t.machine)
            } else {
                format!(
                    "first check failed at {} by {source}: {}, on {}",
                    t.observation_attempted, t.observation_error, t.machine
                )
            }
        } else {
            let state = if t.last_state.is_empty() {
                "no agent"
            } else {
                &t.last_state
            };
            let next = if t.observation_error.is_empty() {
                "next check pending".to_string()
            } else {
                format!(
                    "latest check failed at {}: {}",
                    t.observation_attempted, t.observation_error
                )
            };
            format!(
                "{state}; last checked {} by {source}; {next}, on {}",
                t.last_observed, t.machine
            )
        };
        return Row {
            thread: t.clone(),
            group: recorded,
            note: if t.startup_wait_started.is_empty() {
                note
            } else {
                format!("starting (checking agent readiness), on {}", t.machine)
            },
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
        format!("{}: {}", t.failure_class.plain(), t.error)
    } else if !t.startup_wait_started.is_empty() {
        "starting (checking agent readiness)".to_string()
    } else if !live.pane_exists {
        "process gone: pane or agent is gone without a report".to_string()
    } else {
        live.agent_state
            .unwrap_or_else(|| "agent state unknown; pane still exists".into())
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

fn placement_summary(record: &Thread) -> String {
    let machine = if record.machine.is_empty() {
        "local"
    } else {
        &record.machine
    };
    let reason = if record.placement_reason.is_empty() {
        "not recorded"
    } else {
        &record.placement_reason
    };
    format!("runs_on = {machine:?}\nplacement = {reason:?}\n")
}

pub fn print_show(ctx: &Ctx, slug: &str, id: &str) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    let view = session_view(ctx, &project);
    let row = row(&record, view.as_ref(), jiff::Timestamp::now());
    println!("group = {:?}", row.group.label());
    println!("live = {:?}", row.note);
    print!("{}", placement_summary(&record));
    print!("{}", toml::to_string(&record)?);
    if let Some(attestation) = done_attestation(&project, &record) {
        println!(
            "attested = {:?}",
            format!("{}: {}", attestation.coordinator, attestation.reason)
        );
    }
    if let Some(report) = thread::sealed_report_path(&project, &record) {
        println!("# final report: {}", report.display());
    } else if let Some(report) = thread::final_report_path(&project, &record) {
        println!("# historical report (not completion): {}", report.display());
    } else {
        let draft = Path::new(&record.thread_dir).join("report.md");
        if std::fs::symlink_metadata(&draft).is_ok_and(|metadata| metadata.is_file()) {
            println!(
                "# unsealed report draft (not completion): {}",
                draft.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn attest_refuses_every_unproven_or_ineligible_report() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let ctx = world.ctx();
        let make = |status, cancellation: &str, report_hash: &str| {
            thread::allocate(&project, |thread| {
                thread.status = status;
                thread.attempt = 1;
                thread.cancellation_reason = cancellation.into();
                thread.report_hash = report_hash.into();
            })
            .unwrap()
        };

        let open = make(Status::Open, "", "hash");
        assert!(
            attest(&ctx, "demo", &open.id, "checked")
                .unwrap_err()
                .to_string()
                .contains("attest_not_resolved")
        );

        let cancelled = make(Status::Resolved, "stopped", "hash");
        assert!(
            attest(&ctx, "demo", &cancelled.id, "checked")
                .unwrap_err()
                .to_string()
                .contains("attest_cancelled")
        );

        let completed = make(Status::Resolved, "", "hash");
        let event = crate::contracts::Event {
            id: format!("{}-1-1", completed.id),
            op: format!("{}-1-1", completed.id),
            thread: completed.id.clone(),
            attempt: 1,
            round: None,
            recipient: crate::contracts::Recipient::default(),
            created: project::now(),
            payload: crate::contracts::EventPayload {
                done: Some(crate::contracts::DonePayload {
                    sha: String::new(),
                    report_path: "stored".into(),
                    artifact: "hash".into(),
                    attestation: None,
                }),
                ..crate::contracts::EventPayload::default()
            },
        };
        crate::events::seal_create_if_absent(&project, &event).unwrap();
        assert!(
            attest(&ctx, "demo", &completed.id, "checked")
                .unwrap_err()
                .to_string()
                .contains("attest_already_done")
        );

        let missing = make(Status::Resolved, "", "hash");
        assert!(
            attest(&ctx, "demo", &missing.id, "checked")
                .unwrap_err()
                .to_string()
                .contains("attest_report_missing")
        );

        let mismatch = make(Status::Resolved, "", "not-the-report-hash");
        std::fs::write(thread::home_report_path(&project, &mismatch.id), b"report").unwrap();
        assert!(
            attest(&ctx, "demo", &mismatch.id, "checked")
                .unwrap_err()
                .to_string()
                .contains("attest_report_mismatch")
        );
    }

    #[test]
    fn attest_uses_the_lane_folders_head_when_it_still_exists() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let folder = world.home.path().join("lane-folder");
        std::fs::create_dir_all(folder.join(".git")).unwrap();
        let report = b"finished report\n";
        let hash = thread::sha256_hex(report);
        let lane = thread::allocate(&project, |thread| {
            thread.status = Status::Resolved;
            thread.attempt = 1;
            thread.worktree_path = folder.to_string_lossy().into_owned();
            thread.report_hash = hash;
        })
        .unwrap();
        std::fs::write(thread::home_report_path(&project, &lane.id), report).unwrap();
        world
            .runner
            .on("rev-parse HEAD", crate::runner::fake::ok("abc\n"));

        let outcome = attest(
            &world.ctx(),
            "demo",
            &lane.id,
            "The stored result was checked.",
        )
        .unwrap();
        assert_eq!(outcome.sha.as_deref(), Some("abc"));
        let event = crate::events::load(&project, &outcome.event).unwrap();
        assert_eq!(event.payload.done.unwrap().sha, "abc");
    }

    #[test]
    fn resolve_outcome_types_automatic_worktree_cleanup() {
        let outcome = ResolveOutcome {
            thread: "t-0001".into(),
            state: "resolved".into(),
            final_copy: "complete".into(),
            copy_notes: Vec::new(),
            pane: "closed".into(),
            worktree: "kept".into(),
            worktree_path: "/wt".into(),
            worktree_reason: Some("work_not_done".into()),
            branch: "lane".into(),
        };
        let data = serde_json::to_value(&outcome).unwrap();
        assert_eq!(data["worktree"], "kept");
        assert_eq!(data["worktree_reason"], "work_not_done");
        assert_eq!(
            outcome.message("demo"),
            "t-0001 resolved.\nIts pane and tab were closed.\nThe worktree /wt was kept: work_not_done.\n"
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
    fn a_follow_up_stays_queued_until_the_matching_bootstrap_receipt() {
        let mut record = worktree_thread();
        record.status = Status::Open;
        record.prompt_pending = false;
        record.launch.kind = "pi".into();
        record.bootstrap.clear();
        assert!(awaiting_bootstrap(&record));

        record.bootstrap = "acknowledged".into();
        assert!(!awaiting_bootstrap(&record));

        record.kind = Kind::Adopted;
        record.bootstrap.clear();
        assert!(!awaiting_bootstrap(&record));
    }

    #[test]
    fn prompt_refusals_and_sending_while_working() {
        let t = Thread {
            agent_name: String::new(),
            kind: Kind::Adopted,
            ..worktree_thread()
        };
        assert!(
            prompt_state(&t, &[], false)
                .unwrap_err()
                .to_string()
                .contains("bare shell prompt")
        );
        assert!(prompt_state(&t, &[agent("unknown")], false).is_err());
        assert!(
            prompt_state(&t, &[agent("blocked")], false)
                .unwrap_err()
                .to_string()
                .contains("agent_blocked")
        );
        assert_eq!(
            prompt_state(&t, &[agent("working")], false).unwrap(),
            "working"
        );
        assert_eq!(prompt_state(&t, &[agent("idle")], false).unwrap(), "idle");
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
    fn birth_sentence_keeps_structure_and_exact_technical_details() {
        let err = check_birth_plain("").unwrap_err().to_string();
        assert!(err.contains("plain_missing"), "{err}");
        check_birth_plain("README, docs and skill files change src/plain.rs for t-0284.").unwrap();
        check_birth_plain("The lane does the work.").unwrap();
        let long = format!("{}.", vec!["README"; 26].join(" "));
        check_birth_plain(&long).unwrap();
    }

    struct GitReal<'a> {
        fake: &'a crate::runner::fake::FakeRunner,
    }

    impl crate::runner::Runner for GitReal<'_> {
        fn run(&self, cmd: &crate::runner::Cmd) -> anyhow::Result<crate::runner::Output> {
            if matches!(cmd.program.as_str(), "git" | "du" | "rsync") {
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
    fn ade_start_with_a_job_keeps_the_lead_brief_across_retry() {
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
            "[routing]\ndefault = \"test_claude\"\nretries = 1\nfallback = []\n\n[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n",
        )
        .unwrap();

        crate::talk::append(
            &project,
            None,
            crate::talk::Entry::Rolf {
                request: "q-brief".into(),
                text: "Keep the complete lead brief with the task.".into(),
                answer: None,
            },
        )
        .unwrap();
        let stable_task = crate::task::add(
            &project,
            "Fix the saved login.",
            vec!["request:q-brief".into()],
            vec!["The saved login works after a restart.".into()],
            Some(repo_s.clone()),
            None,
        )
        .unwrap();
        let lead_brief = "FULL LEAD BRIEF: preserve this exact repair instruction.\n\n## Required detail\n\nKeep `--exact` intact.\n";
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
                // The CLI maps `--task-file` to this verbatim field.
                task: lead_brief.into(),
                plain: "The lane does the work.".into(),
                workflow: None,
                recipe: None,
                recipe_basis: None,
                // The CLI maps `--job` to this stable task id.
                task_id: stable_task.id.clone(),
                review_round: String::new(),
            },
        )
        .unwrap();
        assert_eq!(started.role, "lane");
        assert!(!started.launch.kind.is_empty());
        assert!(!started.launch.brief_hash.is_empty());
        assert_eq!(started.attempt, 1);
        let wt = Path::new(&repo_s).join(".worktrees").join(&started.id);
        assert!(wt.is_dir(), "git worktree should exist");
        // The lane branches from the exact integration commit while its brief
        // is a content-addressed project artifact and ignored runtime file.
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
        assert_eq!(git_out(&["rev-parse", "main"]), started.base);
        assert!(git_out(&["ls-tree", "-r", "--name-only", "main", "--", "tasks"]).is_empty());
        let artifact = crate::thread::artifact(&project, &started.launch.brief_hash).unwrap();
        let brief = String::from_utf8_lossy(&artifact);
        assert!(brief.starts_with("plain: The lane does the work."));
        assert!(
            brief.contains(&format!("# Task\n\n## Lead brief\n\n{lead_brief}")),
            "{brief}"
        );
        assert!(
            brief.contains(&format!("## {} — Fix the saved login.", stable_task.id)),
            "{brief}"
        );
        assert_eq!(
            std::fs::read(Path::new(&started.thread_dir).join("brief.md")).unwrap(),
            artifact
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

        // The ready lane is primed once with its role skill and frozen
        // runtime brief.
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
            .map(|c| c.args.get(3).cloned().unwrap_or_default())
            .collect();
        assert_eq!(prompts.len(), 1, "{prompts:?}");
        assert!(
            prompts[0].ends_with(&format!(
                " skill lane`, then read .herdr-project/demo-{}/brief.md and do what it says.",
                started.id
            )),
            "{}",
            prompts[0]
        );
        drop(calls);
        *world.agents.borrow_mut() = "[]".into();

        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        thread::update(&project, &started.id, |thread| {
            thread.failure_class = crate::contracts::FailureClass::ProcessGone;
        })
        .unwrap();
        retry(&ctx, "demo", &started.id, "the first process disappeared").unwrap();
        let retried = thread::load(&project, &started.id).unwrap();
        assert_eq!(retried.attempt, 2);
        assert_eq!(retried.launch.kind, kind);
        assert_eq!(retried.launch.attempt, 2);
        assert_eq!(retried.launch.escalations, 0);
        assert_eq!(retried.launch.same_recipe_retries, 1);
        assert_eq!(retried.launch.brief_hash, started.launch.brief_hash);
        assert!(retried.placement_reason.contains("retry on `local`"));
        assert!(retried.placement_reason.contains(&retried.launch.recipe_id));
        let retried_brief = crate::thread::artifact(&project, &retried.launch.brief_hash).unwrap();
        assert!(String::from_utf8_lossy(&retried_brief).contains(lead_brief));
        let calls = world.runner.calls.borrow();
        assert!(!calls.iter().any(|c| c.display().contains("worktree open")));
        let tabs = calls
            .iter()
            .filter(|c| c.display().contains("tab create"))
            .count();
        assert!(tabs >= 2, "retry should open a tab, not a workspace");
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
                "[routing]\ndefault = \"test_claude\"\nretries = 1\nfallback = []\n\n[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n\n[harness]\nrepos = [{{ path = \"{harness_s}\" }}]\n"
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
            recipe: None,
            recipe_basis: None,
            task_id: String::new(),
            review_round: String::new(),
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
                recipe: None,
                recipe_basis: None,
                task_id: String::new(),
                review_round: String::new(),
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
        let inbox = std::fs::read_dir(crate::inbox::inbox_dir(&project))
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

    const SHIPPED_MACHINE_CONFIG: &str = "[routing]\ndefault = \"test_claude\"\nretries = 1\nfallback = []\n\n[[routing.rules]]\nproduct = \"web-research\"\nrecipe = \"agy_gemini_flash\"\n\n[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n\n[dispatch]\nmachine = \"oci\"\n";

    const NATIVE_BOX: &str = "\n[machines.oci]\nlabel = \"oci\"\ntarget = \"oci-pi\"\nsession = \"default\"\nhome = \"/home/ubuntu\"\nroot = \"/home/ubuntu/.herdr-ade\"\nworktrees = \"/home/ubuntu/projects\"\nbuild = \"/home/ubuntu/build/lanes\"\npath = \"/home/ubuntu/.local/bin:/home/ubuntu/.cargo/bin:/usr/local/bin:/usr/bin:/bin\"\nade_bin = \"/home/ubuntu/.local/bin/herdr-ade\"\npi_bin = \"/home/ubuntu/.local/bin/herdr-pi\"\nkinds = [\"pi\", \"claude\", \"agy\"]\n";

    fn lane_config() -> String {
        format!("{SHIPPED_MACHINE_CONFIG}{NATIVE_BOX}")
    }

    fn start_args(repo: Option<String>, machine: Option<String>) -> StartArgs {
        StartArgs {
            title: "Fix login".into(),
            repo,
            machine,
            base: None,
            task: "Do the thing.".into(),
            plain: "The lane does the work.".into(),
            workflow: None,
            recipe: None,
            recipe_basis: None,
            task_id: String::new(),
            review_round: String::new(),
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
        assert_eq!(
            default_machine("lane", "oci", Some("/r")),
            Some("oci".into())
        );
        assert_eq!(
            default_machine("reviewer", "oci", Some("/r")),
            Some("oci".into())
        );
        assert_eq!(default_machine("research", "oci", Some("/r")), None);
        assert_eq!(default_machine("lane", "", Some("/r")), None);
        assert_eq!(default_machine("lane", "oci", None), None);
    }

    #[test]
    fn box_lanes_share_one_project_workspace_and_take_separate_tabs() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, &lane_config());
        stub_box(&fx);
        let started = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        assert_eq!(started.machine, "oci");
        assert_eq!(started.machine_id, "oci-id");
        assert_eq!(started.launch.machine, "oci");
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
    fn claude_and_agy_picks_use_the_mac_without_box_readiness_checks() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, SHIPPED_MACHINE_CONFIG);
        stub_box(&fx);
        let repo = Some(fx.repo.to_string_lossy().into_owned());
        let claude = start(&fx.world.ctx(), "demo", start_args(repo.clone(), None)).unwrap();
        assert_eq!(claude.launch.recipe_id, "test_claude");
        assert!(claude.machine.is_empty());
        assert_eq!(claude.launch.machine, "local");

        let mut args = start_args(repo, None);
        args.task = "+++\nproduct = \"web-research\"\n+++\nCompare the published results.".into();
        let agy = start(&fx.world.ctx(), "demo", args).unwrap();
        assert_eq!(agy.launch.recipe_id, "agy_gemini_flash");
        assert!(agy.machine.is_empty());
        assert_eq!(agy.launch.machine, "local");
        assert!(agy.launch.args.iter().any(|arg| arg == "--new-project"));

        let ledger =
            std::fs::read_to_string(fx.project.state_dir().join("dispatch.jsonl")).unwrap();
        assert!(
            ledger.contains("does not run adapter kind `claude`"),
            "{ledger}"
        );
        assert!(
            ledger.contains("does not run adapter kind `agy`"),
            "{ledger}"
        );
        assert!(
            fx.world
                .runner
                .calls
                .borrow()
                .iter()
                .all(|call| call.program != "ssh"),
            "an unsupported kind must not probe the box"
        );
    }

    #[test]
    fn pi_lanes_and_reviewers_still_use_a_ready_box() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, SHIPPED_MACHINE_CONFIG);
        let task = "Run the bounded coding task.";
        let config_path = fx.world.home.path().join("cfg/config.toml");
        let config = std::fs::read_to_string(&config_path).unwrap();
        let hash = crate::thread::sha256_hex(task.as_bytes());
        std::fs::write(
            &config_path,
            format!("{config}\n[routing.pins]\n\"{hash}\" = \"pi_opencode_deepseek\"\n"),
        )
        .unwrap();
        stub_box(&fx);
        for role in [None, Some("reviewer")] {
            let mut args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
            args.task = task.into();
            args.workflow = role.map(str::to_string);
            let started = start(&fx.world.ctx(), "demo", args).unwrap();
            assert_eq!(started.launch.recipe_id, "pi_opencode_deepseek");
            assert_eq!(started.machine, "oci");
            assert_eq!(started.machine_id, "oci-id");
        }
    }

    #[test]
    fn an_explicit_machine_that_excludes_the_pick_is_refused_without_ssh() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, SHIPPED_MACHINE_CONFIG);
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
        assert!(error.contains("does not run adapter kind `agy`"), "{error}");
        assert!(thread::list(&fx.project).is_empty());
        let ledger =
            std::fs::read_to_string(fx.project.state_dir().join("dispatch.jsonl")).unwrap();
        assert!(ledger.contains("placement-refused"), "{ledger}");
        assert!(
            fx.world
                .runner
                .calls
                .borrow()
                .iter()
                .all(|call| call.program != "ssh"),
            "an explicitly unsupported kind must not probe the box"
        );
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
        let config_dir = home.path().join("cfg");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), lane_config()).unwrap();
        let ctx = Ctx {
            env: &env,
            root: home.path().join("root"),
            config_dir,
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
            publish_url: Some("https://example/repo.git".into()),
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
        let e = box_repo_row(Path::new(""), &settings, "oci", "/r")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("box_publish_url_missing") && e.contains("publish_url"),
            "{e}"
        );

        settings.repos[0].box_path = None;
        settings.repos[0].publish_url = Some("https://example/r.git".into());
        let e = box_repo_row(Path::new(""), &settings, "oci", "/r")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("box_path_missing") && e.contains("box_path"),
            "{e}"
        );

        // A repo with neither, and not in the built-in list, keeps the
        // mapping message; a built-in harness repo resolves from the map.
        settings.repos[0].publish_url = None;
        let e = box_repo_row(Path::new(""), &settings, "oci", "/other")
            .unwrap_err()
            .to_string();
        assert!(e.contains("box_repo_unmapped"), "{e}");
        let (box_path, url) = box_repo_row(
            Path::new(""),
            &settings,
            "oci",
            "/Users/rolfie/projects/herdr",
        )
        .unwrap();
        assert_eq!(box_path, "/home/ubuntu/projects/herdr");
        assert!(url.ends_with("herdr.git"), "{url}");
    }

    #[test]
    fn a_default_start_with_no_box_publish_url_falls_back_to_this_mac() {
        let (fx, _remote) = box_fixture();
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos[0].publish_url = None;
        let text = format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap());
        std::fs::write(fx.project.project_md(), text).unwrap();
        write_config(&fx, &lane_config());
        stub_box(&fx);
        let started = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        assert!(started.machine.is_empty());
        assert_eq!(started.launch.machine, "local");
        assert!(started.placement_reason.contains("box_publish_url_missing"));
        let summary = placement_summary(&thread::load(&fx.project, &started.id).unwrap());
        assert!(summary.contains("runs_on = \"local\""), "{summary}");
        assert!(summary.contains("box_publish_url_missing"), "{summary}");
        let mut older = started.clone();
        older.placement_reason.clear();
        assert!(placement_summary(&older).contains("placement = \"not recorded\""));
        assert!(
            started
                .worktree_path
                .starts_with(&fx.repo.to_string_lossy().to_string())
        );
        assert_eq!(
            say_lines(&fx.project),
            vec![
                "the box has no publishing address for this repository, so this lane runs here"
                    .to_string()
            ]
        );
        let ledger =
            std::fs::read_to_string(fx.project.state_dir().join("dispatch.jsonl")).unwrap();
        assert!(ledger.contains("box_publish_url_missing"), "{ledger}");

        let error = start(
            &fx.world.ctx(),
            "demo",
            start_args(
                Some(fx.repo.to_string_lossy().into_owned()),
                Some("oci".into()),
            ),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("box_publish_url_missing"), "{error}");
        assert_eq!(thread::list(&fx.project).len(), 1);
    }

    #[test]
    fn machine_local_keeps_a_box_repo_lane_on_this_mac() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, &lane_config());
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
        assert_eq!(started.launch.machine, "local");
        assert!(
            started
                .worktree_path
                .starts_with(&fx.repo.to_string_lossy().to_string())
        );
    }

    #[test]
    fn cancel_and_resolve_record_an_already_gone_local_worktree_as_removed() {
        use crate::round::testkit::{fixture, git};

        let fx = fixture();
        let repo = fx.repo.to_string_lossy().into_owned();
        let ctx = fx.world.ctx();
        let prepare_gone = |number| {
            let (id, _) = fx.lane(number);
            let record = thread::update(&fx.project, &id, |t| {
                t.repo = repo.clone();
                t.thread_dir = t.worktree_path.clone();
            })
            .unwrap();
            std::fs::remove_dir_all(&record.worktree_path).unwrap();
            (id, record.worktree_path)
        };

        let (cancelled_id, cancelled_path) = prepare_gone(1);
        let cancelled =
            cancel(&ctx, "demo", &cancelled_id, "the work is no longer needed").unwrap();
        assert_eq!(cancelled.worktree, "removed");
        assert_eq!(cancelled.worktree_reason, None);
        assert!(
            thread::load(&fx.project, &cancelled_id)
                .unwrap()
                .worktree_path
                .is_empty()
        );

        let (resolved_id, resolved_path) = prepare_gone(2);
        let resolved = resolve(&ctx, "demo", &resolved_id, &ResolveArgs::default()).unwrap();
        assert_eq!(resolved.worktree, "removed");
        assert_eq!(resolved.worktree_path, resolved_path);
        assert_eq!(resolved.worktree_reason, None);
        assert!(
            thread::load(&fx.project, &resolved_id)
                .unwrap()
                .worktree_path
                .is_empty()
        );

        let registered = git(&fx.repo, &["worktree", "list", "--porcelain"]);
        assert!(!registered.contains(&cancelled_path), "{registered}");
        assert!(!registered.contains(&resolved_path), "{registered}");
    }

    #[test]
    fn correction_barrier_reaches_box_before_prompt_delivery() {
        use crate::contracts::{RoundPhase, RoundRecord};
        use crate::runner::fake::ok;

        let fx = crate::round::testkit::fixture();
        let record = thread::allocate(&fx.project, |t| {
            t.machine = "box".into();
            t.machine_id = "box-id".into();
        })
        .unwrap();
        let round = RoundRecord {
            round: "r54".into(),
            phase: RoundPhase::UnderReview,
            reviewer: Some(record.id.clone()),
            reviewer_awaiting_report_after: Some(format!("{}-1-1", record.id)),
            ..RoundRecord::default()
        };
        std::fs::create_dir_all(crate::round::rounds_dir(&fx.project)).unwrap();
        std::fs::write(
            crate::round::round_path(&fx.project, "r54"),
            toml::to_string(&round).unwrap(),
        )
        .unwrap();
        fx.world.runner.on(
            "machine list --json",
            ok(r#"[{"id":"box-id","label":"box","target":"box","session":"default","enabled":true}]"#),
        );
        fx.world
            .runner
            .on_fn(|cmd| cmd.program == "ssh", |_| Ok(ok("")));
        sync_box_corrections(&fx.world.ctx(), &fx.project, &record).unwrap();
        let calls = fx.world.runner.calls.borrow();
        let sent = calls.iter().find(|cmd| cmd.program == "ssh").unwrap();
        assert!(sent.args.last().unwrap().contains(".state/corrections"));
        assert!(
            sent.stdin
                .as_ref()
                .unwrap()
                .contains(&format!("{}-1-1", record.id))
        );
    }

    #[test]
    fn a_gone_box_worktree_is_removed_only_after_the_box_prunes_it() {
        use crate::runner::fake::ok;

        let fx = crate::round::testkit::fixture();
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos[0].box_path = Some("/box/repo".into());
        settings.repos[0].publish_url = Some("https://example.invalid/repo.git".into());
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        fx.world.runner.on(
            "machine list --json",
            ok(r#"[{"id":"box-id","label":"box","target":"box","session":"default","enabled":true}]"#),
        );
        fx.world.runner.on_fn(
            |cmd| {
                cmd.program == "ssh"
                    && cmd
                        .args
                        .last()
                        .is_some_and(|script| script.contains("__HERDR_WORKTREE_REMOVED__"))
            },
            |_| Ok(ok("__HERDR_WORKTREE_REMOVED__\n")),
        );
        fx.world
            .runner
            .on_fn(|cmd| cmd.program == "ssh", |_| Ok(ok("")));

        let remote_path = "/box/repo/.worktrees/t-0001";
        let record = thread::allocate(&fx.project, |t| {
            t.kind = Kind::Worktree;
            t.status = Status::Open;
            t.repo = fx.repo.to_string_lossy().into_owned();
            t.machine = "box".into();
            t.machine_id = "box-id".into();
            t.worktree_path = remote_path.into();
            t.thread_dir = format!("{remote_path}/.herdr-project/demo-t-0001");
            t.branch = "lane/box".into();
        })
        .unwrap();

        let outcome =
            resolve(&fx.world.ctx(), "demo", &record.id, &ResolveArgs::default()).unwrap();
        assert_eq!(outcome.worktree, "removed");
        assert_eq!(outcome.worktree_path, remote_path);
        assert!(
            thread::load(&fx.project, &record.id)
                .unwrap()
                .worktree_path
                .is_empty()
        );
        let calls = fx.world.runner.calls.borrow();
        assert!(
            calls
                .iter()
                .any(|call| call.display().contains("__HERDR_WORKTREE_REMOVED__"))
        );
        let cleanup = calls
            .iter()
            .find(|call| call.display().contains("git worktree prune --expire=now"))
            .expect("box cleanup");
        assert!(!cleanup.display().contains("rm -rf --"));
        assert!(calls.iter().any(|call| {
            call.display()
                .contains("rm -rf -- /home/ubuntu/build/lanes/demo-t-0001")
        }));
    }

    #[test]
    fn omitted_repo_uses_only_listed_repo_and_refuses_ambiguity() {
        let (fx, _) = box_fixture();
        write_config(&fx, &lane_config());
        stub_box(&fx);
        let repo = fx.repo.to_string_lossy().into_owned();
        let started = start(&fx.world.ctx(), "demo", start_args(None, None)).unwrap();
        assert_eq!(started.repo, repo);
        assert_eq!(started.kind, Kind::Worktree);
        assert_ne!(started.cwd, fx.project.dir().to_string_lossy());
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos.push(crate::project::Repo {
            path: "/another/repo".into(),
            ..Default::default()
        });
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        let error = start(&fx.world.ctx(), "demo", start_args(None, None))
            .unwrap_err()
            .to_string();
        assert!(error.contains("repo_ambiguous"), "{error}");
    }

    #[test]
    fn retry_refuses_a_starting_or_working_agent_with_its_visible_screen() {
        use crate::runner::fake::{FakeRunner, ok};
        for (state, starting) in [("blocked", true), ("working", false)] {
            let runner = FakeRunner::new();
            runner.on("agent list", ok(&format!(r#"{{"result":{{"agents":[{{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","cwd":"/repo","name":"hp-demo-t-0001","agent_status":"{state}"}}]}}}}"#)));
            runner.on("pane read", ok("Trust this folder?\n"));
            let herdr = Herdr::new("herdr", "", &runner);
            let record = Thread {
                id: "t-0001".into(),
                pane_id: "w1:p2".into(),
                tab_id: "w1:t2".into(),
                workspace_id: "w1".into(),
                cwd: "/repo".into(),
                agent_name: "hp-demo-t-0001".into(),
                status: Status::Open,
                startup_wait_started: if starting {
                    project::now()
                } else {
                    String::new()
                },
                launch: crate::contracts::Launch {
                    ready_timeout_ms: 300_000,
                    ..Default::default()
                },
                ..Thread::default()
            };
            let error = refuse_busy_retry(&herdr, &record).unwrap_err().to_string();
            assert!(
                error.contains(if starting {
                    "still starting"
                } else {
                    "still working"
                }),
                "{error}"
            );
            assert!(error.contains("Trust this folder?"), "{error}");
            assert_eq!(runner.count("pane read"), 1);
        }
    }

    #[test]
    fn expired_startup_retry_reports_the_same_screen() {
        use crate::runner::fake::{FakeRunner, ok};
        let runner = FakeRunner::new();
        runner.on("pane read", ok("Trust this folder?\n  1. Yes\n"));
        let herdr = Herdr::new("herdr", "", &runner);
        let record = Thread {
            id: "t-0001".into(),
            pane_id: "w1:p2".into(),
            error: "agent_not_ready: screen: Trust this folder? | 1. Yes; herdr: blocked".into(),
            ..Thread::default()
        };
        let screen = same_startup_screen(&herdr, &record).unwrap().unwrap();
        assert!(screen.contains("Trust this folder?"), "{screen}");
        assert_eq!(runner.count("pane read"), 1);

        let changed = FakeRunner::new();
        changed.on("pane read", ok("The helper is now ready\n"));
        let herdr = Herdr::new("herdr", "", &changed);
        let error = same_startup_screen(&herdr, &record)
            .unwrap_err()
            .to_string();
        assert!(error.contains("startup_screen_changed"), "{error}");
    }

    #[test]
    fn a_held_box_falls_back_to_this_mac_with_one_line() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, &lane_config());
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
        write_config(&fx, &lane_config());
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
        write_config(&fx, &lane_config());
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
        use crate::round::testkit::git;

        let (fx, remote) = box_fixture();
        write_config(
            &fx,
            &format!(
                "[routing]\ndefault = \"test_claude\"\nretries = 1\nfallback = []\n\n[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the careful checker\"\n[dispatch]\nmachine = \"oci\"\n{NATIVE_BOX}"
            ),
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
        assert!(skill.contains("`ha done` publishes C on your own reviewer branch"));
        let brief = String::from_utf8(
            thread::artifact(&fx.project, record.review_artifact.as_deref().unwrap()).unwrap(),
        )
        .unwrap();
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
        let report = format!(".herdr-project/demo-{}/report.md", started.id);
        std::fs::write(box_repo.join(".git/info/exclude"), ".herdr-project/\n").unwrap();
        std::fs::create_dir_all(box_repo.join(&report).parent().unwrap()).unwrap();
        std::fs::write(
            box_repo.join(&report),
            format!(
                "+++\nverdict = \"MERGE\"\nround = \"r1\"\ncandidate = \"{candidate}\"\nmanifest_hash = \"{}\"\npolicy_hash = \"{}\"\ngates = []\n+++\n\nAll lanes checked.\n",
                record.manifest_hash.as_deref().unwrap(),
                record.policy_hash,
            ),
        )
        .unwrap();
        assert_ne!(started.base, candidate);
        let error =
            ops::check_published_ref(ctx.runner, &box_repo, &started.branch, &remote, &candidate)
                .unwrap_err()
                .to_string();
        assert!(error.starts_with("published_ref_mismatch:"), "{error}");
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
                    sha: candidate.clone(),
                    report_path: report.clone(),
                },
                helper_pid: std::process::id(),
            },
        )
        .unwrap();
        ops::stage_box_done(
            &box_project,
            &op.op,
            &box_repo,
            ctx.runner,
            &crate::contracts::LaneCard {
                thread: started.id.clone(),
                attempt: started.attempt,
                branch: started.branch.clone(),
                publish_url: remote.clone(),
                recipient: op.recipient.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        ops::check_published_ref(ctx.runner, &box_repo, &started.branch, &remote, &candidate)
            .unwrap();
        assert_eq!(git(&fx.repo, &["rev-parse", &started.branch]), started.base);
        assert_eq!(
            git(&fx.repo, &["ls-remote", &remote, "refs/heads/main"]),
            ""
        );
        let event = ops::seal(&box_project, &op.op, |_| Ok(())).unwrap();
        assert_eq!(event.payload.done.unwrap().sha, candidate);
        assert_eq!(crate::events::list(&box_project).len(), 1);
    }
}
