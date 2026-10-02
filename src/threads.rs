//! The `thread` subcommands. Each is one deterministic mechanic; the
//! coordinator decides whether, what and where.

use std::collections::{BTreeMap, BTreeSet};
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

// External cleanup is serial and may require several SSH trips per lane.
// Keep both landing and its retry queue bounded, rather than draining a pile.
pub(crate) const CLEANUP_BATCH_SIZE: usize = 2;

type CleanupLists = Option<(Vec<Agent>, Vec<Pane>)>;

/// One cleanup slice's live lists. Reused across members and the in-use/close
/// checks; per-pane process checks still run at the point of removal.
#[derive(Default)]
pub(crate) struct CleanupViews {
    lists: BTreeMap<(String, String), CleanupLists>,
}

impl CleanupViews {
    fn for_thread<'a>(
        &mut self,
        ctx: &'a Ctx,
        project: &Project,
        record: &Thread,
    ) -> Result<Option<SessionView<'a>>> {
        let socket = project.coordinator().map(|c| c.socket).unwrap_or_default();
        let key = (socket.clone(), record.machine_route().to_string());
        if !self.lists.contains_key(&key) {
            let lists = cleanup_view(ctx, project, record)?.map(|view| (view.agents, view.panes));
            self.lists.insert(key.clone(), lists);
        }
        Ok(self.lists[&key].clone().map(|(agents, panes)| SessionView {
            herdr: Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner)
                .on_machine(record.machine_route()),
            agents,
            panes,
        }))
    }

    fn closed(&mut self, project: &Project, record: &Thread) {
        // Herdr ids are only unique within a session/machine.
        let key = (
            project.coordinator().map(|c| c.socket).unwrap_or_default(),
            record.machine_route().to_string(),
        );
        if let Some(Some((agents, panes))) = self.lists.get_mut(&key) {
            agents.retain(|a| !thread::agent_matches(record, a));
            panes.retain(|p| p.tab_id != record.tab_id || p.workspace_id != record.workspace_id);
        }
    }
}

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
    if out.timed_out {
        bail!(
            "git {}: timed out; repo activity at timeout: {}",
            args.join(" "),
            crate::git::repo_activity(repo)
        );
    }
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
    /// An exact recipe the coordinator chose for this one lane.
    pub recipe: Option<String>,
    pub task_id: String,
    /// Internal reviewer identity; empty for every non-reviewer start.
    pub review_id: String,
}

/// Internal birth description: required, with no vocabulary or length gate.
pub fn check_birth_plain(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        bail!("plain_missing");
    }
    Ok(())
}

/// Creates the worktree or tab, the thread directory and the brief, then
/// launches its agent and delivers its brief in the same call. The ticker
/// resumes any unfinished startup using the same launch and delivery paths.
pub fn start(ctx: &Ctx, slug: &str, args: StartArgs) -> Result<Thread> {
    start_with_ticker(ctx, slug, args, ticker::start, None, true)
}

/// Place the reviewer while `review` holds its lock. The ticker launches the
/// durable pending attempt on its next pass, outside the repository lock.
pub(crate) fn start_during_advance(ctx: &Ctx, slug: &str, args: StartArgs) -> Result<Thread> {
    start_with_ticker(ctx, slug, args, ticker::ensure, None, false)
}

fn start_with_ticker(
    ctx: &Ctx,
    slug: &str,
    args: StartArgs,
    ensure_ticker: fn(&Ctx<'_>) -> Result<()>,
    source_truncation: Option<serde_json::Value>,
    launch_now: bool,
) -> Result<Thread> {
    let project = Project::load(&ctx.root, slug)?;
    if args.workflow.as_deref() == Some("reviewer") && args.review_id.is_empty() {
        bail!(
            "workflow_reserved: reviewer lanes are started by ha review. For an independent check use --workflow critic (verdict = \"PASS\"|\"FAIL\" front matter); for a specific recipe use --recipe <id>."
        );
    }
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
    crate::plan::check_prerequisites(&project, &args.task_id)?;
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
        return Err(crate::refusal::error(
            format!("workflow_unknown: `{role}` does not name a lane instruction set"),
            format!(
                "ha thread start {} --workflow lane --job <job> --task-file <file>",
                project.slug
            ),
        ));
    }
    // A box lane still commits and pushes from the Mac clone, so every
    // explicit repository is a local path and follows the same allowlist,
    // local and box lanes alike (SPEC-remote §4.2).
    let requested_repo = match args.repo.as_deref() {
        Some(repo) => repo,
        None => match settings.repos.as_slice() {
            [only] => &only.path,
            [] => {
                return Err(crate::refusal::error(
                    "repo_required: this project has no listed repository; pass --repo after listing one",
                    format!(
                        "ha thread start {} --repo <repository-path> --job <job> --task-file <file>",
                        project.slug
                    ),
                ));
            }
            _ => {
                return Err(crate::refusal::error(
                    format!(
                        "repo_ambiguous: this project lists several repositories; pass --repo: {}",
                        settings
                            .repos
                            .iter()
                            .map(|repo| repo.path.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    format!(
                        "ha thread start {} --repo {} --job <job> --task-file <file>",
                        project.slug, settings.repos[0].path
                    ),
                ));
            }
        },
    };
    let repo = std::fs::canonicalize(requested_repo)
        .with_context(|| format!("repository {requested_repo} does not exist"))?
        .to_string_lossy()
        .into_owned();
    if !crate::harness::allowed_repo(&settings, &ctx.config_dir, &repo)? {
        return Err(crate::refusal::error(
            format!(
                "repo_not_listed: {repo} is not listed in `repos` in PROJECT.md and is not a harness repository"
            ),
            format!(
                "ha thread start {} --repo <listed-repository-path> --job <job> --task-file <file>",
                project.slug
            ),
        ));
    }
    let listed = settings.repos.iter().find(|row| {
        std::fs::canonicalize(&row.path).is_ok_and(|path| path.to_string_lossy() == repo)
    });
    if let Some(recipe) = &args.recipe {
        crate::launch::validate_explicit_recipe(
            ctx,
            &project,
            &args.task_id,
            &args.task,
            role,
            recipe,
        )?;
    }
    let mut launch = crate::launch::resolve_launch(
        ctx,
        &project,
        &crate::launch::ResolveInput {
            task: &args.task,
            workflow: role,
            recipe: args.recipe.as_deref(),
            recipe_basis: None,
            recipe_request: None,
            source_truncation: source_truncation.as_ref(),
            ..Default::default()
        },
    )?;
    // The machine is resolved before any tab or worktree exists (SPEC-remote
    // §4.1, d-0005). `--machine` wins and never falls back; a default box
    // start whose box cannot be used falls back to this Mac.
    let explicit_machine = args.machine.as_deref().filter(|m| !m.is_empty());
    let (placement, provider_wait) = match resolve_placement(
        ctx,
        explicit_machine,
        role,
        &launch,
        Some(repo.as_str()),
        listed,
    ) {
        Ok(placement) => (placement, None),
        Err(error) if provider_readiness_error(&format!("{error:#}")) => {
            // Save an unplaced attempt on its requested machine. No pane or
            // worktree exists until a later readiness probe succeeds.
            let default = default_machine(role, &launch.machine, Some(&repo));
            let machine = explicit_machine
                .or(default.as_deref())
                .unwrap_or(crate::contracts::MACHINE_LOCAL);
            (
                Placement {
                    machine: if machine == crate::contracts::MACHINE_LOCAL {
                        String::new()
                    } else {
                        machine.to_string()
                    },
                    ..Placement::default()
                },
                Some(format!("{error:#}")),
            )
        }
        Err(error) => {
            if crate::remote::is_unreachable(&format!("{error:#}")) {
                return Err(error);
            }
            crate::launch::dispatch(
                &project,
                serde_json::json!({"kind":"placement-refused", "recipe":launch.recipe_id,
                    "explicit":explicit_machine, "error":format!("{error:#}")}),
            )?;
            return Err(error);
        }
    };
    crate::launch::dispatch(
        &project,
        serde_json::json!({"kind":"placement", "recipe":launch.recipe_id,
            "machine":placement.dispatch_machine(), "reason":placement.reason,
            "tried":placement.tried}),
    )?;
    let machine = placement.machine.clone();
    // Selection initially carries the configured dispatch candidate because
    // placement needs it. The durable lane launch names where this attempt was
    // actually placed, including an explicit or fallback local placement.
    launch.machine = placement.dispatch_machine().to_string();

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
        t.review_id = args.review_id.clone();
        t.plain = args.plain.trim().to_string();
        t.attempt = 1;
        t.launch = launch.clone();
        if let Some(reason) = &provider_wait {
            t.provider_wait_started = project::now();
            t.error = format!("waiting for provider: {reason}");
        }
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

    if provider_wait.is_some() {
        return thread::load(&project, &id);
    }

    match place_and_brief(ctx, &project, &view, &id, false) {
        Ok(thread) => {
            refresh_plan(ctx, &project);
            if launch_now {
                ticker::launch_thread_now(ctx, &project, &id)?;
            }
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
            if crate::remote::is_unreachable(&message) {
                return Err(error);
            }
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
            if crate::refusal::is(&error) {
                return Err(error);
            }
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
    fn dispatch_machine(&self) -> &str {
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
            crate::doctor::check_start_disk(ctx, None, repo)
                .and_then(|_| crate::doctor::recipe_ready_local(ctx, launch))
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
                        crate::doctor::check_start_disk(ctx, None, repo)
                            .and_then(|_| crate::doctor::recipe_ready_local(ctx, launch))
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
            Err(missing) => {
                if crate::remote::is_unreachable(&missing) {
                    bail!("{missing}");
                }
                if missing.contains("disk_low:") {
                    bail!("{missing}");
                }
                let provider_wait = missing.contains("pi_not_ready");
                tried.push(serde_json::json!({
                    "machine":candidate, "ready":false, "missing":missing
                }));
                // A transient provider failure does not move a pinned recipe
                // to a different machine merely because its probe was slow.
                if provider_wait {
                    break;
                }
            }
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

/// Do not mistake an unavailable machine or repository for a provider blip.
fn provider_readiness_error(error: &str) -> bool {
    error.contains("recipe_unavailable:")
        && !crate::remote::is_unreachable(error)
        && error.contains("pi_not_ready")
        && !error.contains("box_repo_")
        && !error.contains("machine_held:")
        && !error.contains("machine_kind_unavailable:")
}

/// Complete a provider-blocked placement through the ordinary startup path.
pub(crate) fn resume_provider_start(ctx: &Ctx, project: &Project, id: &str) -> Result<()> {
    crate::plan::check_attempt_prerequisites(project, id)?;
    let view = require_session(ctx, project)?;
    let waiting = thread::load(project, id)?;
    if waiting.is_remote() {
        // Placement was deferred before its profile could be recorded. The
        // box pane and lane card must use the saved id, not an empty route.
        let profile = remote::machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            waiting.machine_route(),
        )?;
        thread::update(project, id, |t| {
            t.machine = profile.label.clone();
            t.machine_id = profile.id.clone();
            t.launch.machine = profile.label.clone();
        })?;
    }
    place_and_brief(ctx, project, &view, id, false)?;
    Ok(())
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
/// repo with neither uses a configured harness or machine mapping, or reports
/// that no mapping exists.
fn box_repo_candidate(
    config_dir: &Path,
    machine: &str,
    repo: Option<&str>,
    row: Option<&crate::project::Repo>,
) -> Result<(String, String)> {
    let repo = repo.context("box_repo_unmapped: a box lane needs a repository")?;
    // A machine-specific mapping wins over a project's generic box row.
    let machine_row = remote::machine_declaration(config_dir, machine)?
        .repos
        .into_iter()
        .find(|r| r.path == repo);
    let configured = if machine_row.is_some() || row.is_none() {
        remote::box_repo_for(config_dir, machine, repo)?
    } else {
        None
    };
    let path = machine_row
        .as_ref()
        .and_then(|r| r.box_path.clone())
        .or_else(|| row.and_then(|r| r.box_path.clone()))
        .or_else(|| configured.as_ref().and_then(|r| r.box_path.clone()));
    let url = machine_row
        .as_ref()
        .and_then(|r| r.publish_url.clone())
        .or_else(|| row.and_then(|r| r.publish_url.clone()))
        .or_else(|| configured.as_ref().and_then(|r| r.publish_url.clone()));
    match (path, url) {
        (Some(box_path), Some(publish_url)) => Ok((box_path, publish_url)),
        (Some(_), None) => bail!(
            "box_publish_url_missing: `{repo}` has a box_path in PROJECT.md but no `publish_url` in that row; add the URL the box fetches the lane branch from (the remote the branch is pushed to) before the first box start"
        ),
        (None, Some(_)) => bail!(
            "box_path_missing: `{repo}` has a publish_url in PROJECT.md but no `box_path` in that row; add the box clone path before the first box start"
        ),
        (None, None) => bail!(
            "box_repo_unmapped: {repo} has no Mac-to-box row; add one before the first box start"
        ),
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

/// Put the pinned report artifacts and source commits on the box before
/// the reviewer process starts. Each transferred file is verified remotely.
fn stage_box_review(
    ctx: &Ctx,
    project: &Project,
    reviewer: &Thread,
    box_root: &str,
    target: &str,
) -> Result<()> {
    let record = crate::review::load(project, &reviewer.review_id)?;
    let state = format!(
        "/{}/{}/.state",
        box_root.trim_start_matches('/'),
        project.slug
    );
    // A partially merged candidate (a conflict) may not contain every member
    // object. Make every pinned source available before the box reviewer runs.
    let profile = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        reviewer.machine_route(),
    )?;
    let settings = project.read_project_md()?.0;
    let (box_repo, url) = box_repo_row(&ctx.config_dir, &settings, &profile.label, &record.repo)?;
    let machine = remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    let git = crate::repo::Git::new(ctx.runner, &record.repo);
    let mut fetch = format!(
        "git -C {} fetch {}",
        remote::quote(&box_repo),
        remote::quote(&url)
    );
    for member in &record.members {
        let event = crate::events::load(project, &member.event)?;
        let published = event
            .payload
            .done
            .as_ref()
            .and_then(|d| d.published_ref.as_deref());
        let reference = format!("refs/heads/{}", published.unwrap_or(&member.branch));
        if published.is_none() {
            thread::update(project, &member.thread, |t| {
                t.review_sources.insert(url.clone(), member.sha.clone());
            })?;
            git.run(&["push", &url, &format!("{}:{reference}", member.sha)])?;
        }
        fetch.push(' ');
        fetch.push_str(&remote::quote(&reference));
    }
    let out = remote::ssh(
        ctx.runner,
        target,
        &remote::with_path(&machine.path, &fetch),
        None,
        Duration::from_secs(120),
    )?;
    if !out.success() {
        bail!("could not fetch pile inputs on box: {}", out.error_text());
    }
    let hashes: std::collections::BTreeSet<_> = record
        .members
        .iter()
        .map(|member| member.artifact.clone())
        .collect();
    for hash in hashes {
        let bytes = thread::artifact(project, &hash)?;
        let text = String::from_utf8(bytes).context("review source artifact is not UTF-8")?;
        remote::write_runtime_file(
            ctx.runner,
            target,
            &format!("{state}/artifacts/{hash}"),
            &text,
            &hash,
        )?;
    }
    Ok(())
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
    let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;

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
        // A review task contains absolute artifact paths. Freeze paths for the
        // machine that will read them, not the coordinator's filesystem.
        let brief = if record.role == "reviewer" && !record.review_id.is_empty() {
            brief.replace(
                &project.state_dir().to_string_lossy().to_string(),
                &format!("{}/{}/.state", machine.root, project.slug),
            )
        } else {
            brief
        };
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
    if record.role == "reviewer" && !record.review_id.is_empty() {
        stage_box_review(ctx, project, record, &machine.root, &target)?;
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
    // The agent launch supplies its exclusive wrapper PATH after bashrc has
    // run. A pane-level PATH cannot enforce this on an interactive shell.
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
    let created = match matching.first() {
        Some(workspace) => herdr
            .tab_create_env(
                &workspace.workspace_id,
                Path::new(&box_worktree),
                &record.id,
                false,
                &env,
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?,
        None => herdr
            .workspace_create_env(Path::new(&box_worktree), &label, false, &env)
            .map_err(|error| anyhow::anyhow!("{error}"))?,
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
    herdr
        .tab_rename(&created.tab_id, &format!("{} starting…", record.id))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // ADE owns attention notices on both machines. Clear historical links
    // before starting; fork notices would bypass the shared outbox.
    if project.coordinator().is_some() {
        herdr
            .pane_clear_tokens(&created.pane_id, &["parent"])
            .map_err(|error| {
                anyhow::anyhow!(
                    "could not clear box pane {} push link: {error}",
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
        return Err(crate::refusal::error(
            format!("integration_branch_required: `{integration}` is not a local branch"),
            "ha thread start <project> --base <existing-branch> --job <job> --task-file <file>",
        ));
    }
    let branch = thread::branch_name(&project.slug, &record.id, &record.title);
    let task = std::fs::read_to_string(thread::task_path(project, &record.id)).unwrap_or_default();
    let planned = Path::new(&record.repo).join(".worktrees").join(&record.id);
    crate::claude_trust::check_folder(ctx, &record.launch.kind, record.is_remote(), &planned)?;
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
    crate::claude_trust::check_folder(ctx, &record.launch.kind, record.is_remote(), &folder)?;
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
        t.recovery_pending = false;
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

/// Resume a persisted recovery without re-picking or touching worktree files.
pub fn place_recovery(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if record.launch_attempts >= thread::MAX_LAUNCH_ATTEMPTS {
        thread::update(project, &record.id, |t| {
            t.recovery_pending = false;
            t.error = "recovery_placement_exhausted: could not open the replacement lane".into();
        })?;
        bail!("recovery_placement_exhausted");
    }
    thread::update(project, &record.id, |t| t.launch_attempts += 1)?;
    let view = require_session(ctx, project)?;
    let herdr = view.herdr.on_machine(record.machine_route());
    if !record.tab_id.is_empty() {
        // Only this lane's failed attempt is stopped, never a coordinator.
        let panes = herdr.pane_list()?;
        if let Some(pane) = panes.iter().find(|p| p.pane_id == record.pane_id) {
            if pane.workspace_id != record.workspace_id || pane.tab_id != record.tab_id {
                bail!("recovery_identity_mismatch: old pane was reused");
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
    thread::update(project, &record.id, |t| t.recovery_pending = false)?;
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
/// same recipe within its retry budget, or waits for evidence.
pub fn retry(ctx: &Ctx, slug: &str, id: &str, reason: &str) -> Result<RetryOutcome> {
    retry_with_ticker(ctx, slug, id, reason, ticker::start, true, false)
}

/// Round recovery already holds the advance lock, so it must not replace and
/// wait for a ticker which may itself be waiting for that lock.
pub(crate) fn retry_during_advance(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    reason: &str,
) -> Result<RetryOutcome> {
    // A manual review retry is the same coordinator decision as `thread
    // retry`; only its ticker handling differs because advance_lock is held.
    retry_with_ticker(ctx, slug, id, reason, ticker::ensure, true, true)
}

fn retry_with_ticker(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    reason: &str,
    ensure_ticker: fn(&Ctx<'_>) -> Result<()>,
    launch_now: bool,
    replace_busy: bool,
) -> Result<RetryOutcome> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if !replace_busy {
        crate::review::require_resolvable(&project, id)?;
    }
    if record.role == "reviewer" && !record.review_id.is_empty() {
        let review = crate::review::load(&project, &record.review_id)?;
        if let Some(bound) = review.reviewer.as_deref()
            && bound != id
        {
            bail!(
                "reviewer_already_bound: `{bound}` is bound to `{}`; the review's own retry handles its reviewer",
                record.review_id
            );
        }
    }
    // A crash after the attempt transition but before placement resumes the
    // same selected attempt. It must not spend another routing recovery.
    if record.recovery_pending {
        ensure_ticker(ctx)?;
        place_prelaunch_recovery(ctx, &project, &record)?;
        if launch_now {
            ticker::launch_thread_now(ctx, &project, id)?;
        }
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
        return Err(crate::refusal::error(
            "retry_adopted: an adopted process has no launch recipe; use `thread rebind`",
            format!("ha thread rebind {slug} {id} --pane <verified-pane>"),
        ));
    }
    if record.status == Status::Resolved {
        return Err(crate::refusal::error(
            format!("retry_resolved: {id} is resolved"),
            format!("ha thread show {slug} {id}"),
        ));
    }
    if record.parked {
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(crate::refusal::error(
                "retry_reason_missing: say why the parked lane is needed again",
                format!("ha thread retry {slug} {id} --reason \"<why needed again>\""),
            ));
        }
        // A correction to sealed work keeps the same branch and attempt.
        // Routing recovery is only for a failed start or failed work.
        let outcome = prompt(ctx, slug, id, reason)?;
        return Ok(RetryOutcome {
            thread: id.into(),
            attempt: match outcome {
                PromptOutcome::Queued { attempt } | PromptOutcome::Sent { attempt, .. } => attempt,
            },
            pane_id: thread::load(&project, id)?.pane_id,
            recipe: record.launch.recipe_id,
            screen: None,
        });
    }
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(crate::refusal::error(
            "retry_reason_missing: say why the attempt is being replaced",
            format!("ha thread retry {slug} {id} --reason \"<why replace attempt>\""),
        ));
    }
    let view = require_session(ctx, &project)?;
    let herdr = view.herdr.on_machine(record.machine_route());
    if !replace_busy {
        refuse_busy_retry(&herdr, &record)?;
    }
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
    // No agent was submitted: retry the same selection without spending a
    // process/provider recovery allowance.
    let mut launch = if record.launch_attempts == 0 {
        record.launch.clone()
    } else {
        crate::launch::resolve_coordinator_retry(ctx, &project, &input, record.failure_class)?
    };
    launch.attempt = record.attempt.max(1).saturating_add(1);
    launch.brief_hash = record.launch.brief_hash.clone();

    // Local placement also stores an empty machine. Its placement reason is
    // the evidence that it was selected (possibly by an explicit --machine
    // local), so only dispatch again when no placement was recorded at all.
    let unplaced = record.machine.is_empty()
        && record.placement_reason.is_empty()
        && record.pane_id.is_empty()
        && record.tab_id.is_empty()
        && record.worktree_path.is_empty();
    let placement = if unplaced {
        launch.machine = crate::launch::parse_launch_config(&ctx.config_dir)?
            .dispatch
            .machine;
        let (settings, _) = project.read_project_md()?;
        let listed = settings.repos.iter().find(|row| {
            std::fs::canonicalize(&row.path).is_ok_and(|path| path.to_string_lossy() == record.repo)
        });
        let placement =
            resolve_placement(ctx, None, &record.role, &launch, Some(&record.repo), listed)?;
        crate::launch::dispatch(
            &project,
            serde_json::json!({"kind":"placement", "recipe":launch.recipe_id,
                "machine":placement.dispatch_machine(), "reason":placement.reason,
                "tried":placement.tried}),
        )?;
        launch.machine = placement.dispatch_machine().to_string();
        Some(placement)
    } else {
        None
    };
    let selected_recipe = launch.recipe_id.clone();
    thread::update_checked(&project, id, |t| {
        if t.attempt != record.attempt || t.pane_id != record.pane_id {
            bail!("retry_stale: thread changed while its replacement was prepared");
        }
        t.attempt = launch.attempt;
        t.agent = launch.kind.clone();
        if let Some(placement) = &placement {
            t.machine = placement.machine.clone();
            t.machine_id = placement.machine_id.clone();
            t.placement_reason = format!("retry: {}", placement.reason);
        } else {
            let machine = if t.machine.is_empty() {
                "local"
            } else {
                &t.machine
            };
            t.placement_reason = format!(
                "retry on `{machine}` with recipe `{}`; machine kept from the previous attempt",
                launch.recipe_id
            );
        }
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
        t.recovery_pending = true;
        Ok(())
    })?;

    ensure_ticker(ctx)?;
    place_prelaunch_recovery(ctx, &project, &thread::load(&project, id)?)?;
    if launch_now {
        ticker::launch_thread_now(ctx, &project, id)?;
    }
    let placed = thread::load(&project, id)?;
    Ok(RetryOutcome {
        thread: placed.id,
        attempt: placed.attempt,
        pane_id: placed.pane_id,
        recipe: selected_recipe,
        screen,
    })
}

// Placement counts its own tries; if it failed before submitting an agent,
// keep the launch counter at zero so review recovery never calls it process gone.
// A reviewer is retried by the review's retry, not the ticker's immediate
// recovery pass (which would bypass that clock and exhaust placement).
fn place_prelaunch_recovery(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    let result = place_recovery(ctx, project, record);
    if let Err(error) = &result
        && record.launch_attempts == 0
    {
        thread::update(project, &record.id, |t| {
            t.launch_attempts = 0;
            if t.role == "reviewer" {
                t.recovery_pending = false;
                t.error = format!("{error:#}");
            }
        })?;
    }
    result
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
        return Err(crate::refusal::error(
            format!("rebind_resolved: {id} is resolved"),
            format!("ha thread show {slug} {id}"),
        ));
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
    if !record.agent_name.is_empty() && !agent.name.is_empty() && agent.name != record.agent_name {
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
    herdr.pane_clear_tokens(pane_id, &["parent"])?;
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
        return Err(crate::refusal::error(
            format!("cancel_reason_missing: say why {id} is being stopped"),
            format!("ha thread cancel {slug} {id} --reason \"<why stop>\""),
        ));
    }
    crate::review::require_resolvable(&project, id)?;
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
        t.cleanup_pending = true;
        if !t.cleanup_reason.starts_with("retained worktree removal: ") {
            t.cleanup_reason = "cancelled".into();
        }
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
    let mut worktree = "not_applicable".to_string();
    let mut worktree_reason = close_error;
    if removable_folder(&project, &record) {
        if !worktree_exists(ctx, &project, &record)? {
            thread::update(&project, id, |t| t.worktree_path.clear())?;
            worktree = "removed".into();
        } else if pane == "cleanup_pending" {
            worktree = "kept".into();
        } else if let Err(error) = preserve_report_links(ctx, &project, &record) {
            let detail = format!("linked_files_not_kept: {error:#}");
            worktree = "kept".into();
            worktree_reason = Some(detail);
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
    let mut cleanup_failed = false;
    if pane != "cleanup_pending" {
        // The review may already be cancelling when a superseded reviewer reaches
        // here. Keep cleanup retryable instead of failing its worker's prompt.
        let cleanup = (|| -> Result<()> {
            // A cleared worktree path still owes owned-ref retirement on retry.
            if worktree == "removed" || !removable_folder(&project, &record) {
                crate::branches::resolved_thread(ctx, &project, &record)?;
            }
            remove_finished_build_folder(ctx, &project, &record)?;
            remove_scratch_session(ctx, &record)?;
            Ok(())
        })();
        match cleanup {
            Ok(())
                if !worktree_reason
                    .as_ref()
                    .is_some_and(|reason| reason.starts_with("linked_files_not_kept:")) =>
            {
                thread::update(&project, id, |t| {
                    t.cleanup_pending = false;
                    t.cleanup_reason.clear();
                })?;
            }
            Ok(()) => {}
            Err(error) => {
                cleanup_failed = true;
                worktree_reason = Some(format!("cleanup pending: {error:#}"));
            }
        }
    }
    if let Some(detail) = &worktree_reason {
        thread::update(&project, id, |t| {
            if t.cleanup_pending && !t.cleanup_reason.starts_with("retained worktree removal: ") {
                t.cleanup_reason = detail.clone();
            }
        })?;
    }
    refresh_plan(ctx, &project);
    Ok(CancelOutcome {
        thread: id.to_string(),
        state: if pane == "cleanup_pending"
            || cleanup_failed
            || worktree_reason
                .as_ref()
                .is_some_and(|reason| reason.starts_with("linked_files_not_kept:"))
        {
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
/// review. The ticker can then retry the whole final-copy and cleanup path.
pub(crate) fn resolve_automatically(
    ctx: &Ctx,
    project: &Project,
    id: &str,
    reason: &str,
) -> ResolveOutcome {
    resolve_automatically_with_views(ctx, project, id, reason, &mut CleanupViews::default())
}

pub(crate) fn resolve_automatically_with_views(
    ctx: &Ctx,
    project: &Project,
    id: &str,
    reason: &str,
    views: &mut CleanupViews,
) -> ResolveOutcome {
    // Record the terminal lifecycle before touching Herdr or a worktree. A
    // process death at any later instruction leaves a ticker-visible retry.
    let before = thread::load(project, id).unwrap_or_default();
    if let Err(error) = thread::update(project, id, |t| {
        t.status = Status::Resolved;
        t.resolved_reason = "cleanup pending".into();
        t.prompt_pending = false;
        t.cleanup_pending = true;
        if !t.cleanup_reason.starts_with("retained worktree removal: ") {
            t.cleanup_reason = reason.to_string();
        }
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
    let attempted = resolve_with_views(ctx, &project.slug, id, &ResolveArgs::default(), views);
    match attempted {
        Ok(mut outcome) => {
            let pending = thread::load(project, id).is_ok_and(|t| t.cleanup_pending);
            if !pending {
                let _ = thread::update(project, id, |t| {
                    t.resolved_reason = reason.to_string();
                    t.cleanup_reason.clear();
                });
            } else {
                outcome.state = "cleanup_pending".into();
            }
            outcome
        }
        Err(error) => {
            let detail = format!("{error:#}");
            let _ = thread::update(project, id, |t| {
                if t.cleanup_pending && !t.cleanup_reason.starts_with("retained worktree removal: ")
                {
                    t.cleanup_reason = detail.clone();
                }
            });
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

/// Retry a bounded slice of durable cleanup, including after a landed pile.
pub(crate) fn retry_pending_cleanup(ctx: &Ctx, project: &Project) -> Result<()> {
    let mut views = CleanupViews::default();
    let mut pending = thread::list_live(project)
        .into_iter()
        .filter(|record| record.cleanup_pending)
        .filter(|record| !record.cleanup_reason.starts_with("linked_files_not_kept:"))
        .collect::<Vec<_>>();
    // Every attempt updates the thread. Oldest first prevents an unreachable
    // member at the front of the pile from starving the rest of the queue.
    pending.sort_by(|a, b| a.updated.cmp(&b.updated).then(a.id.cmp(&b.id)));
    for record in pending.into_iter().take(CLEANUP_BATCH_SIZE) {
        // A moved-tip refusal can become resolvable after a reviewed box seal
        // lands; let the pinned branch checks decide on each retry.
        if !record.cancellation_reason.is_empty() {
            if let Err(error) = cancel(ctx, &project.slug, &record.id, &record.cancellation_reason)
            {
                eprintln!("note: cleanup pending for {}: {error:#}", record.id);
            }
        } else {
            let reason = if record.cleanup_reason.is_empty() {
                "automatic"
            } else {
                &record.cleanup_reason
            };
            resolve_automatically_with_views(ctx, project, &record.id, reason, &mut views);
        }
    }

    Ok(())
}

/// A report-only code lane has nothing to review or land. Once its sealed
/// report is available, close it immediately; changed lanes remain visible so
/// the coordinator can put them in a pile.
pub(crate) fn resolve_report_only(ctx: &Ctx, project: &Project) {
    let events = crate::events::for_unresolved_threads(project);
    for record in thread::list_live(project) {
        if record.status == Status::Resolved || record.role == "reviewer" {
            continue;
        }
        let latest = crate::events::latest_event(&events, &record.id, record.attempt.max(1));
        let unchanged = latest
            .and_then(|event| event.payload.done.as_ref())
            .is_some_and(|done| {
                done.has_changes == Some(false)
                    || record.changes_seal == latest.map(|e| e.id.as_str()).unwrap_or("")
                        && record.has_changes == Some(false)
            });
        // Report-only lanes have no review pin to hold them open. A queued
        // correction or one delivered after this seal still needs a new seal.
        let already_merged = !record.merged_sha.is_empty() && record.merged_review.is_empty();
        if (unchanged || already_merged) && !follow_up_pending_for_seal(&record, latest) {
            resolve_automatically(
                ctx,
                project,
                &record.id,
                if already_merged {
                    "merged before cutover"
                } else {
                    "report-only"
                },
            );
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "delivery")]
pub enum PromptOutcome {
    Queued { attempt: u32 },
    Sent { attempt: u32, agent_state: String },
}

pub(crate) fn follow_up_pending_for_seal(
    record: &Thread,
    latest: Option<&crate::contracts::Event>,
) -> bool {
    record.follow_ups.iter().any(|f| {
        f.attempt == record.attempt.max(1)
            && (matches!(f.state, FollowUpState::Queued | FollowUpState::Uncertain)
                || f.state == FollowUpState::Delivered
                    && !f.after_seal.is_empty()
                    && latest.is_some_and(|event| event.id == f.after_seal))
    })
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

/// Box lane state has no Mac review records. Publish the barriers before the
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

/// Commit transport success and the answered wait as one lifecycle update.
/// A new attempt cannot borrow the old answer, but delivery history remains.
pub(crate) fn record_follow_up_delivery(
    project: &Project,
    id: &str,
    index: usize,
    follow_up: &FollowUp,
    after_seal: &str,
) -> Result<()> {
    thread::update_checked(project, id, |thread| {
        let saved = thread
            .follow_ups
            .get_mut(index)
            .context("queued follow-up disappeared during delivery")?;
        if saved.attempt != follow_up.attempt
            || saved.text != follow_up.text
            || saved.state != FollowUpState::Uncertain
        {
            bail!("queued follow-up changed during delivery");
        }
        saved.state = FollowUpState::Delivered;
        saved.delivered_at = project::now();
        saved.after_seal = after_seal.to_string();
        if thread.attempt.max(1) == follow_up.attempt {
            if !follow_up.waiting_event.is_empty() {
                thread.answered_waiting_event = follow_up.waiting_event.clone();
            }
            thread.connection_waiting = false;
            thread.connection_resumes.clear();
            thread.failure_class = crate::contracts::FailureClass::Unknown;
            thread.provider_failure_kind = None;
        }
        Ok(())
    })?;
    Ok(())
}

/// Herdr's PTY/activity errors may follow submission. Only these explicit
/// refusals prove no prompt was written and permit another delivery attempt.
pub(crate) fn prompt_refused_before_submission(error: &crate::herdr::HerdrError) -> bool {
    matches!(
        error.code.as_str(),
        "agent_not_ready" | "agent_blocked" | "agent_not_found" | "empty_agent_prompt"
    )
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
        Status::Resolved => {
            return Err(crate::refusal::error(
                format!("{id} is resolved"),
                format!("ha thread show {slug} {id}"),
            ));
        }
        Status::Failed => {
            return Err(crate::refusal::error(
                format!("{id} is gone"),
                format!("ha thread retry {slug} {id} --reason \"<why replace attempt>\""),
            ));
        }
        Status::Starting | Status::Open => {}
    }
    // The brief and every follow-up have one ordered delivery path. Once one
    // message is queued, later messages join it until the ticker drains them.
    if !record.parked
        && (record.status == Status::Starting
            || record.prompt_pending
            || awaiting_bootstrap(&record)
            || awaiting_follow_up(&record))
    {
        let events_before_send = crate::events::checked(&project)?;
        // Keep the queued message invisible to the ticker until every review
        // it affects is durably held. Working also closes the gap between this
        // thread update and that hold for an already accepted reviewer.
        let previous_group = record.last_group.clone();
        thread::update(&project, id, |thread| {
            thread.last_group = Group::Working.token().to_string();
        })?;
        if let Err(error) = crate::review::require_follow_up(&project, id) {
            thread::update(&project, id, |thread| {
                thread.last_group = previous_group;
            })?;
            return Err(error);
        }
        let mut queued = false;
        record = thread::update_checked(&project, id, |thread| {
            match thread.status {
                Status::Resolved => {
                    return Err(crate::refusal::error(
                        format!("{id} is resolved"),
                        format!("ha thread show {slug} {id}"),
                    ));
                }
                Status::Failed => {
                    return Err(crate::refusal::error(
                        format!("{id} is gone"),
                        format!("ha thread retry {slug} {id} --reason \"<why replace attempt>\""),
                    ));
                }
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
                    queued_at: project::now(),
                    ..FollowUp::default()
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
    if record.parked {
        let events = crate::events::checked(&project)?;
        let previous_group = record.last_group.clone();
        thread::update(&project, id, |t| {
            t.last_group = Group::Working.token().into()
        })?;
        if let Err(error) = crate::review::require_follow_up(&project, id) {
            thread::update(&project, id, |t| t.last_group = previous_group)?;
            return Err(error);
        }
        // Queue the correction in the same durable update that un-parks the
        // new pane. Otherwise a ticker pass between starting the agent and
        // sending the text can close it against the previous done seal.
        reopen_parked(ctx, &project, &record, text, &events)?;
        return Ok(PromptOutcome::Queued {
            attempt: record.attempt.max(1),
        });
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
    let events_before_send = crate::events::checked(&project)?;
    // Mark the work before invalidating earlier review evidence. The merge
    // boundary sees either this working state or the durable hold. If an
    // already committed intent refuses the hold, restore the prior projection
    // and never deliver the text.
    let previous_group = record.last_group.clone();
    thread::update(&project, id, |thread| {
        thread.last_group = Group::Working.token().to_string();
    })?;
    if let Err(error) = crate::review::require_follow_up(&project, id) {
        thread::update(&project, id, |thread| {
            thread.last_group = previous_group;
        })?;
        return Err(error);
    }
    sync_box_corrections(ctx, &project, &record)?;
    let attempt = record.attempt.max(1);
    let follow_up = FollowUp {
        attempt,
        text: text.to_string(),
        state: FollowUpState::Uncertain,
        waiting_event: latest_waiting_event_id(&events_before_send, id, attempt)
            .unwrap_or_default(),
        queued_at: project::now(),
        ..FollowUp::default()
    };
    let _prompt_lock = thread::prompt_lock(&project, id)?;
    let mut queued = false;
    let staged = thread::update_checked(&project, id, |thread| {
        if thread.status != Status::Open
            || thread.attempt.max(1) != attempt
            || thread.pane_id != record.pane_id
        {
            bail!("prompt_attempt_changed: {id} changed during prompt preparation");
        }
        queued = thread.prompt_pending || awaiting_bootstrap(thread) || awaiting_follow_up(thread);
        let mut saved = follow_up.clone();
        if queued {
            saved.state = FollowUpState::Queued;
        }
        thread.follow_ups.push(saved);
        Ok(())
    })?;
    if queued {
        return Ok(PromptOutcome::Queued { attempt });
    }
    let index = staged.follow_ups.len() - 1;
    let after_seal = crate::events::latest_done_event(&events_before_send, id, attempt)
        .map(|event| event.id.clone())
        .unwrap_or_default();
    // Adapter-owned error screens recover through their pane input hook.
    let result = if state == "blocked" {
        herdr.pane_submit_text(&record.pane_id, text)
    } else {
        let timeout = if record.launch.ready_timeout_ms == 0 {
            crate::herdr::AGENT_START_TIMEOUT.as_millis() as u64
        } else {
            record.launch.ready_timeout_ms
        };
        herdr.agent_prompt_wait_started(&record.pane_id, text, timeout)
    };
    if let Err(error) = result {
        if prompt_refused_before_submission(&error) {
            thread::update(&project, id, |thread| {
                thread.follow_ups[index].state = FollowUpState::Queued;
            })?;
        }
        return Err(anyhow::anyhow!("{error}"));
    }
    record_follow_up_delivery(&project, id, index, &follow_up, &after_seal)?;
    if state == "blocked" {
        thread::update(&project, id, |t| {
            if t.attempt.max(1) == attempt {
                t.error.clear();
            }
        })?;
    }
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
            Err(crate::refusal::error(
                format!(
                    "agent_blocked: {} is waiting on the user in its pane ({})",
                    record.id, record.pane_id
                ),
                "wait for the user to answer in the agent pane; then retry the command",
            ))
        }
        "unknown" => Err(crate::refusal::error(
            format!("{}'s agent state is unknown; not sending", record.id),
            "wait for the agent process to report a known state; a live state update clears this refusal",
        )),
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
            "ha thread attest <project> <thread> --reason \"<reason>\"",
        ));
    }
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if record.status != Status::Resolved {
        return Err(crate::refusal::error(
            format!("attest_not_resolved: {id} is not resolved"),
            format!("ha thread show {slug} {id}"),
        ));
    }
    if !record.cancellation_reason.is_empty() {
        return Err(crate::refusal::error(
            format!("attest_cancelled: {id} was cancelled"),
            format!("ha thread show {slug} {id}"),
        ));
    }
    let attempt = record.attempt.max(1);
    if crate::events::checked(&project)?
        .iter()
        .any(|event| event.thread == id && event.attempt == attempt && event.payload.done.is_some())
    {
        return Err(crate::refusal::error(
            format!(
                "attest_already_done: {id} already has sealed done evidence for attempt {attempt}"
            ),
            format!("ha thread show {slug} {id}"),
        ));
    }

    let draft_path =
        (!record.thread_dir.is_empty()).then(|| Path::new(&record.thread_dir).join("report.md"));
    let historical_path = thread::home_report_path(&project, id);
    let report_path = draft_path
        .iter()
        .chain(std::iter::once(&historical_path))
        .find(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()))
        .ok_or_else(|| {
            crate::refusal::error(
                format!(
                    "attest_report_missing: {} has no preserved report draft",
                    id
                ),
                format!("ha thread show {slug} {id}"),
            )
        })?;
    let bytes = std::fs::read(report_path)
        .with_context(|| format!("could not read report {}", report_path.display()))?;
    let actual_hash = thread::sha256_hex(&bytes);
    if record.report_hash.is_empty() || actual_hash != record.report_hash {
        return Err(crate::refusal::error(
            format!(
                "attest_report_mismatch: stored report hashes to {actual_hash}, record names {}",
                if record.report_hash.is_empty() {
                    "no hash"
                } else {
                    &record.report_hash
                }
            ),
            format!("ha thread show {slug} {id}"),
        ));
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
            crate::refusal::error(
                "attest_coordinator_missing: project has no coordinator binding",
                "wait for the project coordinator to bind a pane before attesting",
            )
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
        recipient: crate::contracts::Recipient {
            pane: coordinator.pane_id.clone(),
            coordinator_attempt: coordinator.attempt(),
        },
        created: project::now(),
        payload: crate::contracts::EventPayload {
            done: Some(crate::contracts::DonePayload {
                has_changes: None,
                sha: sha.clone().unwrap_or_default(),
                report_path: crate::events::artifact_path(&project, &artifact)
                    .to_string_lossy()
                    .into_owned(),
                artifact: artifact.clone(),
                attestation: Some(crate::contracts::Attestation {
                    coordinator: coordinator_name.clone(),
                    reason: reason.to_string(),
                }),
                published_ref: None,
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
    crate::events::for_thread(project, &record.id)
        .into_iter()
        .filter(|event| event.attempt == record.attempt.max(1))
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
                "The worktree {} was removed.\n",
                self.worktree_path
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
    resolve_with_views(ctx, slug, id, args, &mut CleanupViews::default())
}

fn resolve_with_views(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    args: &ResolveArgs,
    views: &mut CleanupViews,
) -> Result<ResolveOutcome> {
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
    let events = crate::events::for_thread(&project, id);
    if follow_up_pending_for_seal(
        &record,
        crate::events::latest_done_event(&events, id, record.attempt.max(1)),
    ) {
        bail!(
            "follow_up_pending: {id} must finish the queued follow-up and seal again before resolution"
        );
    }
    crate::review::require_resolvable(&project, id)?;

    let removable = removable_folder(&project, &record);
    let already_removed = removable && !worktree_exists(ctx, &project, &record)?;

    // Every path that resolves a thread performs a final copy first.
    let mut removal_refusal = None;
    let (mut final_copy, mut copy_notes) = if args.skip_copy {
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

    // Even an explicit copy override cannot delete linked files that were not
    // preserved. The sealed report is still available when --skip-copy is set.
    {
        let preserved = thread::load(&project, id)?;
        if let Err(error) = preserve_report_links(ctx, &project, &preserved) {
            let detail = format!("linked_files_not_kept: {error:#}");
            copy_notes.push(detail.clone());
            final_copy = "partial".into();
            removal_refusal = Some(detail);
        }
    }

    let mut pane_closed = false;
    let mut worktree_removed = false;
    if already_removed {
        thread::update(&project, id, |t| t.worktree_path.clear())?;
        worktree_removed = true;
    } else if removable {
        if args.keep_pane && removal_refusal.is_none() {
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
                return Err(crate::refusal::error(
                    format!(
                        "worktree_dirty: uncommitted changes in {}; not removing ({})",
                        record.worktree_path,
                        inspection.dirty.join(", ")
                    ),
                    format!("ha thread show {slug} {id}"),
                ));
            }
            removal_refusal = inspection.ignored_reason(&record.worktree_path);
        }
        if removal_refusal.is_none() {
            removal_in_use_gate(ctx, &project, &record, views)?;
            // Stop the idle agent before removing its current directory. This
            // also makes the raw box-side `git worktree remove` independent of
            // Herdr workspace ownership.
            pane_closed = close_pane_with_views(ctx, &project, &record, views)?;
            remove_worktree(ctx, &project, &record)?;
            thread::update(&project, id, |t| t.worktree_path.clear())?;
            worktree_removed = true;
        }
    }
    if worktree_removed || !removable {
        crate::branches::resolved_thread(ctx, &project, &record)?;
    }
    let resolved = thread::update(&project, id, |t| {
        t.status = Status::Resolved;
        t.resolved_reason = "manual".into();
        t.prompt_pending = false;
    })?;
    thread::update(&project, id, |t| {
        t.cleanup_pending = removal_refusal
            .as_ref()
            .is_some_and(|r| r.starts_with("linked_files_not_kept:"));
        t.cleanup_reason = if t.cleanup_pending {
            removal_refusal.clone().unwrap_or_default()
        } else {
            String::new()
        };
    })?;
    if let Ok(Some(view)) = views.for_thread(ctx, &project, &resolved) {
        clear_thread_tokens(&view.herdr, &resolved);
    }
    let pane_closed = if args.keep_pane || pane_closed {
        pane_closed
    } else {
        close_pane_with_views(ctx, &project, &resolved, views)?
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
    } else if let Some(reason) = &removal_refusal {
        ("kept", Some(reason.clone()))
    } else if resolved.kind == Kind::Worktree || managed_git_folder(&project, &resolved) {
        ("not_recorded", None)
    } else {
        ("not_applicable", None)
    };
    refresh_plan(ctx, &project);
    Ok(ResolveOutcome {
        thread: id.to_string(),
        state: if resolved.cleanup_pending
            || removal_refusal
                .as_ref()
                .is_some_and(|r| r.starts_with("linked_files_not_kept:"))
        {
            "cleanup_pending"
        } else {
            "resolved"
        }
        .into(),
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
        || (record.launch_attempts > 0 && !record.startup_wait_started.is_empty()))
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
        || (record.launch_attempts > 0
            && record.prompt_pending
            && !record.error.starts_with("brief_delivery_failed:")
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
    Ok(
        fail_start_checked(ctx, project, id, reason, class, recover, None)?
            .expect("unconditional failed start must update the record"),
    )
}

/// A snapshot-driven failure must still belong to the same open attempt when
/// the transition takes the record lock. Another caller may place a new pane
/// between a fresh Herdr query and this update.
pub(crate) fn fail_start_checked(
    ctx: &Ctx,
    project: &Project,
    id: &str,
    reason: &str,
    class: crate::contracts::FailureClass,
    recover: bool,
    expected: Option<&Thread>,
) -> Result<Option<Thread>> {
    let provider_kind = None;
    let record = thread::load(project, id)?;
    let (recovery, recovery_error) = if !recover
        || (record.launch_attempts == 0 && !(record.is_remote() && expected.is_some()))
    {
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
    let mut matched = false;
    let failed = thread::update_checked(project, id, |t| {
        if expected.is_some_and(|old| {
            !matches!(t.status, Status::Open | Status::Starting)
                || t.attempt != old.attempt
                || t.pane_id != old.pane_id
                || t.tab_id != old.tab_id
                || t.workspace_id != old.workspace_id
                || (class != crate::contracts::FailureClass::ProcessGone
                    && !t.report_hash.is_empty())
        }) {
            return Ok(());
        }
        if expected.is_some() && attempt_sealed(project, t) {
            return Ok(());
        }
        matched = true;
        if class == crate::contracts::FailureClass::ProcessGone
            && !t.parked
            && !attempt_sealed(project, t)
        {
            t.start_notices.push(crate::steps::Notice {
                line: format!("GONE {id} attempt {}", t.attempt.max(1)),
                submitted: false,
            });
        }
        t.status = Status::Failed;
        if recovery.is_none() {
            let reason = crate::steps::short_error(
                &recovery_error.clone().unwrap_or_else(|| reason.to_string()),
            );
            t.start_notices.push(crate::steps::Notice {
                line: format!(
                    "FAILED {id}: {reason} — next: ha thread retry {} {id}",
                    project.slug
                ),
                submitted: false,
            });
        }
        t.prompt_pending = false;
        t.startup_wait_started.clear();
        t.provider_wait_started.clear();
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
            t.recovery_pending = true;
        }
        t.last_group = Group::WaitingOnYou.token().to_string();
        Ok(())
    })?;
    if !matched {
        return Ok(None);
    }
    // A confirmed missing box pane has nothing left to close. Do not make
    // recovery depend on the Mac coordinator session being reachable.
    if !(expected.is_some() && failed.is_remote()) {
        if !failed.tab_id.is_empty() {
            let view = session_view(ctx, project).context(
                "failed-start cleanup could not reach the session; the attempt stays bound",
            )?;
            clear_thread_tokens(&view.herdr, &failed);
        }
        close_pane(ctx, project, &failed)?;
    }
    Ok(Some(failed))
}

/// Close the thread's pane and tab. A dedicated lane workspace is closed as
/// one unit, but a workspace containing another pane, tab, or agent is shared
/// and only this lane's tab is closed. A tab herdr no longer knows, or a
/// session it cannot reach, has nothing to close and is not an error.
/// A durable completion is parkable when no follow-up expects another seal.
/// Delivery may still be in the outbox; closure must not add another notice.
pub(crate) fn attempt_sealed(project: &Project, record: &Thread) -> bool {
    let events = crate::events::for_thread(project, &record.id);
    let latest = crate::events::latest_event(&events, &record.id, record.attempt.max(1));
    latest.is_some()
        && !follow_up_pending_for_seal(record, latest)
        && !record.follow_ups.iter().any(|f| {
            f.attempt == record.attempt.max(1)
                && f.state == FollowUpState::Delivered
                && latest.is_some_and(|event| f.waiting_event == event.id)
        })
}

pub(crate) fn parkable(project: &Project, record: &Thread) -> bool {
    if record.status != Status::Open || record.prompt_pending {
        return false;
    }
    let events = crate::events::for_thread(project, &record.id);
    let Some(done) = crate::events::latest_done_event(&events, &record.id, record.attempt.max(1))
    else {
        return false;
    };
    !record.follow_ups.iter().any(|f| {
        f.attempt == record.attempt.max(1)
            && (matches!(f.state, FollowUpState::Queued | FollowUpState::Uncertain)
                || (f.state == FollowUpState::Delivered && f.after_seal == done.id))
    })
}

/// Bring a completed lane back without provisioning its branch or replacing
/// its frozen task. The old agent session id is kept across the pane close.
fn reopen_parked(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
    text: &str,
    events: &[crate::contracts::Event],
) -> Result<()> {
    let view = require_session(ctx, project)?;
    let herdr = view.herdr.on_machine(record.machine_route());
    let coordinator = project
        .coordinator()
        .context("project coordinator missing")?;
    let machine = if record.is_remote() {
        Some(remote::declaration_for_route(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            record.machine_route(),
        )?)
    } else {
        None
    };
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
        &record.launch.brief_hash,
        machine.as_ref(),
        &spec,
    );
    let folder = Path::new(&record.worktree_path);
    let created = if record.is_remote() {
        let _lock =
            project::remote_workspace_lock(&ctx.root, &project.slug, record.machine_route())?;
        let (settings, _) = project.read_project_md()?;
        let label = project::display_name(&settings.name, &project.slug);
        let workspaces: Vec<_> = herdr
            .workspace_list()?
            .into_iter()
            .filter(|w| w.label == label)
            .collect();
        if workspaces.len() > 1 {
            bail!("remote_workspace_duplicate: multiple workspaces for {label}");
        }
        match workspaces.first() {
            Some(workspace) => {
                herdr.tab_create_env(&workspace.workspace_id, folder, &record.id, false, &env)?
            }
            None => herdr.workspace_create_env(folder, &label, false, &env)?,
        }
    } else {
        herdr.tab_create_env(&coordinator.workspace_id, folder, &record.id, false, &env)?
    };
    let cwd = herdr
        .pane_cwd(&created.pane_id)
        .unwrap_or_else(|_| record.worktree_path.clone());
    let mut placed = record.clone();
    placed.workspace_id = created.workspace_id;
    placed.tab_id = created.tab_id;
    placed.pane_id = created.pane_id;
    placed.cwd = if cwd.is_empty() {
        record.worktree_path.clone()
    } else {
        cwd
    };
    let old_session = record.identity.agent_session.as_deref();
    let mut args = record.launch.args.clone();
    let resuming = match (record.launch.kind.as_str(), old_session) {
        ("pi", Some(id)) => {
            args.extend(["--session".into(), id.into()]);
            true
        }
        ("claude", Some(id)) => {
            args.extend(["--resume".into(), id.into()]);
            true
        }
        ("codex", Some(id)) => {
            args.extend(["resume".into(), id.into()]);
            true
        }
        _ => false,
    };
    // A fresh agent must read the old report and the new instruction, rather
    // than silently starting from an empty conversation.
    let result = (|| -> Result<()> {
        if let Some(machine) = &machine {
            herdr.pane_clear_tokens(&placed.pane_id, &["parent"])?;
            // The box's `ha done` authenticates the pane against its lane
            // card. Rebind that card before starting a resumed agent.
            let profile = remote::machine_profile(
                ctx.runner,
                &ctx.env.herdr_bin(),
                &ctx.config_dir,
                record.machine_route(),
            )?;
            let (settings, _) = project.read_project_md()?;
            let (box_repo, publish_url) =
                box_repo_row(&ctx.config_dir, &settings, &profile.label, &record.repo)?;
            let card = crate::contracts::LaneCard {
                project: project.slug.clone(),
                thread: record.id.clone(),
                attempt: record.attempt.max(1),
                brief_hash: record.launch.brief_hash.clone(),
                role: record.role.clone(),
                kind: record.launch.kind.clone(),
                pane_id: placed.pane_id.clone(),
                machine_label: record.machine.clone(),
                machine_id: record.machine_id.clone(),
                box_repo,
                box_worktree: record.worktree_path.clone(),
                brief_commit: record.base.clone(),
                branch: record.branch.clone(),
                publish_url,
                recipient: crate::contracts::Recipient {
                    pane: coordinator.pane_id.clone(),
                    coordinator_attempt: coordinator.attempt(),
                },
                start_line: thread::launch_prompt(
                    &format!("{} --root {}", machine.ade_bin, machine.root),
                    &project.slug,
                    record,
                ),
                created: project::now(),
            };
            remote::provision_card(
                ctx.runner,
                &profile.target,
                &project.slug,
                &format!(
                    "{}/{}/.state/lanes/{}.toml",
                    machine.root, project.slug, record.id
                ),
                &toml::to_string(&card)?,
            )?;
        }
        let agent = herdr.agent_start_opts(&crate::herdr::AgentStart {
            name: &record.agent_name,
            kind: &record.launch.kind,
            pane: &placed.pane_id,
            agent_args: &args,
            launch_bin: None,
            parent: None,
            ready_timeout_ms: record.launch.ready_timeout_ms,
        })?;
        let process = herdr
            .pane_process_info(&placed.pane_id)
            .ok()
            .and_then(|info| info.identity(&record.launch.kind));
        thread::update_checked(project, &record.id, |t| {
            if !t.parked || t.attempt != record.attempt {
                bail!("reopen_stale: completion changed during reopen");
            }
            t.workspace_id = placed.workspace_id.clone();
            t.tab_id = placed.tab_id.clone();
            t.pane_id = placed.pane_id.clone();
            t.cwd = placed.cwd.clone();
            t.parked = false;
            t.last_group = Group::Working.token().into();
            // Only a resumed conversation has consumed the frozen brief.
            // A fresh process must earn a receipt for its new pane before any
            // queued correction is delivered.
            t.prompt_pending = !resuming;
            t.bootstrap = if resuming {
                "acknowledged".into()
            } else {
                String::new()
            };
            t.follow_ups.push(FollowUp {
                attempt: t.attempt.max(1),
                text: if resuming {
                    text.to_string()
                } else {
                    format!(
                        "Read your frozen brief at {}/brief.md and sealed report at {}. Continue in the same folder.\n\n{}",
                        record.thread_dir,
                        record.report_path(),
                        text
                    )
                },
                state: FollowUpState::Queued,
                waiting_event: latest_waiting_event_id(events, &record.id, record.attempt.max(1))
                    .unwrap_or_default(),
                queued_at: project::now(),
                ..FollowUp::default()
            });
            t.last_state = agent.agent_status.clone();
            t.last_state_change = project::now();
            let prior_session = t.identity.agent_session.clone();
            thread::bind_identity(t, &coordinator.socket, &agent, process);
            if resuming && t.identity.agent_session.is_none() {
                t.identity.agent_session = prior_session;
            }
            Ok(())
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = close_pane(ctx, project, &placed);
    }
    result
}

pub(crate) fn park_completed(ctx: &Ctx, project: &Project) -> Result<()> {
    let mut first = None;
    for record in thread::list_live(project) {
        if let Err(error) = park_one(ctx, project, &record) {
            first.get_or_insert(error.context(format!("{}: park", record.id)));
        }
    }
    first.map_or(Ok(()), Err)
}

fn park_one(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if record.parked || !parkable(project, record) {
        return Ok(());
    }
    let Some(done) = crate::events::latest_done_event(
        &crate::events::for_thread(project, &record.id),
        &record.id,
        record.attempt.max(1),
    )
    .cloned() else {
        return Ok(());
    };
    // The seal is durable before delivery. Keep its pane from sitting idle
    // while a coordinator or courier is temporarily unreachable.
    let _ = done;
    if !record.tab_id.is_empty() {
        close_pane(ctx, project, record)?;
    }
    thread::update_checked(project, &record.id, |t| {
        if t.attempt != record.attempt || !parkable(project, t) {
            bail!("park_stale: lane changed while its pane closed");
        }
        t.parked = true;
        t.last_group = Group::Parked.token().into();
        Ok(())
    })?;
    Ok(())
}

/// Cleanup observes the server that owns the lane, not the coordinator's
/// availability. A stopped local session has no panes; an unreachable running
/// session is still unknown and must not authorize removing a checkout.
fn cleanup_view<'a>(
    ctx: &'a Ctx,
    project: &Project,
    record: &Thread,
) -> Result<Option<SessionView<'a>>> {
    let socket = project.coordinator().map(|c| c.socket).unwrap_or_default();
    if record.is_remote() {
        let herdr =
            Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner).on_machine(record.machine_route());
        let agents = herdr.agent_list()?;
        let panes = herdr.pane_list()?;
        return Ok(Some(SessionView {
            herdr,
            agents,
            panes,
        }));
    }
    if let Some(view) = session_view(ctx, project) {
        return Ok(Some(view));
    }
    if crate::herdr::session_list(&ctx.env.herdr_bin(), ctx.runner)?
        .iter()
        .any(|session| session.running && session.socket_path == Path::new(&socket))
    {
        bail!(
            "cleanup_session_unreachable: the pane for {} was not closed; the running session of `{}` cannot be checked",
            record.id,
            project.slug
        );
    }
    Ok(None)
}

pub(crate) fn close_pane(ctx: &Ctx, project: &Project, record: &Thread) -> Result<bool> {
    close_pane_with_views(ctx, project, record, &mut CleanupViews::default())
}

fn close_pane_with_views(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
    views: &mut CleanupViews,
) -> Result<bool> {
    if record.tab_id.is_empty() {
        return Ok(false);
    }
    let Some(view) = views.for_thread(ctx, project, record)? else {
        return Ok(false);
    };
    let herdr = view.herdr;
    let panes = view.panes;
    let agents = view.agents;
    // The terminal may have changed cwd since placement. The three stable
    // Herdr ids still identify the exact tab this attempt created.
    let owns_pane = panes.iter().any(|pane| {
        pane.pane_id == record.pane_id
            && pane.tab_id == record.tab_id
            && pane.workspace_id == record.workspace_id
    });
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
    // ADE's durable notices own deliberate closure. The fork would otherwise
    // turn a retry, resolution or sealed park into a second GONE wake.
    herdr.pane_clear_tokens(&record.pane_id, &["parent"])?;
    let result = if owns_workspace && !holds_something_else {
        herdr.workspace_close(&record.workspace_id)
    } else {
        herdr.tab_close(&record.tab_id)
    };
    match result {
        Ok(()) => {
            views.closed(project, record);
            Ok(true)
        }
        Err(error) if matches!(error.code.as_str(), "tab_not_found" | "workspace_not_found") => {
            views.closed(project, record);
            Ok(false)
        }
        Err(error) => Err(anyhow::anyhow!("{error}")),
    }
}

/// Finds the sealed report hash and copies library deliverables. Linked files
/// and any rewritten report are preserved at resolve time. A box lane's sealed
/// artifact arrives through the Mac courier;
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

/// Preserve relative Markdown links before the worktree can be removed. The
/// seal is immutable, so the rewritten report is a separate addressed copy.
const LINKED_FILES_CAP: u64 = 200 * 1024 * 1024;

fn report_destinations(report: &str) -> Vec<(std::ops::Range<usize>, String)> {
    // Mask code without changing byte offsets: the ranges below still index the
    // original report. Fences may use either CommonMark marker and any length.
    let mut visible = report.as_bytes().to_vec();
    let mut fence: Option<(u8, usize)> = None;
    let mut offset = 0;
    for line in report.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let indent = content.bytes().take_while(|b| *b == b' ').count();
        let marker = content.as_bytes().get(indent).copied();
        let run = marker.filter(|m| matches!(m, b'`' | b'~')).map_or(0, |m| {
            content.as_bytes()[indent..]
                .iter()
                .take_while(|b| **b == m)
                .count()
        });
        let rest = &content[indent + run..];
        let close = fence.is_some_and(|(m, n)| {
            indent <= 3 && marker == Some(m) && run >= n && rest.trim().is_empty()
        });
        let open = fence.is_none()
            && indent <= 3
            && matches!(marker, Some(b'`' | b'~'))
            && run >= 3
            && (marker != Some(b'`') || !rest.contains('`'));
        if fence.is_some() || open {
            visible[offset..offset + line.len()].fill(b' ');
        }
        if close {
            fence = None;
        } else if open {
            fence = Some((marker.unwrap(), run));
        }
        offset += line.len();
    }
    let mut i = 0;
    while i < visible.len() {
        if visible[i] == b'`' {
            let start = i;
            while i < visible.len() && visible[i] == b'`' {
                i += 1;
            }
            let count = i - start;
            let mut end = i;
            while end < visible.len() {
                if visible[end] == b'`' {
                    let mut next = end;
                    while next < visible.len() && visible[next] == b'`' {
                        next += 1;
                    }
                    if next - end == count {
                        visible[start..next].fill(b' ');
                        i = next;
                        break;
                    }
                    end = next;
                } else {
                    end += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    let bytes = &visible;
    let mut found = Vec::new();
    let mut i = 0;
    while i + 2 < bytes.len() {
        // srcset is a list of image URLs, each with an optional descriptor.
        // Treat every URL as a link, not the entire attribute as one path.
        if bytes[i..].starts_with(b"srcset=") {
            let mut start = i + b"srcset=".len();
            let quote = match bytes.get(start) {
                Some(b'\'' | b'"') => {
                    let quote = bytes[start];
                    start += 1;
                    Some(quote)
                }
                _ => None,
            };
            let mut end = start;
            while end < bytes.len()
                && bytes[end] != b'\n'
                && bytes[end] != b'\r'
                && quote.is_none_or(|q| bytes[end] != q)
                && (quote.is_some() || !matches!(bytes[end], b' ' | b'\t' | b'>'))
            {
                end += 1;
            }
            let mut pos = start;
            while pos < end {
                while pos < end && matches!(bytes[pos], b' ' | b'\t' | b',') {
                    pos += 1;
                }
                let url_start = pos;
                let data_url = bytes[pos..end].starts_with(b"data:");
                while pos < end
                    && !matches!(bytes[pos], b' ' | b'\t')
                    && (data_url || bytes[pos] != b',')
                {
                    pos += 1;
                }
                if pos > url_start
                    && report.is_char_boundary(url_start)
                    && report.is_char_boundary(pos)
                {
                    found.push((url_start..pos, report[url_start..pos].to_string()));
                }
                // Ignore the resolution descriptor, then find the next URL.
                while pos < end && bytes[pos] != b',' {
                    pos += 1;
                }
            }
            i = end;
            continue;
        }
        // Inline links/images and reference definitions: [text](path),
        // ![alt](path), and [label]: path. Preserve titles and fragments.
        let html = [b"src=".as_slice(), b"href=".as_slice()]
            .into_iter()
            .find(|attr| bytes[i..].starts_with(attr));
        let start = if let Some(attr) = html {
            Some(i + attr.len())
        } else if bytes[i] == b']' && bytes[i + 1] == b'(' {
            Some(i + 2)
        } else if bytes[i] == b']' && bytes[i + 1] == b':' {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            Some(j)
        } else {
            None
        };
        if let Some(mut start) = start {
            let quote = html.and_then(|_| match bytes.get(start) {
                Some(b'\'' | b'"') => Some(bytes[start]),
                _ => None,
            });
            if quote.is_some() {
                start += 1;
            }
            let angle = bytes.get(start) == Some(&b'<');
            if angle {
                start += 1;
            }
            let mut end = start;
            let mut depth = 0_u32;
            while end < bytes.len() {
                let b = bytes[end];
                if quote == Some(b)
                    || matches!(b, b'\n' | b'\r')
                    || (quote.is_none() && !angle && matches!(b, b' ' | b'\t'))
                    || (html.is_some() && quote.is_none() && b == b'>')
                    || (angle && b == b'>')
                {
                    break;
                }
                if quote.is_none() && !angle && b == b')' {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                } else if quote.is_none() && !angle && b == b'(' {
                    depth += 1;
                }
                end += 1;
            }
            if end > start && report.is_char_boundary(start) && report.is_char_boundary(end) {
                found.push((start..end, report[start..end].to_string()));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    found
}

#[derive(Debug)]
enum ReportLink {
    Thread(std::path::PathBuf),
    Repo {
        relative: std::path::PathBuf,
        source: std::path::PathBuf,
    },
}

fn linked_relative_path(
    project: &Project,
    record: &Thread,
    dest: &str,
) -> Result<Option<ReportLink>> {
    let raw = dest.split(['#', '?']).next().unwrap_or(dest);
    if raw.is_empty()
        || raw.contains("://")
        || raw.starts_with("data:")
        || raw.starts_with("mailto:")
    {
        return Ok(None);
    }
    let prefix = format!(".herdr-project/{}-{}/", project.slug, record.id);
    let raw = raw.strip_prefix(&prefix).unwrap_or(raw);
    let mut decoded = Vec::new();
    let mut index = 0;
    let bytes = raw.as_bytes();
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            )
        {
            decoded.push((high * 16 + low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).unwrap_or_else(|_| raw.to_string());
    let root = std::path::Path::new(&record.thread_dir);
    let source = root.join(&decoded);
    let mut path = root.to_path_buf();
    for component in std::path::Path::new(&decoded).components() {
        match component {
            std::path::Component::Normal(part) => path.push(part),
            std::path::Component::CurDir => (),
            std::path::Component::ParentDir => {
                if !path.pop() {
                    bail!("linked path is not inside the worktree: {dest}");
                }
            }
            std::path::Component::RootDir => path = std::path::PathBuf::from("/"),
            _ => bail!("linked path is not inside the worktree: {dest}"),
        }
    }
    if let Ok(relative) = path.strip_prefix(root) {
        return Ok(
            (!relative.as_os_str().is_empty()).then(|| ReportLink::Thread(relative.to_path_buf()))
        );
    }
    if !record.worktree_path.is_empty()
        && let Ok(relative) = path.strip_prefix(&record.worktree_path)
        && !relative.as_os_str().is_empty()
    {
        return Ok(Some(ReportLink::Repo {
            relative: relative.to_path_buf(),
            source,
        }));
    }
    bail!("linked path is not inside the worktree: {dest}")
}

fn linked_file_missing(ctx: &Ctx, record: &Thread, relative: &std::path::Path) -> Result<bool> {
    let path = std::path::Path::new(&record.thread_dir).join(relative);
    if !record.is_remote() {
        return match std::fs::symlink_metadata(&path) {
            Ok(_) => Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error.into()),
        };
    }
    let profile = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?;
    let script = format!(
        "if test -e {file} || test -L {file}; then printf 'present'; else printf 'missing'; fi",
        file = remote::quote(&path.to_string_lossy())
    );
    let out = remote::ssh(
        ctx.runner,
        &profile.target,
        &script,
        None,
        std::time::Duration::from_secs(20),
    )?;
    if !out.success() {
        bail!(
            "could not inspect linked file {}: {}",
            path.display(),
            out.error_text()
        );
    }
    match out.stdout.trim() {
        "missing" => Ok(true),
        "present" => Ok(false),
        _ => bail!(
            "linked file probe returned no presence answer: {}",
            path.display()
        ),
    }
}

struct LinkedFiles {
    directory: bool,
    files: std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
}

fn markdown_label(name: &str) -> String {
    name.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(['\n', '\r'], " ")
}

fn linked_bytes(ctx: &Ctx, record: &Thread, relative: &std::path::Path) -> Result<Vec<u8>> {
    let mut linked = linked_files(ctx, record, relative)?;
    if linked.directory {
        bail!("report is not a regular file: {}", relative.display());
    }
    linked.files.remove(relative).context("linked file missing")
}

// The box walks and hashes the entire link in one probe. Payload requests
// slice the concatenated files, so even thousands of small images need at
// most 25 more round trips (200 MiB / 8 MiB), never one trip per file.
const LINKED_BOX_SCRIPT: &str = r#"
import os, sys, stat, json, hashlib, subprocess
root = os.path.realpath(sys.argv[1])
path = os.path.join(root, sys.argv[2])
def checked(path):
    info = os.lstat(path)
    canonical = os.path.realpath(path)
    if os.path.commonpath([root, canonical]) != root:
        sys.exit(3)
    part = path
    while part != root:
        if os.path.islink(part):
            sys.exit(4)
        parent = os.path.dirname(part)
        if parent == part:
            sys.exit(3)
        part = parent
    if not (stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):
        sys.exit(4)
    return info
try:
    os.stat(root)
    directory = stat.S_ISDIR(checked(path).st_mode)
    if len(sys.argv) == 4 and sys.argv[3] == 'repo':
        if directory:
            sys.exit(4)
        name = os.path.relpath(path, root)
        def git(*args):
            return subprocess.run(['git', '-C', root, *args], check=True, stdout=subprocess.PIPE).stdout
        spec = ':(literal)' + name
        if not git('ls-files', '-z', '--', spec):
            sys.exit(6)
        if git('status', '--porcelain', '-z', '--untracked-files=all', '--', spec):
            sys.exit(7)
        sys.stdout.write(git('hash-object', '--no-filters', '--', path).decode())
        sys.exit(0)
    entries = []
    pending = [path]
    total = 0
    while pending:
        item = pending.pop()
        info = checked(item)
        if stat.S_ISDIR(info.st_mode):
            with os.scandir(item) as children:
                pending.extend(child.path for child in children)
        else:
            total += info.st_size
            if total > 209715200:
                sys.exit(5)
            entries.append((os.path.relpath(item, root), info.st_size))
    entries.sort()
    if len(sys.argv) == 3:
        manifest = []
        for name, size in entries:
            with open(os.path.join(root, name), 'rb') as source:
                digest = hashlib.sha256()
                while data := source.read(1048576):
                    digest.update(data)
                digest = digest.hexdigest()
            manifest.append({'path': name, 'size': size, 'hash': digest})
        print(json.dumps({'directory': directory, 'files': manifest}))
    else:
        offset = int(sys.argv[3])
        remaining = 8388608
        for name, size in entries:
            if offset >= size:
                offset -= size
                continue
            with open(os.path.join(root, name), 'rb') as source:
                source.seek(offset)
                data = source.read(min(size - offset, remaining))
            sys.stdout.write(data.hex())
            remaining -= len(data)
            offset = 0
            if remaining == 0:
                break
except FileNotFoundError:
    sys.exit(2)
"#;

fn linked_probe_result(out: &crate::runner::Output, path: &std::path::Path) -> Result<()> {
    if out.success() {
        return Ok(());
    }
    let reason = if out.timed_out {
        "timed out".to_string()
    } else {
        match out.code {
            Some(2) => "missing".into(),
            Some(3) => "not inside the thread folder".into(),
            Some(4) => "not a regular file or folder (symlinks are not kept)".into(),
            Some(5) => "over cap (200 MiB)".into(),
            _ => format!("probe failed: {}", out.error_text()),
        }
    };
    bail!("linked path {reason}: {}; worktree kept", path.display());
}

fn linked_files(ctx: &Ctx, record: &Thread, relative: &std::path::Path) -> Result<LinkedFiles> {
    let root = std::path::Path::new(&record.thread_dir);
    let path = root.join(relative);
    if !record.is_remote() {
        let canonical_root = root.canonicalize()?;
        let mut pending = vec![path.clone()];
        let mut paths = Vec::new();
        let mut total = 0_u64;
        let mut directory = false;
        while let Some(item) = pending.pop() {
            let info = std::fs::symlink_metadata(&item).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    anyhow::anyhow!("linked path missing: {}", item.display())
                } else {
                    anyhow::anyhow!("could not inspect linked path {}: {error}", item.display())
                }
            })?;
            let canonical = match std::fs::canonicalize(&item) {
                Ok(canonical) => canonical,
                Err(_) if info.file_type().is_symlink() => {
                    bail!(
                        "linked path is not a regular file or folder (symlink): {}",
                        item.display()
                    );
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("could not resolve linked path {}", item.display())
                    });
                }
            };
            if !canonical.starts_with(&canonical_root) {
                bail!(
                    "linked path is not inside the thread folder: {}",
                    item.display()
                );
            }
            for ancestor in item.ancestors().take_while(|p| *p != root) {
                if std::fs::symlink_metadata(ancestor)?
                    .file_type()
                    .is_symlink()
                {
                    bail!(
                        "linked path is not a regular file or folder (symlink): {}",
                        ancestor.display()
                    );
                }
            }
            if info.is_dir() {
                directory |= item == path;
                for child in std::fs::read_dir(&item)? {
                    pending.push(child?.path());
                }
            } else if info.is_file() {
                total = total
                    .checked_add(info.len())
                    .context("linked files size overflow")?;
                if total > LINKED_FILES_CAP {
                    bail!("linked files over cap (200 MiB); worktree kept");
                }
                paths.push(item);
            } else {
                bail!(
                    "linked path is not a regular file or folder: {}",
                    item.display()
                );
            }
        }
        let mut files = std::collections::BTreeMap::new();
        let mut read_total = 0_u64;
        for item in paths {
            // Bound reads too, in case a file grows after inspection.
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(&item)?
                .take(LINKED_FILES_CAP - read_total + 1)
                .read_to_end(&mut bytes)?;
            read_total += bytes.len() as u64;
            if read_total > LINKED_FILES_CAP {
                bail!("linked files over cap (200 MiB); worktree kept");
            }
            files.insert(item.strip_prefix(root)?.to_path_buf(), bytes);
        }
        return Ok(LinkedFiles { directory, files });
    }
    #[derive(serde::Deserialize)]
    struct Manifest {
        directory: bool,
        files: Vec<ManifestFile>,
    }
    #[derive(serde::Deserialize)]
    struct ManifestFile {
        path: std::path::PathBuf,
        size: u64,
        hash: String,
    }
    let profile = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?;
    let script = format!(
        "python3 -c {} {} {}",
        remote::quote(LINKED_BOX_SCRIPT),
        remote::quote(&record.thread_dir),
        remote::quote(&relative.to_string_lossy())
    );
    let probe = |script: &str| -> Result<String> {
        let out = remote::ssh(
            ctx.runner,
            &profile.target,
            script,
            None,
            std::time::Duration::from_secs(90),
        )?;
        linked_probe_result(&out, &path)?;
        Ok(out.stdout)
    };
    let manifest: Manifest = serde_json::from_str(&probe(&script)?)?;
    let mut total = 0_u64;
    for file in &manifest.files {
        if file
            .path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
            || !file.path.starts_with(relative)
        {
            bail!(
                "linked path is not inside the thread folder: {}",
                file.path.display()
            );
        }
        total = total
            .checked_add(file.size)
            .context("linked files size overflow")?;
        if total > LINKED_FILES_CAP {
            bail!("linked files over cap (200 MiB); worktree kept");
        }
    }
    let mut bytes = Vec::new();
    for offset in (0..total).step_by(8 * 1024 * 1024) {
        let hex = probe(&format!("{script} {offset}"))?;
        let hex = hex.trim();
        if !hex.is_ascii() || hex.len() % 2 != 0 {
            bail!("invalid linked file payload");
        }
        for i in (0..hex.len()).step_by(2) {
            bytes.push(u8::from_str_radix(&hex[i..i + 2], 16)?);
        }
        if bytes.len() as u64 != (offset + 8 * 1024 * 1024).min(total) {
            bail!(
                "remote linked files changed during copy: {}",
                path.display()
            );
        }
    }
    let mut files = std::collections::BTreeMap::new();
    let mut offset = 0;
    for file in manifest.files {
        let end = offset + file.size as usize;
        let content = bytes[offset..end].to_vec();
        if thread::sha256_hex(&content) != file.hash
            || files.insert(file.path.clone(), content).is_some()
        {
            bail!(
                "remote linked file changed during copy: {}",
                file.path.display()
            );
        }
        offset = end;
    }
    Ok(LinkedFiles {
        directory: manifest.directory,
        files,
    })
}

/// Repo links are kept only by integration, never copied into ADE artifacts.
fn repo_link_kept(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
    relative: &std::path::Path,
    path: &std::path::Path,
) -> Result<()> {
    let root = std::path::Path::new(&record.worktree_path);
    let spec = format!(":(literal){}", relative.to_string_lossy());
    let hash = if record.is_remote() {
        let profile = remote::machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            record.machine_route(),
        )?;
        let script = format!(
            "python3 -c {} {} {} repo",
            remote::quote(LINKED_BOX_SCRIPT),
            remote::quote(&record.worktree_path),
            remote::quote(&path.to_string_lossy())
        );
        let out = remote::ssh(
            ctx.runner,
            &profile.target,
            &script,
            None,
            Duration::from_secs(90),
        )?;
        if !out.success() {
            let reason = match out.code.filter(|_| !out.timed_out) {
                Some(3) => "not inside the worktree",
                Some(6) => "untracked repo file",
                Some(7) => "uncommitted repo file",
                _ => {
                    linked_probe_result(&out, path)?;
                    unreachable!()
                }
            };
            bail!("linked path {reason}: {}; worktree kept", path.display());
        }
        out.stdout.trim().to_string()
    } else {
        let canonical_root = root.canonicalize()?;
        let canonical = path
            .canonicalize()
            .with_context(|| format!("linked path missing: {}", path.display()))?;
        if !canonical.starts_with(&canonical_root) {
            bail!(
                "linked path is not inside the worktree: {}; worktree kept",
                path.display()
            );
        }
        for ancestor in path.ancestors().take_while(|p| *p != root) {
            if std::fs::symlink_metadata(ancestor)?
                .file_type()
                .is_symlink()
            {
                bail!(
                    "linked path is not a regular file (symlink): {}; worktree kept",
                    path.display()
                );
            }
        }
        if !std::fs::metadata(path)?.is_file() {
            bail!(
                "linked path is not a regular repo file: {}; worktree kept",
                path.display()
            );
        }
        if git(
            ctx.runner,
            &record.worktree_path,
            &["ls-files", "-z", "--", &spec],
            GIT_TIMEOUT,
        )?
        .is_empty()
        {
            bail!(
                "linked path is an untracked repo file: {}; worktree kept",
                path.display()
            );
        }
        if !git(
            ctx.runner,
            &record.worktree_path,
            &[
                "status",
                "--porcelain",
                "-z",
                "--untracked-files=all",
                "--",
                &spec,
            ],
            GIT_TIMEOUT,
        )?
        .is_empty()
        {
            bail!(
                "linked path is an uncommitted repo file: {}; worktree kept",
                path.display()
            );
        }
        git(
            ctx.runner,
            &record.worktree_path,
            &["hash-object", "--no-filters", "--", &path.to_string_lossy()],
            GIT_TIMEOUT,
        )?
    };
    // After placement `base` is the frozen start SHA, not a branch name.
    // Honor the configured integration branch even if another is checked out.
    let rows = project.read_project_md()?.0.repos;
    let integration = rows
        .into_iter()
        .chain(crate::harness::repos(&ctx.config_dir)?)
        .find(|row| row.path == record.repo)
        .and_then(|row| row.branch)
        .map(Ok)
        .unwrap_or_else(|| crate::git::symbolic_head(ctx.runner, &record.repo))?;
    let head = crate::git::rev_parse(
        ctx.runner,
        &record.repo,
        &format!("refs/heads/{integration}"),
    )?;
    let blob = git(
        ctx.runner,
        &record.repo,
        &[
            "ls-tree",
            "--format=%(objecttype) %(objectname)",
            &head,
            "--",
            &spec,
        ],
        GIT_TIMEOUT,
    )?;
    if blob != format!("blob {hash}") {
        bail!(
            "linked repo file content is not committed on integration branch `{integration}`: {}; worktree kept",
            relative.display()
        );
    }
    Ok(())
}

fn draft_has_existing_links(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
    text: &str,
) -> Result<bool> {
    let mut missing = Vec::new();
    let mut existing = false;
    for (_, dest) in report_destinations(text) {
        match linked_relative_path(project, record, &dest)? {
            Some(ReportLink::Thread(relative)) => {
                if linked_file_missing(ctx, record, &relative)? {
                    missing.push(dest);
                } else {
                    existing = true;
                }
            }
            Some(ReportLink::Repo { relative, source }) => {
                repo_link_kept(ctx, project, record, &relative, &source)?;
            }
            None => (),
        }
    }
    thread::update(project, &record.id, |t| {
        t.missing_report_links = missing.clone()
    })?;
    if !missing.is_empty() {
        bail!(
            "linked paths missing: {}; worktree kept",
            missing.join(", ")
        );
    }
    Ok(existing)
}

fn preserve_report_links(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if thread::sealed_report_path(project, record).is_none() {
        if crate::events::for_thread(project, &record.id)
            .into_iter()
            .any(|event| {
                event.thread == record.id
                    && event.attempt == record.attempt.max(1)
                    && event.payload.done.is_some()
            })
        {
            bail!("the sealed report artifact is missing or damaged; worktree kept");
        }
        if record.is_remote() {
            let profile = remote::machine_profile(
                ctx.runner,
                &ctx.env.herdr_bin(),
                &ctx.config_dir,
                record.machine_route(),
            )?;
            let root = remote::quote(&record.thread_dir);
            let draft = remote::quote(&format!("{}/report.md", record.thread_dir));
            let script = format!(
                "root=$(realpath -e -- {root}) || exit 2; test -r \"$root\" && test -x \"$root\" || exit 3; if test -e {draft} || test -L {draft}; then printf '__HERDR_DRAFT_PRESENT__\\n'; else printf '__HERDR_DRAFT_ABSENT__\\n'; fi"
            );
            let out = remote::ssh(
                ctx.runner,
                &profile.target,
                &script,
                None,
                std::time::Duration::from_secs(20),
            )?;
            if !out.success() {
                bail!(
                    "could not inspect box report: {}; worktree kept",
                    out.error_text()
                );
            }
            match out.stdout.trim() {
                "__HERDR_DRAFT_ABSENT__" => return Ok(()),
                "__HERDR_DRAFT_PRESENT__" => {
                    let bytes = linked_bytes(ctx, record, std::path::Path::new("report.md"))?;
                    let text = String::from_utf8(bytes)?;
                    if draft_has_existing_links(ctx, project, record, &text)? {
                        bail!(
                            "the box report links to files but has no sealed artifact; worktree kept"
                        );
                    }
                    return Ok(());
                }
                _ => bail!("box report probe returned no presence answer; worktree kept"),
            }
        }
        let draft = std::path::Path::new(&record.thread_dir).join("report.md");
        match std::fs::read_to_string(&draft) {
            Ok(text) => {
                if draft_has_existing_links(ctx, project, record, &text)? {
                    bail!("the report links to files but has no sealed artifact; worktree kept");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => bail!(
                "could not inspect {}: {error}; worktree kept",
                draft.display()
            ),
        }
        return Ok(());
    }
    // Read the seal, not an earlier rewritten report, for repeatable retries.
    let events = crate::events::for_thread(project, &record.id);
    let sealed = crate::events::latest_done_event(&events, &record.id, record.attempt.max(1))
        .and_then(|event| event.payload.done.as_ref())
        .map(|done| done.artifact.clone())
        .context("sealed report event is missing")?;
    let bytes = std::fs::read(crate::events::artifact_path(project, &sealed))?;
    if thread::sha256_hex(&bytes) != sealed {
        bail!("sealed report artifact is damaged: {sealed}");
    }
    let text = String::from_utf8(bytes)?;
    let destinations = report_destinations(&text);
    let mut replacements = Vec::new();
    let mut files = std::collections::BTreeMap::new();
    let mut total = 0_u64;
    let mut missing = Vec::new();
    for (range, dest) in destinations {
        let relative = match linked_relative_path(project, record, &dest)? {
            Some(ReportLink::Thread(relative)) => relative,
            Some(ReportLink::Repo { relative, source }) => {
                // Git keeps this content on integration; leave the link as written.
                repo_link_kept(ctx, project, record, &relative, &source)?;
                continue;
            }
            None => continue,
        };
        let linked = linked_files(ctx, record, &relative).inspect_err(|_| {
            // Keep the missing-link record useful, without letting a missing
            // deliverable permit removal of its worktree.
            if linked_file_missing(ctx, record, &relative).unwrap_or(false) {
                missing.push(dest.clone());
                let _ = thread::update(project, &record.id, |t| {
                    t.missing_report_links = missing.clone();
                });
            }
        })?;
        let hash = if linked.directory {
            // A directory link becomes an addressed index: each child still
            // uses the same content-addressed storage as a single-file link.
            let mut index = String::from("# Linked folder\n\n");
            for (path, bytes) in &linked.files {
                let name = path.strip_prefix(&relative)?.to_string_lossy();
                index.push_str(&format!(
                    "- [{}]({})\n",
                    markdown_label(&name),
                    thread::sha256_hex(bytes)
                ));
            }
            thread::store_artifact(project, index.as_bytes())?
        } else {
            thread::sha256_hex(&linked.files[&relative])
        };
        for (path, bytes) in linked.files {
            if let std::collections::btree_map::Entry::Vacant(entry) = files.entry(path) {
                total = total
                    .checked_add(bytes.len() as u64)
                    .context("linked files size overflow")?;
                if total > LINKED_FILES_CAP {
                    bail!("linked files over cap (200 MiB); worktree kept");
                }
                entry.insert(bytes);
            }
        }
        let suffix = &dest[dest.split(['#', '?']).next().unwrap_or(&dest).len()..];
        replacements.push((range, format!("{hash}{suffix}")));
    }
    thread::update(project, &record.id, |t| {
        t.missing_report_links = missing.clone()
    })?;
    for (relative, bytes) in files {
        thread::store_artifact(project, &bytes)
            .with_context(|| format!("could not preserve {}", relative.display()))?;
    }
    if replacements.is_empty() {
        return Ok(());
    }
    let mut rewritten = text;
    for (range, dest) in replacements.into_iter().rev() {
        rewritten.replace_range(range, &dest);
    }
    let hash = thread::store_artifact(project, rewritten.as_bytes())?;
    thread::update(project, &record.id, |t| {
        t.final_report_hash = hash.clone();
        t.final_report_seal = sealed.clone();
    })?;
    Ok(())
}

/// A box lane's report arrives as the courier's imported artifact
/// (SPEC-remote §4.3). Once the current attempt has a sealed `done` whose
/// artifact is on the Mac and hashes to its name, the report copy is complete.
/// Linked files are transported separately before worktree removal. The D4
/// removal gate reads the sealed artifact.
fn imported_report(project: &Project, record: &Thread) -> thread::Copied {
    let attempt = record.attempt.max(1);
    let events = crate::events::for_thread(project, &record.id);
    let hash = crate::events::latest_done_event(&events, &record.id, attempt)
        .and_then(|event| event.payload.done.as_ref())
        .map(|done| done.artifact.clone());
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
    finished_worktree_reason_with_merged(ctx, project, record, None)
}

/// Doctor preloads the merged local branch names once per repository. Explicit
/// removal still takes the individual SHA/ancestry path above and rechecks it.
pub(crate) fn finished_worktree_reason_with_merged(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
    merged: Option<&BTreeSet<String>>,
) -> Result<Option<String>> {
    if managed_git_folder(project, record) {
        let done = crate::events::for_thread(project, &record.id)
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
    if !record.merged_sha.is_empty() || crate::review::lane_review(project, record)?.is_some() {
        return Ok(None);
    }
    if record.role == "reviewer"
        && crate::review::list(project)?.iter().any(|r| {
            r.reviewer.as_deref() == Some(&record.id) && (r.phase.closed() || r.fast_forward)
        })
    {
        return Ok(None);
    }

    if record.branch.is_empty() {
        return Ok(Some(
            "work_not_done: no lane branch is recorded and no landed review contains the thread"
                .into(),
        ));
    }
    if merged.is_some_and(|branches| branches.contains(&record.branch)) {
        return Ok(None);
    }
    let Some(lane_head) = crate::git::branch_head(ctx.runner, &record.repo, &record.branch)? else {
        return Ok(Some(format!(
            "work_not_done: branch `{}` is missing and no landed review contains the thread",
            record.branch
        )));
    };
    if !record.base.is_empty()
        && !crate::repo::Git::new(ctx.runner, &record.repo)
            .trees_differ(&record.base, &lane_head)?
    {
        return Ok(None);
    }
    if merged.is_some() {
        return Ok(Some(format!(
            "work_not_done: branch `{}` is not merged into the integration branch",
            record.branch
        )));
    }
    let integration = crate::git::symbolic_head(ctx.runner, &record.repo)?;
    let integration_head = crate::git::branch_head(ctx.runner, &record.repo, &integration)?
        .with_context(|| format!("integration branch `{integration}` is missing"))?;
    if crate::git::is_ancestor(ctx.runner, &record.repo, &lane_head, &integration_head)? {
        Ok(None)
    } else {
        Ok(Some(format!(
            "work_not_done: branch `{}` is not on integration branch `{integration}` and no landed review contains the thread",
            record.branch
        )))
    }
}

pub(crate) fn report_artifact_stored(project: &Project, record: &Thread) -> Result<bool> {
    let attempt = record.attempt.max(1);
    for event in crate::events::checked_for_thread(project, &record.id)?
        .into_iter()
        .filter(|event| event.attempt == attempt)
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

/// Explicitly discard only ignored data after safety checks, then retire the ref.
pub(crate) fn remove_kept_worktree(ctx: &Ctx, slug: &str, id: &str) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if record.status != Status::Resolved || record.worktree_path.is_empty() {
        bail!("{id} has no resolved, retained worktree");
    }
    if !removable_folder(&project, &record) || !worktree_exists(ctx, &project, &record)? {
        bail!("{id} has no removable worktree at {}", record.worktree_path);
    }
    if let Some(reason) = finished_worktree_reason(ctx, &project, &record)? {
        bail!("{id} is not finished; not removing its worktree: {reason}");
    }
    preserve_report_links(ctx, &project, &record)
        .with_context(|| format!("linked_files_not_kept: cannot discard {id}'s worktree"))?;
    let inspection = inspect_worktree_for_removal(ctx, &project, &record)?;
    if !inspection.dirty.is_empty() {
        bail!("worktree_dirty: {}", inspection.dirty.join(", "));
    }
    // Pin the checked-out branch tip before discarding the checkout. A retained
    // box branch must also match its published tip or verified immutable seal.
    let tip = crate::branches::require_published_tip(ctx, &project, &record)?;
    let mut views = CleanupViews::default();
    removal_in_use_gate(ctx, &project, &record, &mut views)?;
    close_pane_with_views(ctx, &project, &record, &mut views)?;
    // The marker makes ref retirement retryable even if the process dies
    // between removing the checkout and deleting the branch.
    let pinned = thread::update(&project, id, |t| {
        t.cleanup_pending = true;
        t.cleanup_reason = format!("retained worktree removal: {tip}");
    })?;
    remove_worktree_force_ignored(ctx, &project, &record)?;
    thread::update(&project, id, |t| t.worktree_path.clear())?;
    crate::branches::resolved_thread(ctx, &project, &pinned)?;
    thread::update(&project, id, |t| {
        t.cleanup_pending = false;
        t.cleanup_reason.clear();
    })?;
    Ok(format!(
        "removed {} and branch {}",
        record.worktree_path, record.branch
    ))
}

fn remove_worktree_force_ignored(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if managed_git_folder(project, record) {
        return remove_worktree(ctx, project, record);
    }
    let repo = if record.is_remote() {
        let (settings, _) = project.read_project_md()?;
        let profile = remote::machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            record.machine_route(),
        )?;
        box_repo_row(&ctx.config_dir, &settings, &profile.label, &record.repo)?.0
    } else {
        record.repo.clone()
    };
    if !record.is_remote() {
        let out = ctx.runner.run(
            &crate::runner::Cmd::new("git", Duration::from_secs(30)).args([
                "-C",
                &repo,
                "worktree",
                "remove",
                "--force",
                &record.worktree_path,
            ]),
        )?;
        if !out.success() {
            bail!("{}", out.error_text());
        }
    } else {
        let profile = remote::machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            record.machine_route(),
        )?;
        let machine = remote::machine_declaration(&ctx.config_dir, &profile.label)?;
        let script = remote::with_path(
            &machine.path,
            &format!(
                "cd {} && git worktree remove --force {}",
                remote::quote(&repo),
                remote::quote(&record.worktree_path)
            ),
        );
        let out = remote::ssh(
            ctx.runner,
            &profile.target,
            &script,
            None,
            Duration::from_secs(40),
        )?;
        if !out.success() {
            bail!("{}", out.error_text());
        }
    }
    Ok(())
}

/// Never forces. Git's refusal is reported unchanged.
pub(crate) fn remove_worktree(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
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
fn removal_in_use_gate(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
    views: &mut CleanupViews,
) -> Result<()> {
    let Some(view) = views.for_thread(ctx, project, record)? else {
        return Ok(());
    };
    let agents = &view.agents;
    let panes = &view.panes;
    if agents.iter().any(|agent| {
        (thread::agent_matches(record, agent)
            || Path::new(&agent.cwd).starts_with(&record.worktree_path))
            && !(thread::agent_matches(record, agent) && agent.ready())
    }) {
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

    let herdr = view.herdr;
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

/// Remove historical push links from verified box lanes after a rebind.
pub(crate) fn clear_push_links(ctx: &Ctx, project: &Project) -> Result<()> {
    let view = require_session(ctx, project)?;
    let mut observed = std::collections::BTreeMap::new();
    for lane in thread::list_live(project) {
        // The ordinary lineage pass below repairs local lanes using the same
        // observation; this binding pass handles box lanes only.
        if !lane.is_remote()
            || lane.status == Status::Resolved
            || lane.parked
            || lane.pane_id.is_empty()
        {
            continue;
        }
        let machine = lane.machine_route().to_string();
        if !observed.contains_key(&machine) {
            let herdr = view.herdr.on_machine(&machine);
            let agents = herdr.agent_list().map_err(|e| anyhow::anyhow!("{e}"))?;
            let panes = herdr.pane_list().map_err(|e| anyhow::anyhow!("{e}"))?;
            observed.insert(machine.clone(), (agents, panes));
        }
        let (agents, panes) = &observed[&machine];
        let Some(agent) = agents.iter().find(|a| thread::agent_matches(&lane, a)) else {
            continue;
        };
        if !panes.iter().any(|p| thread::pane_matches(&lane, p)) {
            continue;
        }
        let herdr = view.herdr.on_machine(&machine);
        let live = herdr
            .pane_process_info(&lane.pane_id)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .identities();
        if !thread::identity_verifies(&lane, agent, &live) {
            continue;
        }
        if agent.parent().is_some() {
            herdr.pane_clear_tokens(&lane.pane_id, &["parent"])?;
        }
    }
    Ok(())
}

/// Lineage repair (SPEC-ADE D3); lineage is local.
pub fn tick(project: &Project, herdr: &Herdr, agents: &[Agent]) -> Result<()> {
    if project.coordinator().is_none() {
        return Ok(());
    }
    for record in thread::list_live(project) {
        if record.is_remote() || record.status == Status::Resolved {
            continue;
        }
        let Some(agent) = agents.iter().find(|a| thread::agent_matches(&record, a)) else {
            continue;
        };
        // An unverified pane's metadata is never changed (D3).
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
        if agent.parent().is_some() {
            herdr.pane_clear_tokens(&record.pane_id, &["parent"])?;
        }
    }
    Ok(())
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
    let view = session_view(ctx, project);
    let now = jiff::Timestamp::now();
    thread::list(project)
        .into_iter()
        .map(|t| {
            let mut result = row(&t, view.as_ref(), now);
            if result.note.starts_with("process gone:") && parkable(project, &t) {
                result.group = Group::Parked;
                result.note = "pane parked until requested".into();
            }
            result
        })
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
    if t.parked {
        return Row {
            thread: t.clone(),
            group: Group::Parked,
            note: "pane parked until requested".into(),
        };
    }
    // Placement was refused before a pane existed. Do not diagnose a missing
    // process for a start that has never launched, even with no live session.
    if t.launch_attempts == 0 && !t.provider_wait_started.is_empty() {
        let provider = crate::pi::launch::flag_value(&t.launch.args, "--provider")
            .unwrap_or_else(|| t.launch.kind.clone());
        let reason = if t.error.contains("readiness probe timed out") {
            format!("readiness probe timed out at {}", t.provider_wait_started)
        } else {
            format!(
                "readiness check failed at {}: {}",
                t.provider_wait_started, t.error
            )
        };
        return Row {
            thread: t.clone(),
            group: recorded,
            note: format!(
                "waiting for provider {provider}: {reason}; the start is queued and retries by itself"
            ),
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
    let row = rows(ctx, &project)
        .into_iter()
        .find(|row| row.thread.id == id)
        .context("thread disappeared while reading its live state")?;
    println!("group = {:?}", row.group.label());
    println!("live = {:?}", row.note);
    print!("{}", placement_summary(&record));
    print!("{}", toml::to_string(&record)?);
    for (index, follow_up) in record.follow_ups.iter().enumerate() {
        println!(
            "follow-up {} [{}] queued={} delivered={} closed={} — {}",
            index + 1,
            format!("{:?}", follow_up.state).to_lowercase(),
            follow_up.queued_at,
            follow_up.delivered_at,
            follow_up.closed_at,
            follow_up
                .text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
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

    fn linked_test_box(world: &crate::scenarios::World) {
        let config = world.home.path().join("cfg/config.toml");
        let existing = std::fs::read_to_string(&config).unwrap();
        std::fs::write(config, format!("{existing}{}", crate::remote::TEST_MACHINE)).unwrap();
        world.runner.on("machine list --json", crate::runner::fake::ok(
            r#"[{"id":"buildbox-id","label":"buildbox","target":"buildbox-pi","session":"default","enabled":true}]"#,
        ));
        // Execute only the linked-file script locally: SSH still goes through
        // the fake runner, while the real probe protocol is exercised.
        world.runner.on_fn(
            |cmd| cmd.program == "ssh" && cmd.display().contains("manifest.append"),
            |cmd| {
                let output = std::process::Command::new("sh")
                    .args(["-c", cmd.args.last().unwrap()])
                    .output()?;
                Ok(crate::runner::Output {
                    code: output.status.code(),
                    stdout: String::from_utf8(output.stdout)?,
                    stderr: String::from_utf8(output.stderr)?,
                    timed_out: false,
                })
            },
        );
    }

    fn seal_linked_report(project: &Project, lane: &Thread, report: &str) {
        let hash = thread::store_artifact(project, report.as_bytes()).unwrap();
        crate::events::seal_create_if_absent(
            project,
            &crate::contracts::Event {
                id: format!("{}-1-done", lane.id),
                op: format!("{}-1-done", lane.id),
                thread: lane.id.clone(),
                attempt: 1,
                created: project::now(),
                recipient: crate::contracts::Recipient::default(),
                payload: crate::contracts::EventPayload {
                    done: Some(crate::contracts::DonePayload {
                        has_changes: None,
                        sha: "sealed".into(),
                        report_path: lane.report_path(),
                        artifact: hash,
                        attestation: None,
                        published_ref: None,
                    }),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    }

    fn repo_link_fixture(remote: bool) -> (crate::testkit::Fx, Thread) {
        use crate::testkit::{commit_file, fixture, git};

        let fx = fixture();
        commit_file(
            &fx.repo,
            "figures/3d/gaba-dose/curves.svg",
            "<svg/>\n",
            "figure",
        );
        let (id, _) = fx.lane(1);
        let old = thread::load(&fx.project, &id).unwrap();
        let base = git(&fx.repo, &["rev-parse", "main"]);
        git(
            Path::new(&old.worktree_path),
            &["branch", "-m", "hp/demo/t-0001"],
        );
        git(&fx.repo, &["merge", "--ff-only", "hp/demo/t-0001"]);
        let merged = commit_file(&fx.repo, "other.txt", "unrelated", "integration advances");
        std::fs::write(
            fx.repo.join(".git/info/exclude"),
            ".worktrees/\n.herdr-project/\n",
        )
        .unwrap();
        if remote {
            linked_test_box(&fx.world);
            // Run the box protocol on the fixture's independent git checkout.
            fx.world.runner.on_fn(
                |cmd| cmd.program == "ssh",
                |cmd| {
                    let output = std::process::Command::new("sh")
                        .args(["-c", cmd.args.last().unwrap()])
                        .output()?;
                    Ok(crate::runner::Output {
                        code: output.status.code(),
                        stdout: String::from_utf8(output.stdout)?,
                        stderr: String::from_utf8(output.stderr)?,
                        timed_out: false,
                    })
                },
            );
            let (mut settings, body) = fx.project.read_project_md().unwrap();
            settings.repos[0].box_path = Some(fx.repo.to_string_lossy().into_owned());
            settings.repos[0].publish_url = Some(fx.repo.to_string_lossy().into_owned());
            std::fs::write(
                fx.project.project_md(),
                format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap()),
            )
            .unwrap();
        }
        let lane = thread::update(&fx.project, &id, |t| {
            t.branch = "hp/demo/t-0001".into();
            t.base = base;
            t.thread_dir = format!("{}/.herdr-project/demo-{}", t.worktree_path, t.id);
            t.status = Status::Resolved;
            t.merged_sha = merged;
            if remote {
                t.machine = "buildbox".into();
                t.machine_id = "buildbox-id".into();
            }
        })
        .unwrap();
        std::fs::create_dir_all(&lane.thread_dir).unwrap();
        (fx, lane)
    }

    #[test]
    fn integration_keeps_repo_links_for_final_copy_and_kept_removal_on_mac_and_box() {
        use crate::testkit::{commit_file, git};

        for (remote, configured) in [(false, false), (true, false), (false, true), (true, true)] {
            let (fx, lane) = repo_link_fixture(remote);
            if configured {
                git(&fx.repo, &["branch", "integration"]);
                commit_file(
                    &fx.repo,
                    "figures/3d/gaba-dose/curves.svg",
                    "different on checked-out main\n",
                    "main differs from configured integration",
                );
                let (mut settings, body) = fx.project.read_project_md().unwrap();
                settings.repos[0].branch = Some("integration".into());
                std::fs::write(
                    fx.project.project_md(),
                    format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap()),
                )
                .unwrap();
            }
            let report = "[Figure](../../figures/3d/gaba-dose/curves.svg#plot)\n";
            std::fs::write(Path::new(&lane.thread_dir).join("report.md"), report).unwrap();
            seal_linked_report(&fx.project, &lane, report);
            let ctx = fx.world.ctx();
            preserve_report_links(&ctx, &fx.project, &lane).unwrap();
            assert_eq!(
                final_copy(&ctx, &fx.project, &lane).outcome,
                CopyOutcome::Complete
            );
            let saved = thread::load(&fx.project, &lane.id).unwrap();
            assert_eq!(
                std::fs::read_to_string(thread::final_report_path(&fx.project, &saved).unwrap())
                    .unwrap(),
                report
            );
            let figure_hash = thread::sha256_hex(b"<svg/>\n");
            assert!(!crate::events::artifact_path(&fx.project, &figure_hash).exists());
            remove_kept_worktree(&ctx, "demo", &lane.id).unwrap();
            assert!(!Path::new(&lane.worktree_path).exists());
            assert!(
                thread::load(&fx.project, &lane.id)
                    .unwrap()
                    .worktree_path
                    .is_empty()
            );
        }
    }

    #[test]
    fn dirty_kept_worktree_refusal_names_first_unstaged_path_on_mac_and_box() {
        for remote in [false, true] {
            let (fx, lane) = repo_link_fixture(remote);
            seal_linked_report(&fx.project, &lane, "Done.\n");
            std::fs::write(
                Path::new(&lane.worktree_path).join("README.md"),
                "modified\n",
            )
            .unwrap();
            let error = remove_kept_worktree(&fx.world.ctx(), "demo", &lane.id).unwrap_err();
            assert_eq!(error.to_string(), "worktree_dirty: README.md", "{remote}");
            assert!(Path::new(&lane.worktree_path).exists());
        }
    }

    #[test]
    fn repo_link_refusals_keep_worktrees_on_mac_and_box() {
        use crate::testkit::{commit_file, git};
        use std::os::unix::fs::symlink;

        for remote in [false, true] {
            for case in [
                "untracked",
                "modified",
                "staged",
                "lane-only",
                "different-blob",
                "outside",
                "symlink",
                "symlink-parent",
            ] {
                let (fx, lane) = repo_link_fixture(remote);
                let root = Path::new(&lane.worktree_path);
                let figure = "figures/3d/gaba-dose/curves.svg";
                let (dest, reason) = match case {
                    "untracked" => {
                        std::fs::write(root.join("untracked.svg"), "<svg/>").unwrap();
                        ("../../untracked.svg", "untracked repo file")
                    }
                    "modified" | "staged" => {
                        std::fs::write(root.join(figure), "changed").unwrap();
                        if case == "staged" {
                            git(root, &["add", figure]);
                        }
                        (
                            "../../figures/3d/gaba-dose/curves.svg",
                            "uncommitted repo file",
                        )
                    }
                    "lane-only" => {
                        commit_file(root, "lane-only.svg", "new", "lane-only figure");
                        ("../../lane-only.svg", "not committed on integration branch")
                    }
                    "different-blob" => {
                        commit_file(root, figure, "different", "change figure only in lane");
                        (
                            "../../figures/3d/gaba-dose/curves.svg",
                            "not committed on integration branch",
                        )
                    }
                    "outside" => ("../../../outside.svg", "not inside the worktree"),
                    "symlink" => {
                        std::fs::write(fx.world.home.path().join("outside.svg"), "outside")
                            .unwrap();
                        symlink(
                            fx.world.home.path().join("outside.svg"),
                            root.join("escape.svg"),
                        )
                        .unwrap();
                        ("../../escape.svg", "not inside the worktree")
                    }
                    "symlink-parent" => {
                        let outside = fx.world.home.path().join("outside-dir");
                        std::fs::create_dir_all(outside.join("nested")).unwrap();
                        std::fs::write(outside.join("outside.svg"), "outside").unwrap();
                        symlink(outside.join("nested"), root.join("link")).unwrap();
                        ("../../link/../outside.svg", "not inside the worktree")
                    }
                    _ => unreachable!(),
                };
                let report = format!("[Figure]({dest})\n");
                seal_linked_report(&fx.project, &lane, &report);
                let error = remove_kept_worktree(&fx.world.ctx(), "demo", &lane.id).unwrap_err();
                let detail = format!("{error:#}");
                assert!(
                    detail.contains("linked_files_not_kept"),
                    "{remote}/{case}: {detail}"
                );
                assert!(detail.contains(reason), "{remote}/{case}: {detail}");
                assert!(root.exists());
                assert!(
                    !thread::load(&fx.project, &lane.id)
                        .unwrap()
                        .worktree_path
                        .is_empty()
                );
            }
        }
    }

    #[test]
    fn linked_folders_complete_final_copy_on_mac_and_box() {
        for remote in [false, true] {
            let world = crate::scenarios::World::new();
            if remote {
                linked_test_box(&world);
            }
            let project = world.project("demo", "a.sock");
            let lane = world.thread(&project, world.home.path(), |t| {
                t.kind = Kind::Tab;
                t.worktree_path.clear();
                if remote {
                    t.machine = "buildbox".into();
                    t.machine_id = "buildbox-id".into();
                }
            });
            let root = std::path::Path::new(&lane.thread_dir);
            std::fs::create_dir_all(root.join("library/nested/empty")).unwrap();
            let contents = [
                ("library/before.png", b"before".as_slice()),
                (
                    "library/nested/after [dark]\n.png",
                    b"after\0image".as_slice(),
                ),
                ("library/zero.png", b"".as_slice()),
            ];
            for (name, bytes) in contents {
                std::fs::write(root.join(name), bytes).unwrap();
            }
            let report = "Screenshots: [library](library/#shots)\n";
            std::fs::write(root.join("report.md"), report).unwrap();
            seal_linked_report(&project, &lane, report);
            world
                .runner
                .on("du -sk", crate::runner::fake::ok("4\t/library\n"));
            world.runner.on("rsync", crate::runner::fake::ok(""));
            world.runner.on(
                "rm -rf -- /home/agent/build/lanes/",
                crate::runner::fake::ok(""),
            );
            let outcome = resolve(&world.ctx(), "demo", &lane.id, &ResolveArgs::default()).unwrap();
            assert_eq!(outcome.final_copy, "complete", "{outcome:?}");
            assert!(outcome.copy_notes.is_empty(), "{outcome:?}");
            let saved = thread::load(&project, &lane.id).unwrap();
            let rewritten =
                std::fs::read_to_string(thread::final_report_path(&project, &saved).unwrap())
                    .unwrap();
            let index_hash = report_destinations(&rewritten)[0]
                .1
                .split('#')
                .next()
                .unwrap()
                .to_string();
            let index =
                std::fs::read_to_string(crate::events::artifact_path(&project, &index_hash))
                    .unwrap();
            assert_eq!(thread::sha256_hex(index.as_bytes()), index_hash);
            for (_, bytes) in contents {
                let hash = thread::sha256_hex(bytes);
                assert!(index.contains(&hash), "{index}");
                assert_eq!(
                    std::fs::read(crate::events::artifact_path(&project, &hash)).unwrap(),
                    bytes
                );
            }
            assert!(rewritten.contains("#shots"));
            if remote {
                assert_eq!(world.runner.count("manifest.append"), 2);
            }
        }
    }

    #[test]
    fn linked_path_refusals_name_the_reason_on_mac_and_box() {
        use std::os::unix::fs::symlink;
        for remote in [false, true] {
            let world = crate::scenarios::World::new();
            if remote {
                linked_test_box(&world);
            }
            let root = world.home.path().join("thread");
            std::fs::create_dir_all(root.join("library")).unwrap();
            std::fs::write(root.join("regular"), "inside").unwrap();
            std::fs::write(world.home.path().join("outside"), "outside").unwrap();
            symlink(world.home.path().join("outside"), root.join("escape")).unwrap();
            symlink(root.join("regular"), root.join("library/symlink")).unwrap();
            let _socket = std::os::unix::net::UnixListener::bind(root.join("special")).unwrap();
            let record = Thread {
                thread_dir: root.to_string_lossy().into_owned(),
                machine: if remote {
                    "buildbox".into()
                } else {
                    String::new()
                },
                machine_id: if remote {
                    "buildbox-id".into()
                } else {
                    String::new()
                },
                ..Default::default()
            };
            for (name, reason) in [
                ("missing", "missing"),
                ("escape", "not inside the thread folder"),
                ("special", "not a regular file or folder"),
                ("library", "not a regular file or folder"),
            ] {
                let error = linked_files(&world.ctx(), &record, std::path::Path::new(name))
                    .err()
                    .unwrap()
                    .to_string();
                assert!(error.contains(reason), "{remote}: {error}");
                assert!(!error.contains("()"), "{error}");
            }
            std::fs::remove_file(root.join("library/symlink")).unwrap();
            for name in ["a", "b"] {
                std::fs::File::create(root.join("library").join(name))
                    .unwrap()
                    .set_len(101 * 1024 * 1024)
                    .unwrap();
            }
            let error = linked_files(&world.ctx(), &record, std::path::Path::new("library"))
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("over cap (200 MiB)"), "{error}");
        }
    }

    #[test]
    fn box_probe_exit_codes_have_refusal_messages_without_stderr() {
        for (code, reason) in [
            (2, "missing"),
            (3, "not inside the thread folder"),
            (4, "not a regular file or folder"),
            (5, "over cap"),
        ] {
            let error = linked_probe_result(
                &crate::runner::fake::fail(code, ""),
                std::path::Path::new("library"),
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(reason), "{error}");
            assert!(!error.contains("()"));
        }
    }

    #[test]
    fn relative_report_link_cannot_escape_the_worktree() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |_| {});
        for dest in [
            "../../../outside",
            "%2e%2e/%2e%2e/%2e%2e/outside",
            "%2foutside",
        ] {
            assert!(
                linked_relative_path(&project, &lane, dest)
                    .unwrap_err()
                    .to_string()
                    .contains("not inside the worktree")
            );
        }
    }

    #[test]
    fn rebind_accepts_unnamed_agent_but_not_a_different_name() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Failed;
            t.worktree_path = "/work/lane".into();
            t.cwd = t.worktree_path.clone();
            t.agent_name = "recorded".into();
        })
        .unwrap();
        *world.agents.borrow_mut() = r#"[{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","cwd":"/work/lane","agent":"claude","agent_status":"working","name":"different"}]"#.into();
        assert!(
            format!(
                "{:#}",
                rebind(&world.ctx(), "demo", &lane.id, "w1:p2").unwrap_err()
            )
            .contains("rebind_identity_mismatch")
        );
        let unnamed = world.agents.borrow().replace("different", "");
        *world.agents.borrow_mut() = unnamed;
        rebind(&world.ctx(), "demo", &lane.id, "w1:p2").unwrap();
        let saved = thread::load(&project, &lane.id).unwrap();
        assert_eq!(saved.status, Status::Open);
        assert_eq!(saved.agent_name, "");
    }

    #[test]
    fn rebind_clears_push_links_on_both_machines() {
        for machine in ["", "buildbox"] {
            let world = crate::scenarios::World::new();
            let project = world.project("demo", "a.sock");
            let lane = thread::allocate(&project, |t| {
                t.status = Status::Failed;
                t.machine = machine.into();
                t.cwd = "/work/lane".into();
            })
            .unwrap();
            *world.agents.borrow_mut() = r#"[{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","cwd":"/work/lane","agent":"pi","agent_status":"working"}]"#.into();

            rebind(&world.ctx(), "demo", &lane.id, "w1:p2").unwrap();

            let expected = "parent";
            assert!(world.runner.calls.borrow().iter().any(|cmd| {
                cmd.display().contains("pane report-metadata w1:p2")
                    && cmd.display().contains("--clear-token")
                    && cmd.args.iter().any(|arg| arg == expected)
                    && (machine.is_empty() || cmd.display().contains("--machine buildbox"))
            }));
        }
    }

    #[test]
    fn box_final_copy_uses_latest_done_by_creation_then_id_in_current_attempt() {
        use crate::contracts::{DonePayload, Event, EventPayload, Recipient};

        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = thread::allocate(&project, |t| {
            t.machine = "box".into();
            t.attempt = 2;
        })
        .unwrap();
        let latest_hash = thread::store_artifact(&project, b"corrected report").unwrap();
        let earlier_hash = thread::store_artifact(&project, b"earlier report").unwrap();
        for (id, attempt, created, artifact) in [
            ("1", 2, "2026-09-30T10:00:00Z", &earlier_hash),
            ("2", 2, "2026-09-30T11:00:00Z", &earlier_hash),
            ("3", 2, "2026-09-30T11:00:00Z", &latest_hash),
            ("4", 2, "2026-09-30T09:00:00Z", &earlier_hash),
            ("5", 1, "2026-09-30T12:00:00Z", &earlier_hash),
        ] {
            let event = Event {
                id: format!("{}-{attempt}-{id}", lane.id),
                op: format!("{}-{attempt}-{id}", lane.id),
                thread: lane.id.clone(),
                attempt,
                recipient: Recipient::default(),
                created: created.into(),
                payload: EventPayload {
                    done: Some(DonePayload {
                        artifact: artifact.clone(),
                        sha: "abc".into(),
                        report_path: "report.md".into(),
                        has_changes: None,
                        attestation: None,
                        published_ref: None,
                    }),
                    ..EventPayload::default()
                },
            };
            crate::events::seal_create_if_absent(&project, &event).unwrap();
        }
        let copied = final_copy(&world.ctx(), &project, &lane);
        assert_eq!(copied.outcome, CopyOutcome::Complete);
        assert_eq!(copied.report_hash.as_deref(), Some(latest_hash.as_str()));

        // A missing newest artifact must block removal, not fall back to an old seal.
        std::fs::remove_file(crate::events::artifact_path(&project, &latest_hash)).unwrap();
        let copied = final_copy(&world.ctx(), &project, &lane);
        assert!(matches!(copied.outcome, CopyOutcome::Partial(_)));
        assert!(copied.report_hash.is_none());
    }

    #[test]
    fn pasted_error_and_code_spans_do_not_create_report_links() {
        let report = "Résumé: ![real](visible.png)\n```text\nError [ERR_MODULE_NOT_FOUND]: Cannot find package 'yaml' imported from ...\n![x](hidden.png)\n```\n~~~\n[missing]: also-hidden.png\n~~~\n`![inline](inline.png)` and ``[label]: invisible.png``\n";
        let links = report_destinations(report);
        assert_eq!(
            links.into_iter().map(|(_, dest)| dest).collect::<Vec<_>>(),
            vec!["visible.png"]
        );
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

    #[test]
    fn failed_reviewer_placement_waits_for_review_retry_instead_of_ticker_recovery() {
        let world = crate::scenarios::World::new();
        let project = crate::project::create(&world.root, "demo", "", vec![]).unwrap();
        let reviewer = thread::allocate(&project, |t| {
            t.role = "reviewer".into();
            t.status = Status::Failed;
            t.recovery_pending = true;
            t.launch_attempts = 0;
        })
        .unwrap();
        let error = place_prelaunch_recovery(&world.ctx(), &project, &reviewer).unwrap_err();
        assert!(error.to_string().contains("not reachable"), "{error:#}");
        let failed = thread::load(&project, &reviewer.id).unwrap();
        assert_eq!(failed.launch_attempts, 0);
        assert_eq!(failed.status, Status::Failed);
        assert!(!failed.recovery_pending);
        assert!(failed.error.contains("not reachable"));
        // The next ticker pass cannot place this reviewer ahead of its clock.
        crate::recovery::tick(&world.ctx(), &project).unwrap();
        assert_eq!(
            thread::load(&project, &reviewer.id)
                .unwrap()
                .launch_attempts,
            0
        );
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
            recipient: crate::contracts::Recipient::default(),
            created: project::now(),
            payload: crate::contracts::EventPayload {
                done: Some(crate::contracts::DonePayload {
                    has_changes: None,
                    sha: String::new(),
                    report_path: "stored".into(),
                    artifact: "hash".into(),
                    attestation: None,
                    published_ref: None,
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
    fn resolved_empty_commit_branch_is_finished() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let record = Thread {
            status: Status::Resolved,
            repo: "/repo".into(),
            branch: "lane".into(),
            base: "base".into(),
            ..worktree_thread()
        };
        world.runner.on(
            "for-each-ref --format=%(refname) %(objectname) refs/heads/lane",
            crate::runner::fake::ok("refs/heads/lane empty-commit\n"),
        );
        world.runner.on(
            "rev-parse base^{tree}",
            crate::runner::fake::ok("same-tree\n"),
        );
        world.runner.on(
            "rev-parse empty-commit^{tree}",
            crate::runner::fake::ok("same-tree\n"),
        );
        for merged in [None, Some(BTreeSet::new())] {
            assert_eq!(
                finished_worktree_reason_with_merged(
                    &world.ctx(),
                    &project,
                    &record,
                    merged.as_ref()
                )
                .unwrap(),
                None
            );
        }
    }

    #[test]
    fn resolved_unmerged_changed_branch_is_not_finished() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let record = Thread {
            status: Status::Resolved,
            repo: "/repo".into(),
            branch: "lane".into(),
            base: "base".into(),
            ..worktree_thread()
        };
        world.runner.on(
            "for-each-ref --format=%(refname) %(objectname) refs/heads/lane",
            crate::runner::fake::ok("refs/heads/lane changed-commit\n"),
        );
        world.runner.on(
            "rev-parse base^{tree}",
            crate::runner::fake::ok("base-tree\n"),
        );
        world.runner.on(
            "rev-parse changed-commit^{tree}",
            crate::runner::fake::ok("changed-tree\n"),
        );
        world.runner.on(
            "symbolic-ref --short HEAD",
            crate::runner::fake::ok("main\n"),
        );
        world.runner.on(
            "for-each-ref --format=%(refname) %(objectname) refs/heads/main",
            crate::runner::fake::ok("refs/heads/main base\n"),
        );
        world.runner.on(
            "merge-base --is-ancestor changed-commit base",
            crate::runner::fake::fail(1, ""),
        );
        for merged in [None, Some(BTreeSet::new())] {
            let reason = finished_worktree_reason_with_merged(
                &world.ctx(),
                &project,
                &record,
                merged.as_ref(),
            )
            .unwrap()
            .unwrap();
            assert!(reason.starts_with("work_not_done:"), "{reason}");
        }
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
    fn successful_follow_up_keeps_its_delivery_receipt_when_the_attempt_changes() {
        use crate::runner::fake::ok;
        use crate::scenarios::{World, agent_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |t| {
            t.attempt = 2;
            t.prompt_pending = false;
            t.bootstrap = "acknowledged".into();
            t.launch.kind = "pi".into();
        });
        thread::update(&project, &lane.id, |t| t.bootstrap = "acknowledged".into()).unwrap();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json("w2", "w2:t1", "w2:p1", &lane.cwd, &lane.agent_name, "idle")
        );
        crate::events::seal_create_if_absent(
            &project,
            &crate::contracts::Event {
                id: "waiting-2".into(),
                op: "waiting-2".into(),
                thread: lane.id.clone(),
                attempt: 2,
                recipient: crate::contracts::Recipient::default(),
                created: project::now(),
                payload: crate::contracts::EventPayload {
                    waiting: Some(crate::contracts::WaitingPayload {
                        text: "Choose a correction".into(),
                        ..crate::contracts::WaitingPayload::default()
                    }),
                    ..crate::contracts::EventPayload::default()
                },
            },
        )
        .unwrap();
        let sending_project = project.clone();
        let sending_id = lane.id.clone();
        world.runner.on_fn(
            |cmd| cmd.display().contains("agent prompt"),
            move |_| {
                let current = thread::load(&sending_project, &sending_id).unwrap();
                assert_eq!(current.follow_ups[0].state, FollowUpState::Uncertain);
                assert_eq!(current.follow_ups[0].waiting_event, "waiting-2");
                // Model a replacement after submission but before its successful
                // response reaches ADE. Only the old delivery belongs to attempt 2.
                thread::update(&sending_project, &sending_id, |t| {
                    t.attempt = 3;
                    t.connection_waiting = true;
                })
                .unwrap();
                Ok(ok(r#"{"result":{}}"#))
            },
        );
        assert!(matches!(
            prompt(&world.ctx(), "demo", &lane.id, "the accepted correction").unwrap(),
            PromptOutcome::Sent { attempt: 2, .. }
        ));
        let current = thread::load(&project, &lane.id).unwrap();
        assert_eq!(current.attempt, 3);
        assert_eq!(current.follow_ups[0].state, FollowUpState::Delivered);
        assert!(!current.follow_ups[0].delivered_at.is_empty());
        assert!(current.answered_waiting_event.is_empty());
        assert!(current.connection_waiting);
    }

    #[test]
    fn sealed_lane_reopens_on_prompt_and_retry_in_same_folder_with_session() {
        use crate::runner::fake::ok;
        use crate::scenarios::{World, agent_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let folder = world.home.path().join("lane");
        std::fs::create_dir_all(&folder).unwrap();
        let lane = world.thread(&project, &folder, |t| {
            t.attempt = 1;
            t.parked = true;
            t.last_group = Group::Parked.token().into();
            t.bootstrap = "acknowledged".into();
            t.prompt_pending = false;
            t.launch.kind = "pi".into();
            t.launch.recipe_id = "test_pi".into();
            t.launch.brief_hash = "brief".into();
            t.agent = "pi".into();
            t.identity.agent_session = Some("session-42".into());
        });
        thread::update(&project, &lane.id, |t| t.bootstrap = "acknowledged".into()).unwrap();
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        let cwd = folder.to_string_lossy();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json("w1", "w1:t2", "w1:p2", &cwd, &lane.agent_name, "idle")
        );
        world.runner.on("tab create", ok(&format!(
            r#"{{"result":{{"root_pane":{{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","cwd":"{cwd}"}}}}}}"#
        )));
        world.runner.on(
            "pane cwd",
            ok(&format!(r#"{{"result":{{"cwd":"{cwd}"}}}}"#)),
        );
        world.runner.on("agent start", ok(&format!(
            r#"{{"result":{{"agent":{{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","cwd":"{cwd}","name":"{}","agent_status":"idle"}}}}}}"#,
            lane.agent_name
        )));
        world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
        world.runner.on(
            "pane process-info",
            ok(r#"{"result":{"process_info":{"foreground_processes":[]}}}"#),
        );
        let ctx = world.ctx();
        let outcome = prompt(&ctx, "demo", &lane.id, "Fix the rejection").unwrap();
        assert!(
            matches!(outcome, PromptOutcome::Queued { .. }),
            "{outcome:?}"
        );
        let reopened = thread::load(&project, &lane.id).unwrap();
        assert!(!reopened.parked);
        assert!(!reopened.prompt_pending);
        assert_eq!(reopened.bootstrap, "acknowledged");
        assert_eq!(
            reopened.identity.agent_session.as_deref(),
            Some("session-42")
        );
        assert_eq!(reopened.follow_ups.len(), 1);
        assert_eq!(reopened.follow_ups[0].text, "Fix the rejection");
        assert!(!parkable(&project, &reopened));
        assert!(
            !attempt_sealed(&project, &reopened),
            "a reopened correction needs a new seal"
        );
        park_completed(&ctx, &project).unwrap();
        assert!(!thread::load(&project, &lane.id).unwrap().parked);
        assert_eq!(reopened.worktree_path, lane.worktree_path);
        assert_eq!(reopened.attempt, 1);
        assert!(world.runner.calls.borrow().iter().any(|c| {
            let text = c.display();
            text.contains("agent start") && text.contains("--session session-42")
        }));
        // Round retry uses the same reopen path instead of spending a new attempt.
        thread::update(&project, &lane.id, |t| {
            // Model a newer seal after the queued correction was handled.
            t.follow_ups.clear();
            t.parked = true;
            t.last_group = Group::Parked.token().into();
            t.pane_id = "w2:p1".into();
            t.tab_id = "w2:t1".into();
            t.workspace_id = "w2".into();
        })
        .unwrap();
        let retried = retry_during_advance(&ctx, "demo", &lane.id, "Repair the conflict").unwrap();
        assert_eq!(retried.attempt, 1);
        assert_eq!(retried.pane_id, "w1:p2");
        assert_eq!(
            thread::load(&project, &lane.id).unwrap().worktree_path,
            lane.worktree_path
        );
        assert_eq!(world.runner.count("agent prompt"), 0);
        let retried_lane = thread::load(&project, &lane.id).unwrap();
        assert_eq!(retried_lane.follow_ups.len(), 1);
        assert_eq!(retried_lane.follow_ups[0].text, "Repair the conflict");
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
                ("rank".to_string(), "3".to_string()),
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
    fn start_during_install_is_recorded_and_launched_by_the_next_ticker() {
        use crate::runner::fake::ok;
        use crate::scenarios::{World, pane_json};

        let world = World::new();
        let project = world.project("demo", "a.sock");
        let repo = world.home.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let repo = std::fs::canonicalize(repo)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        world.add_repo(&project, &repo);
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        world.runner.on(
            "HERDR_ADE_LAUNCH",
            ok(r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","cwd":"/wt"}}}"#),
        );
        world.runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","name":"hp-demo-t-0001"}}}"#),
        );
        let split = GitReal {
            fake: &world.runner,
        };
        let ctx = Ctx {
            env: &world.env,
            root: world.root.clone(),
            config_dir: world.home.path().join("cfg"),
            runner: &split,
            // The fake runner drives the ticker pass below, not a subprocess.
            detached_ticker: false,
        };
        let install = crate::harness::lock(&ctx.config_dir).unwrap();
        let started = start(
            &ctx,
            "demo",
            StartArgs {
                title: "Repair".into(),
                repo: Some(repo.clone()),
                machine: None,
                base: None,
                task: "Repair the lane.".into(),
                plain: "The lane repairs the project.".into(),
                workflow: None,
                recipe: None,
                task_id: String::new(),
                review_id: String::new(),
            },
        )
        .unwrap();
        assert!(started.prompt_pending);
        assert!(Path::new(&started.thread_dir).join("brief.md").exists());
        assert_eq!(
            thread::load(&project, &started.id).unwrap().status,
            Status::Open
        );
        assert!(!world.runner.calls.borrow().iter().any(|cmd| {
            cmd.display().contains("agent start") && !cmd.display().contains("--help")
        }));

        drop(install);
        let wt = started.worktree_path.clone();
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            pane_json("w1", "w1:t2", "w1:p2", &wt)
        );
        crate::ticker::tick_project(&ctx, &project).unwrap();
        assert!(world.runner.calls.borrow().iter().any(|cmd| {
            cmd.display().contains("agent start") && cmd.display().contains("hp-demo-t-0001")
        }));
        assert_eq!(
            thread::load(&project, &started.id).unwrap().launch_attempts,
            1
        );
    }

    #[test]
    fn explicit_recipe_without_a_request_quote_keeps_the_lead_brief_across_retry() {
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
            "[routing]\ndefault = \"test_claude\"\nretries = 1\n\n[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n\n[recipes.chosen_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the coordinator's choice\"\n",
        )
        .unwrap();

        crate::prompt::record_test_request(
            &project,
            "q-brief",
            "Keep the complete lead brief with the task.",
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
                recipe: Some("chosen_claude".into()),
                // The CLI maps `--job` to this stable task id.
                task_id: stable_task.id.clone(),
                review_id: String::new(),
            },
        )
        .unwrap();
        assert_eq!(started.role, "lane");
        assert_eq!(started.launch.recipe_id, "chosen_claude");
        assert_eq!(started.launch.routing_rule, "explicit");
        assert!(started.launch.recipe_basis.is_empty());
        assert!(started.launch.recipe_request.is_empty());
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
        assert!(!launch.contains("--parent"), "{launch}");
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
        // Recovery must not refuse a recorded replacement while an install
        // owns the ticker: the install's ticker launches this attempt too.
        let install = crate::harness::lock(&ctx.config_dir).unwrap();
        retry(&ctx, "demo", &started.id, "the first process disappeared").unwrap();
        drop(install);
        let retried = thread::load(&project, &started.id).unwrap();
        assert_eq!(retried.attempt, 2);
        assert_eq!(retried.launch.kind, kind);
        assert_eq!(retried.launch.attempt, 2);
        assert_eq!(retried.launch.work_retries, 0);
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
                "[routing]\ndefault = \"test_claude\"\nretries = 1\n\n[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n\n[harness]\nrepos = [{{ path = \"{harness_s}\" }}]\n"
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
            task_id: String::new(),
            review_id: String::new(),
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
                task_id: String::new(),
                review_id: String::new(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(missing.contains("plain_missing"), "{missing}");
    }

    #[test]
    fn a_held_machine_refuses_a_new_box_start() {
        let root = tempfile::tempdir().unwrap();
        assert!(!project::machine_held(root.path(), "buildbox"));
        project::machine_hold(root.path(), "buildbox").unwrap();
        assert!(project::machine_held(root.path(), "buildbox"));
        assert!(project::machine_release(root.path(), "buildbox").unwrap());
        assert!(!project::machine_held(root.path(), "buildbox"));
        assert!(!project::machine_release(root.path(), "buildbox").unwrap());
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
    fn box_fixture() -> (crate::testkit::Fx, String) {
        let fx = crate::testkit::fixture();
        let remote = fx.world.home.path().join("remote.git");
        let status = std::process::Command::new("git")
            .args(["init", "--bare", "-q", &remote.to_string_lossy()])
            .status()
            .unwrap();
        assert!(status.success());
        let remote = remote.to_string_lossy().into_owned();
        crate::testkit::git(&fx.repo, &["remote", "add", "box", &remote]);
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos = vec![crate::project::Repo {
            path: fx.repo.to_string_lossy().into_owned(),
            box_path: Some("/home/agent/projects/repo".into()),
            publish_url: Some(remote.clone()),
            ..Default::default()
        }];
        let text = format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap());
        std::fs::write(fx.project.project_md(), text).unwrap();
        (fx, remote)
    }

    fn write_config(fx: &crate::testkit::Fx, text: &str) {
        let cfg = fx.world.home.path().join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("config.toml"), text).unwrap();
    }

    /// The profile, provisioning and create fakes a box start needs.
    fn stub_box(fx: &crate::testkit::Fx) {
        use crate::runner::fake::ok;
        fx.world.runner.on(
            "machine list --json",
            ok(r#"[{"id":"buildbox-id","label":"buildbox","target":"buildbox-pi","session":"default","enabled":true}]"#),
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
                if script.contains("getconf _NPROCESSORS_ONLN") {
                    return Ok(ok("1.0 16\n"));
                }
                let base = script
                    .split("FETCH_HEAD)\" = ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or("");
                Ok(ok(&format!("{base}\n")))
            },
        );
    }

    const ROUTED_BOX_CONFIG: &str = "[routing]\ndefault = \"test_claude\"\nretries = 1\n\n[[routing.rules]]\nproduct = \"web-research\"\nrecipe = \"agy_gemini_flash\"\n\n[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n\n[dispatch]\nmachine = \"buildbox\"\n";

    const NATIVE_BOX: &str = "\n[machines.buildbox]\nlabel = \"buildbox\"\ntarget = \"buildbox-pi\"\nsession = \"default\"\nhome = \"/home/agent\"\nroot = \"/home/agent/.herdr-ade\"\nworktrees = \"/home/agent/projects\"\nbuild = \"/home/agent/build/lanes\"\npath = \"/home/agent/.local/bin:/home/agent/.cargo/bin:/usr/local/bin:/usr/bin:/bin\"\nade_bin = \"/home/agent/.local/bin/herdr-ade\"\npi_bin = \"/home/agent/.local/bin/herdr-pi\"\nkinds = [\"pi\", \"claude\", \"agy\"]\n";

    fn lane_config() -> String {
        format!("{ROUTED_BOX_CONFIG}{NATIVE_BOX}")
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
            task_id: String::new(),
            review_id: String::new(),
        }
    }

    #[test]
    fn coordinator_cannot_start_a_reviewer_workflow() {
        let fx = crate::testkit::fixture();
        let mut args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
        args.workflow = Some("reviewer".into());
        let error = start(&fx.world.ctx(), "demo", args)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "workflow_reserved: reviewer lanes are started by ha review. For an independent check use --workflow critic (verdict = \"PASS\"|\"FAIL\" front matter); for a specific recipe use --recipe <id>."
        );
    }

    #[test]
    fn only_lane_and_reviewer_default_to_the_box() {
        assert_eq!(
            default_machine("lane", "buildbox", Some("/r")),
            Some("buildbox".into())
        );
        assert_eq!(
            default_machine("reviewer", "buildbox", Some("/r")),
            Some("buildbox".into())
        );
        assert_eq!(default_machine("research", "buildbox", Some("/r")), None);
        assert_eq!(default_machine("lane", "", Some("/r")), None);
        assert_eq!(default_machine("lane", "buildbox", None), None);
    }

    #[test]
    fn retry_keeps_a_placed_machine_and_dispatches_an_unplaced_lane() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, &lane_config());
        stub_box(&fx);
        let args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
        let started = start(&fx.world.ctx(), "demo", args).unwrap();
        assert_eq!(started.machine, "buildbox");
        // A placed attempt keeps its saved machine when retried.
        thread::update(&fx.project, &started.id, |t| {
            t.failure_class = crate::contracts::FailureClass::ProcessGone;
        })
        .unwrap();
        retry(&fx.world.ctx(), "demo", &started.id, "process disappeared").unwrap();
        let placed = thread::load(&fx.project, &started.id).unwrap();
        assert_eq!(placed.machine, "buildbox");
        assert!(placed.placement_reason.contains("machine kept"));

        // A selected local machine is also a placement even though its saved
        // machine is empty. Do not move an explicitly local start to the box.
        let mut local_args = start_args(
            Some(fx.repo.to_string_lossy().into_owned()),
            Some("local".into()),
        );
        local_args.base = Some("main".into());
        let local = start(&fx.world.ctx(), "demo", local_args).unwrap();
        crate::testkit::git(
            &fx.repo,
            &["worktree", "remove", "--force", &local.worktree_path],
        );
        crate::testkit::git(&fx.repo, &["branch", "-D", &local.branch]);
        thread::update(&fx.project, &local.id, |t| {
            t.pane_id.clear();
            t.tab_id.clear();
            t.worktree_path.clear();
            t.base = "main".into();
            t.failure_class = crate::contracts::FailureClass::ProcessGone;
        })
        .unwrap();
        retry(&fx.world.ctx(), "demo", &local.id, "failed before launch").unwrap();
        let retried_local = thread::load(&fx.project, &local.id).unwrap();
        assert!(retried_local.machine.is_empty());
        assert_eq!(retried_local.launch.machine, "local");
        assert!(retried_local.placement_reason.contains("machine kept"));

        // A failed start before it acquired any machine or work is dispatched
        // again, using the current routing pick and the box mapping.
        let args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
        let unplaced = start(&fx.world.ctx(), "demo", args).unwrap();
        thread::update(&fx.project, &unplaced.id, |t| {
            t.machine.clear();
            t.machine_id.clear();
            t.placement_reason.clear();
            t.launch.machine = "local".into();
            t.pane_id.clear();
            t.tab_id.clear();
            t.workspace_id.clear();
            t.worktree_path.clear();
            t.failure_class = crate::contracts::FailureClass::ProcessGone;
        })
        .unwrap();
        retry(
            &fx.world.ctx(),
            "demo",
            &unplaced.id,
            "git timed out before launch",
        )
        .unwrap();
        let retried = thread::load(&fx.project, &unplaced.id).unwrap();
        assert_eq!(retried.machine, "buildbox");
        assert_eq!(retried.launch.machine, "buildbox");
        assert!(retried.placement_reason.contains("retry: recipe"));
    }

    #[test]
    fn box_disk_floor_refuses_before_creating_work_and_recovers() {
        use crate::runner::fake::ok;
        let (fx, _remote) = box_fixture();
        write_config(&fx, &lane_config());
        let free = std::rc::Rc::new(std::cell::Cell::new(5_u64));
        let current = free.clone();
        fx.world.runner.on_fn(
            |cmd| cmd.program == "ssh" && cmd.display().contains("disk_free_kb"),
            move |_| {
                Ok(ok(&format!(
                    "disk_free_kb\t{}\n",
                    current.get() * 1_000_000
                )))
            },
        );
        stub_box(&fx);
        let error = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("disk_low: buildbox has 5.1 GB free under /home/agent/projects"),
            "{error}"
        );
        assert!(thread::list(&fx.project).is_empty());
        assert_eq!(fx.world.runner.count("tab create"), 0);
        assert_eq!(fx.world.runner.count("workspace create"), 0);
        free.set(20);
        let started = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        assert_eq!(started.machine, "buildbox");
        assert!(!started.worktree_path.is_empty());
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
        assert_eq!(started.machine, "buildbox");
        assert_eq!(started.machine_id, "buildbox-id");
        assert_eq!(started.launch.machine, "buildbox");
        assert!(
            started
                .worktree_path
                .starts_with("/home/agent/projects/repo/.worktrees/")
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
                .contains(&format!("tab rename w1:t2 {} starting…", started.id))
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
        write_config(
            &fx,
            &format!("{ROUTED_BOX_CONFIG}{}", crate::remote::TEST_MACHINE),
        );
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

        let dispatch =
            std::fs::read_to_string(fx.project.state_dir().join("dispatch.jsonl")).unwrap();
        assert!(
            dispatch.contains("does not run adapter kind `claude`"),
            "{dispatch}"
        );
        assert!(
            dispatch.contains("does not run adapter kind `agy`"),
            "{dispatch}"
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
        write_config(
            &fx,
            &format!("{ROUTED_BOX_CONFIG}{}", crate::remote::TEST_MACHINE),
        );
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
            if role == Some("reviewer") {
                // Only the review path supplies this identity; test routing without
                // starting an unsupported hand-made reviewer.
                let picked = crate::launch::resolve_launch(
                    &fx.world.ctx(),
                    &fx.project,
                    &crate::launch::ResolveInput {
                        task,
                        workflow: "reviewer",
                        ..Default::default()
                    },
                )
                .unwrap();
                assert_eq!(picked.recipe_id, "pi_opencode_deepseek");
                continue;
            }
            let started = start(&fx.world.ctx(), "demo", args).unwrap();
            assert_eq!(started.launch.recipe_id, "pi_opencode_deepseek");
            assert_eq!(started.machine, "buildbox");
            assert_eq!(started.machine_id, "buildbox-id");
        }
    }

    #[test]
    fn lane_waits_for_provider_and_starts_on_next_pass_without_routing_retry() {
        use crate::runner::fake::{fail, ok};
        let (fx, _remote) = box_fixture();
        write_config(
            &fx,
            &format!("{ROUTED_BOX_CONFIG}{}", crate::remote::TEST_MACHINE),
        );
        let task = "Review this pile.";
        let config_path = fx.world.home.path().join("cfg/config.toml");
        let config = std::fs::read_to_string(&config_path).unwrap();
        let hash = crate::thread::sha256_hex(task.as_bytes());
        std::fs::write(
            &config_path,
            format!("{config}\n[routing.pins]\n\"{hash}\" = \"pi_opencode_deepseek\"\n"),
        )
        .unwrap();
        let ready = std::rc::Rc::new(std::cell::Cell::new(false));
        let state = ready.clone();
        fx.world.runner.on_fn(
            |cmd| {
                cmd.program == "ssh"
                    && cmd.display().contains("herdr-pi")
                    && cmd.display().contains(" check ")
            },
            move |_| {
                if state.get() {
                    Ok(ok("ok"))
                } else {
                    Ok(fail(1, "provider readiness probe timed out"))
                }
            },
        );
        stub_box(&fx);
        let mut args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
        args.task = task.into();
        let waiting = start(&fx.world.ctx(), "demo", args).unwrap();
        assert_eq!(waiting.status, Status::Starting);
        assert!(!waiting.provider_wait_started.is_empty());
        assert_eq!(waiting.launch_attempts, 0);
        assert!(waiting.pane_id.is_empty());
        let ctx = fx.world.ctx();
        let view = session_view(&ctx, &fx.project).unwrap();
        let mut local_wait = waiting.clone();
        local_wait.machine.clear();
        let shown = row(&local_wait, Some(&view), jiff::Timestamp::now());
        assert_eq!(shown.group, Group::Working);
        assert_eq!(
            shown.note,
            format!(
                "waiting for provider opencode-go: readiness probe timed out at {}; the start is queued and retries by itself",
                waiting.provider_wait_started
            )
        );
        let other = thread::allocate(&fx.project, |t| {
            t.launch = waiting.launch.clone();
            t.machine = waiting.machine.clone();
            t.role = "lane".into();
            t.provider_wait_started = waiting.provider_wait_started.clone();
        })
        .unwrap();
        let before = fx.world.runner.count("herdr-pi check");
        crate::ticker::resume_provider_starts(
            &fx.world.ctx(),
            &fx.project,
            &mut std::collections::BTreeMap::new(),
            |error| panic!("{error:#}"),
        );
        assert_eq!(fx.world.runner.count("herdr-pi check") - before, 1);
        thread::update(&fx.project, &other.id, |t| t.status = Status::Resolved).unwrap();
        ready.set(true);
        crate::ticker::resume_provider_starts(
            &fx.world.ctx(),
            &fx.project,
            &mut std::collections::BTreeMap::new(),
            |error| panic!("{error:#}"),
        );
        let placed = thread::load(&fx.project, &waiting.id).unwrap();
        assert_eq!(placed.status, Status::Open);
        assert_eq!(placed.machine_id, "buildbox-id");
        assert!(placed.provider_wait_started.is_empty());
        assert_eq!(placed.launch_attempts, 0);
        assert_eq!(placed.attempt, 1);
        assert!(!placed.pane_id.is_empty());
    }

    #[test]
    fn an_explicit_machine_that_excludes_the_pick_is_refused_without_ssh() {
        let (fx, _remote) = box_fixture();
        write_config(
            &fx,
            &format!("{ROUTED_BOX_CONFIG}{}", crate::remote::TEST_MACHINE),
        );
        stub_box(&fx);
        let mut args = start_args(
            Some(fx.repo.to_string_lossy().into_owned()),
            Some("buildbox".into()),
        );
        args.task = "+++\nproduct = \"web-research\"\n+++\nCompare the published results.".into();
        let error = start(&fx.world.ctx(), "demo", args)
            .unwrap_err()
            .to_string();
        assert!(error.contains("recipe_unavailable"), "{error}");
        assert!(error.contains("agy_gemini_flash"), "{error}");
        assert!(error.contains("buildbox"), "{error}");
        assert!(error.contains("does not run adapter kind `agy`"), "{error}");
        assert!(thread::list(&fx.project).is_empty());
        let dispatch =
            std::fs::read_to_string(fx.project.state_dir().join("dispatch.jsonl")).unwrap();
        assert!(dispatch.contains("placement-refused"), "{dispatch}");
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
                r#"[{"id":"buildbox-id","label":"buildbox","target":"buildbox-pi","session":"default","enabled":true}]"#,
            ),
        );
        runner.on_fn(
            |cmd| cmd.program == "ssh" && cmd.display().contains("getconf _NPROCESSORS_ONLN"),
            |_| Ok(crate::runner::fake::ok("1.0 16\n")),
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
            machine: "buildbox".into(),
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
        assert!(
            error.contains("buildbox") && error.contains("local"),
            "{error}"
        );
        assert!(
            error.contains("lane PATH") && error.contains("not signed in"),
            "{error}"
        );
    }

    #[test]
    fn box_repo_row_names_each_missing_piece() {
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            config.path().join("config.toml"),
            crate::remote::TEST_MACHINE,
        )
        .unwrap();
        let mut settings = crate::project::Settings {
            repos: vec![crate::project::Repo {
                path: "/r".into(),
                box_path: Some("/box/r".into()),
                publish_url: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        let e = box_repo_row(config.path(), &settings, "buildbox", "/r")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("box_publish_url_missing") && e.contains("publish_url"),
            "{e}"
        );

        settings.repos[0].box_path = None;
        settings.repos[0].publish_url = Some("https://example/r.git".into());
        let e = box_repo_row(config.path(), &settings, "buildbox", "/r")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("box_path_missing") && e.contains("box_path"),
            "{e}"
        );

        // A repo with neither, and no configured mapping, stays unmapped.
        settings.repos[0].publish_url = None;
        let e = box_repo_row(config.path(), &settings, "buildbox", "/other")
            .unwrap_err()
            .to_string();
        assert!(e.contains("box_repo_unmapped"), "{e}");
        let e = box_repo_row(
            config.path(),
            &settings,
            "buildbox",
            "/Users/agent/projects/herdr",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("box_repo_unmapped"), "{e}");
    }

    #[test]
    fn machine_repo_mapping_wins_over_generic_project_box_path() {
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            config.path().join("config.toml"),
            format!(
                "{}\n[[machines.buildbox.repos]]\npath = '/r'\nbox_path = '/second/r'\n",
                crate::remote::TEST_MACHINE
            ),
        )
        .unwrap();
        let project_row = crate::project::Repo {
            path: "/r".into(),
            box_path: Some("/first/r".into()),
            publish_url: Some("https://example/first.git".into()),
            ..Default::default()
        };
        assert_eq!(
            box_repo_candidate(config.path(), "buildbox", Some("/r"), Some(&project_row)).unwrap(),
            ("/second/r".into(), "https://example/first.git".into())
        );
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
        let dispatch =
            std::fs::read_to_string(fx.project.state_dir().join("dispatch.jsonl")).unwrap();
        assert!(dispatch.contains("box_publish_url_missing"), "{dispatch}");

        let error = start(
            &fx.world.ctx(),
            "demo",
            start_args(
                Some(fx.repo.to_string_lossy().into_owned()),
                Some("buildbox".into()),
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
        use crate::testkit::{fixture, git};

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
    fn a_gone_box_worktree_is_removed_only_after_the_box_prunes_it() {
        use crate::runner::fake::ok;

        let fx = crate::testkit::fixture();
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
                .contains("rm -rf -- /home/agent/build/lanes/demo-t-0001")
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
    fn retry_cleanup_closes_its_tab_even_after_the_shell_changes_directory() {
        use crate::scenarios::{World, pane_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("lane");
        let lane = world.thread(&project, &cwd, |_| {});
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            pane_json("w2", "w2:t1", "w2:p1", "/somewhere/else")
        );
        assert!(close_pane(&world.ctx(), &project, &lane).unwrap());
        assert_eq!(world.runner.count("workspace close"), 1);
    }

    #[test]
    fn box_lane_close_uses_its_saved_machine() {
        use crate::scenarios::{World, pane_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let cwd = world.home.path().join("lane");
        let lane = world.thread(&project, &cwd, |t| {
            t.machine = "oci".into();
            t.machine_id = "oci".into();
        });
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            pane_json("w2", "w2:t1", "w2:p1", "/different/cwd")
        );
        assert!(close_pane(&world.ctx(), &project, &lane).unwrap());
        assert!(
            world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|cmd| { cmd.display().contains("--machine oci workspace close w2") })
        );
    }

    #[test]
    fn cleanup_slice_reuses_machine_lists_and_drops_closed_tabs() {
        use crate::scenarios::{World, agent_json, pane_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        // Box snapshots must not depend on a live local coordinator session.
        std::fs::remove_file(project.coordinator().unwrap().socket).unwrap();
        let first = Thread {
            id: "t-0001".into(),
            machine: "oci".into(),
            workspace_id: "w2".into(),
            tab_id: "w2:t1".into(),
            pane_id: "w2:p1".into(),
            cwd: "/lane1".into(),
            agent_name: "lane1".into(),
            ..Thread::default()
        };
        let second = Thread {
            id: "t-0002".into(),
            tab_id: "w2:t2".into(),
            pane_id: "w2:p2".into(),
            cwd: "/lane2".into(),
            agent_name: "lane2".into(),
            ..first.clone()
        };
        *world.agents.borrow_mut() = format!(
            "[{},{}]",
            agent_json("w2", "w2:t1", "w2:p1", "/lane1", "lane1", "idle"),
            agent_json("w2", "w2:t2", "w2:p2", "/lane2", "lane2", "idle")
        );
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            pane_json("w2", "w2:t1", "w2:p1", "/lane1"),
            pane_json("w2", "w2:t2", "w2:p2", "/lane2")
        );
        let mut views = CleanupViews::default();
        let ctx = world.ctx();
        assert!(close_pane_with_views(&ctx, &project, &first, &mut views).unwrap());
        assert!(close_pane_with_views(&ctx, &project, &second, &mut views).unwrap());
        assert_eq!(world.runner.count("--machine oci agent list"), 1);
        assert_eq!(world.runner.count("--machine oci pane list"), 1);
        assert_eq!(world.runner.count("--machine oci tab close w2:t1"), 1);
        // After closing the first tab, the second owns the rest of its workspace.
        assert_eq!(world.runner.count("--machine oci workspace close w2"), 1);
        assert!(!close_pane_with_views(&ctx, &project, &first, &mut views).unwrap());
        // The next slice observes fresh state, not the previous pass's cache.
        assert!(
            close_pane_with_views(&ctx, &project, &first, &mut CleanupViews::default()).unwrap()
        );
        assert_eq!(world.runner.count("--machine oci agent list"), 2);
    }

    #[test]
    fn cleanup_retries_are_bounded_and_an_unreachable_member_does_not_starve_the_queue() {
        use crate::scenarios::World;
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let ids = (0..3)
            .map(|_| {
                thread::allocate(&project, |t| {
                    t.status = Status::Resolved;
                    t.machine = "box".into();
                    t.cleanup_pending = true;
                    t.cleanup_reason = "merged".into();
                    t.created = "2020-01-01T00:00:00Z".into();
                })
                .unwrap()
                .id
            })
            .collect::<Vec<_>>();
        retry_pending_cleanup(&world.ctx(), &project).unwrap();
        assert_ne!(
            thread::load(&project, &ids[0]).unwrap().updated,
            "2020-01-01T00:00:00Z"
        );
        assert_ne!(
            thread::load(&project, &ids[1]).unwrap().updated,
            "2020-01-01T00:00:00Z"
        );
        assert_eq!(
            thread::load(&project, &ids[2]).unwrap().updated,
            "2020-01-01T00:00:00Z"
        );
        retry_pending_cleanup(&world.ctx(), &project).unwrap();
        assert_ne!(
            thread::load(&project, &ids[2]).unwrap().updated,
            "2020-01-01T00:00:00Z"
        );
        assert!(thread::list(&project).iter().all(|t| t.cleanup_pending));
    }

    #[test]
    fn retry_accepts_a_placed_pane_without_a_launch_attempt() {
        use crate::runner::fake::{FakeRunner, ok};
        let runner = FakeRunner::new();
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        let herdr = Herdr::new("herdr", "", &runner);
        let record = Thread {
            id: "t-0001".into(),
            pane_id: "w1:p2".into(),
            prompt_pending: true,
            status: Status::Open,
            ..Thread::default()
        };
        refuse_busy_retry(&herdr, &record).unwrap();
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
                launch_attempts: 1,
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
    fn retry_accepts_delivery_failure_but_not_a_now_working_agent() {
        use crate::runner::fake::{FakeRunner, ok};
        for state in ["idle", "working"] {
            let runner = FakeRunner::new();
            runner.on("agent list", ok(&format!(r#"{{"result":{{"agents":[{{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","cwd":"/repo","name":"hp-demo-t-0001","agent_status":"{state}"}}]}}}}"#)));
            runner.on("pane read", ok("empty prompt\n"));
            let herdr = Herdr::new("herdr", "", &runner);
            let record = Thread {
                id: "t-0001".into(),
                pane_id: "w1:p2".into(),
                tab_id: "w1:t2".into(),
                workspace_id: "w1".into(),
                cwd: "/repo".into(),
                agent_name: "hp-demo-t-0001".into(),
                status: Status::Failed,
                launch_attempts: 1,
                prompt_pending: true,
                brief_submitted: true,
                error: "brief_delivery_failed: no activity".into(),
                ..Thread::default()
            };
            assert_eq!(refuse_busy_retry(&herdr, &record).is_ok(), state == "idle");
            let mut pending = record.clone();
            pending.status = Status::Open;
            pending.error = "brief_delivery_pending: timeout".into();
            assert!(refuse_busy_retry(&herdr, &pending).is_err());
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
        project::machine_hold(&ctx.root, "buildbox-id").unwrap();
        let started = start(
            &ctx,
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        assert!(started.machine.is_empty());
    }

    #[test]
    fn an_unreachable_box_defers_without_fallback_or_refusal() {
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
        let error = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap_err();
        assert!(crate::remote::is_unreachable(&format!("{error:#}")));
        assert!(thread::list(&fx.project).is_empty());
        let dispatch = std::fs::read_to_string(fx.project.state_dir().join("dispatch.jsonl"))
            .unwrap_or_default();
        assert!(!dispatch.contains("placement-refused"));
    }

    #[test]
    fn an_explicit_held_box_refuses() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, &lane_config());
        stub_box(&fx);
        let ctx = fx.world.ctx();
        project::machine_hold(&ctx.root, "buildbox-id").unwrap();
        let error = start(
            &ctx,
            "demo",
            start_args(
                Some(fx.repo.to_string_lossy().into_owned()),
                Some("buildbox".into()),
            ),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("machine_held"), "{error}");
    }
}
