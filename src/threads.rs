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
use crate::thread::{
    self, CopyOutcome, FollowUp, FollowUpState, Group, Kind, RetirementAuthority,
    RetirementRequest, Status, Thread,
};
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

/// Local lanes name their coordinator pane; box lanes use herdr's existing
/// machine-qualified parent form so they nest under the Mac coordinator.
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
    /// Files explicitly supplied by the coordinator, frozen with the brief.
    pub attach: Vec<String>,
    pub paths: Vec<String>,
    /// Internal flow/skill label; never a coordinator model-selection input.
    pub workflow: Option<String>,
    /// An exact recipe the coordinator chose for this one lane.
    pub recipe: Option<String>,
    pub task_id: String,
    /// Internal reviewer identity; empty for every non-reviewer start.
    pub review_id: String,
}

/// Validate and freeze a placement intent. The ticker owns checkout creation,
/// terminal binding, the lane card, agent submission and initial input.
pub fn start(ctx: &Ctx, slug: &str, args: StartArgs) -> Result<Thread> {
    start_with_attachments(ctx, slug, args, BTreeMap::new())
}

/// Internal pile starts reuse members' already-frozen blobs, not their original
/// host paths or the coordinator's per-lane input-file size budget.
pub(crate) fn start_with_attachments(
    ctx: &Ctx,
    slug: &str,
    args: StartArgs,
    mut attachments: BTreeMap<String, String>,
) -> Result<Thread> {
    let project = Project::load(&ctx.root, slug)?;
    for hash in attachments.values() {
        thread::artifact(&project, hash)?;
    }
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
    for path in &args.paths {
        crate::gate_paths::validate(path)?;
    }
    crate::plan::check_prerequisites(&project, &args.task_id)?;
    let (settings, _) = project.read_project_md()?;
    // Non-blocking even under review's lock: the ticker owns the slow work.
    ticker::ensure(ctx)?;
    require_session(ctx, &project)?;

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
    let mut launch = crate::launch::resolve_launch(
        ctx,
        &project,
        &crate::launch::ResolveInput {
            task: &args.task,
            task_id: Some(&args.task_id),
            workflow: role,
            recipe: args.recipe.as_deref(),
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
        Err(error)
            if provider_readiness_error(&format!("{error:#}"))
                || format!("{error:#}").contains("version_skew:") =>
        {
            // Save an unplaced attempt on its requested machine. No pane or
            // worktree exists until a later readiness probe succeeds.
            let default = default_machine(role, &launch.machine, Some(&repo));
            let machine = explicit_machine
                .or(default.as_deref())
                .unwrap_or(crate::contracts::MACHINE_LOCAL);
            let profile = if machine == crate::contracts::MACHINE_LOCAL {
                None
            } else {
                Some(remote::machine_profile(
                    ctx.runner,
                    &ctx.env.herdr_bin(),
                    &ctx.config_dir,
                    machine,
                )?)
            };
            (
                Placement {
                    machine: profile
                        .as_ref()
                        .map(|p| p.label.clone())
                        .unwrap_or_default(),
                    machine_id: profile.map(|p| p.id).unwrap_or_default(),
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

    // Keep repository/base and named-file refusals synchronous. No checkout,
    // push, terminal or agent is created in this command.
    crate::repo::Git::new(ctx.runner, &repo)
        .with_timeout(GIT_TIMEOUT)
        .run(&["rev-parse", "--show-toplevel"])
        .with_context(|| format!("{repo} is not a git repository"))?;
    let git = crate::repo::Git::new(ctx.runner, &repo).with_timeout(Duration::from_secs(5));
    let base = if role == "reviewer" && !args.review_id.is_empty() {
        // Internal pile starts freeze a commit, not a branch that can move
        // between review allocation and checkout creation.
        git.run(&[
            "rev-parse",
            "--verify",
            &format!(
                "{}^{{commit}}",
                args.base.as_deref().context("review base missing")?
            ),
        ])?
    } else {
        let integration = integration_branch(
            ctx.runner,
            &Thread {
                repo: repo.clone(),
                base: args.base.clone().unwrap_or_default(),
                ..Thread::default()
            },
        )?;
        git.branch_head(&integration)?.ok_or_else(|| {
            crate::refusal::error(
                format!("integration_branch_required: `{integration}` is not a local branch"),
                format!(
                    "ha thread start {slug} --base <existing-branch> --job <job> --task-file <file>"
                ),
            )
        })?
    };
    let origin = crate::repo::Git::new(ctx.runner, &repo)
        .with_timeout(GIT_TIMEOUT)
        .run(&["remote", "get-url", "origin"])
        .unwrap_or_default();
    if !machine.is_empty() {
        let (_, url) = box_repo_candidate(&ctx.config_dir, &machine, Some(&repo), listed)?;
        remote::remote_for_url(ctx.runner, &repo, &url)?;
    }
    let mut remaining = LINKED_FILES_CAP;
    for path in &args.attach {
        let path = Path::new(path);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("attachment needs a UTF-8 basename")?;
        if attachments.contains_key(name) {
            bail!("attachment_duplicate: `{name}` was supplied more than once");
        }
        let file = std::fs::File::open(path)
            .with_context(|| format!("attachment_missing: {}", path.display()))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            bail!("attachment is not a file: {}", path.display());
        }
        if metadata.len() > remaining {
            bail!("attachments over cap (200 MiB): {}", path.display());
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        file.take(remaining + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > remaining {
            bail!("attachments over cap (200 MiB): {}", path.display());
        }
        remaining -= bytes.len() as u64;
        attachments.insert(name.to_string(), thread::store_artifact(&project, &bytes)?);
    }
    let machine_id = placement.machine_id.clone();
    let record = thread::allocate(&project, |t| {
        t.title = args.title.trim().to_string();
        t.kind = Kind::Worktree;
        t.repo = repo.clone();
        t.origin = origin.clone();
        t.machine = machine.clone();
        t.placement_reason = placement.reason.clone();
        t.machine_id = machine_id.clone();
        t.agent = launch.kind.clone();
        t.base = base.clone();
        t.attachments = attachments.clone();
        t.paths = args.paths.clone();
        t.role = role.to_string();
        t.review_id = args.review_id.clone();
        t.plain = args.title.trim().to_string();
        t.attempt = 1;
        t.partial = Some("freeze".into());
        t.launch = launch.clone();
        if let Some(reason) = &provider_wait {
            t.provider_wait_started = project::now();
            t.error = format!("waiting for placement: {reason}");
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

    let started = freeze_start(ctx, &project, &thread::load(&project, &id)?, &args.task)?;
    refresh_plan(ctx, &project);
    if !machine.is_empty() {
        ticker::request_remote_poll(&ctx.root, &project, &machine)?;
    }
    Ok(started)
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
                if missing.contains("version_skew:") {
                    bail!("{missing}");
                }
                if crate::remote::is_unreachable(&missing) {
                    bail!("{missing}");
                }
                if missing.contains("disk_low:") {
                    bail!("{missing}");
                }
                let provider_wait = missing.contains("pi_not_ready")
                    || missing.contains("readiness probe")
                    || missing.contains("provider rejected");
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
        && (error.contains("pi_not_ready")
            || error.contains("readiness probe")
            || error.contains("provider rejected"))
        && !error.contains("box_repo_")
        && !error.contains("machine_held:")
        && !error.contains("machine_kind_unavailable:")
}

/// Complete a provider-blocked placement through the ordinary startup path.
pub(crate) fn resume_provider_start(ctx: &Ctx, project: &Project, id: &str) -> Result<()> {
    crate::plan::check_attempt_prerequisites(project, id)?;
    require_session(ctx, project)?;
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
    let mut record = thread::load(project, id)?;
    record.provider_wait_started.clear();
    place_recovery(ctx, project, &record)
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

/// Canonical dependency identity also covers old pre-placement records that
/// saved only a label. Renaming a profile must not renew a provider incident.
pub(crate) fn dependency_machine(ctx: &Ctx, machine: &str) -> Result<String> {
    if machine == crate::contracts::MACHINE_LOCAL {
        return Ok(machine.into());
    }
    let profile =
        remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, machine)?;
    Ok(if profile.id.is_empty() {
        profile.label
    } else {
        profile.id
    })
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
fn place_and_brief(ctx: &Ctx, project: &Project, view: &SessionView, id: &str) -> Result<Thread> {
    let record = thread::load(project, id)?;
    prepare_checkout(ctx, project, &record)?;
    let placed = thread::load(project, id)?;
    write_brief(ctx, project, &placed)?;
    bind_terminal(ctx, project, view, &placed)?;
    finish_placement(project, view, id)
}

/// Materializes the frozen, content-addressed brief in the lane's ignored
/// runtime folder. The product repository never tracks it.
fn write_brief(ctx: &Ctx, project: &Project, placed: &Thread) -> Result<()> {
    let machine = if placed.is_remote() {
        Some(remote::declaration_for_route(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            placed.machine_route(),
        )?)
    } else {
        prepare_local_dir(ctx, project, placed)?;
        None
    };
    let mut files = placed
        .attachments
        .iter()
        .map(|(name, hash)| {
            (
                format!("{}/attachments/{name}", placed.thread_dir),
                hash.clone(),
            )
        })
        .collect::<Vec<_>>();
    if placed.kind != Kind::Tab {
        files.push((
            format!("{}/brief.md", placed.thread_dir),
            placed.launch.brief_hash.clone(),
        ));
    }
    for (path, hash) in files {
        let bytes = thread::artifact(project, &hash)?;
        if let Some(machine) = &machine {
            remote::write_runtime_file(ctx.runner, &machine.target, &path, &bytes, &hash)?;
        } else {
            let path = Path::new(&path);
            std::fs::create_dir_all(path.parent().context("runtime file has no parent")?)?;
            project::write_atomic(path, &bytes)?;
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
        remote::write_runtime_file(
            ctx.runner,
            target,
            &format!("{state}/artifacts/{hash}"),
            &thread::artifact(project, &hash)?,
            &hash,
        )?;
    }
    Ok(())
}

/// Freeze paths and bytes at acceptance, before any slow placement effect.
fn freeze_start(ctx: &Ctx, project: &Project, record: &Thread, task: &str) -> Result<Thread> {
    let machine = if record.is_remote() {
        Some(remote::machine_declaration(
            &ctx.config_dir,
            &record.machine,
        )?)
    } else {
        None
    };
    let repo = if record.is_remote() {
        let (settings, _) = project.read_project_md()?;
        box_repo_row(&ctx.config_dir, &settings, &record.machine, &record.repo)?.0
    } else {
        record.repo.clone()
    };
    let folder = format!("{repo}/.worktrees/{}", record.id);
    let mut stub = record.clone();
    stub.thread_dir = thread::thread_dir(&folder, &project.slug, &record.id);
    stub.branch = thread::branch_name(&project.slug, &record.id, &record.title);
    let mut brief = thread::brief_for(project, &stub, task, false)?;
    if record.role == "reviewer"
        && !record.review_id.is_empty()
        && let Some(machine) = machine
    {
        brief = brief.replace(
            &project.state_dir().to_string_lossy().to_string(),
            &format!("{}/{}/.state", machine.root, project.slug),
        );
    }
    let hash = thread::store_artifact(
        project,
        format!("plain: {}\n\n{brief}", record.plain).as_bytes(),
    )?;
    thread::update(project, &record.id, |t| {
        t.thread_dir = stub.thread_dir;
        t.branch = stub.branch;
        t.launch.brief_hash = hash;
        t.recovery_pending = true;
        t.partial = Some("worktree_add".into());
    })
}

/// Checkout creation is independent of terminal placement. A recorded checkout
/// is reused verbatim for retries and parked conversation reopens.
fn prepare_checkout(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if !record.worktree_path.is_empty() && record.partial.as_deref() != Some("worktree_add") {
        if record.thread_dir.is_empty() {
            thread::update(project, &record.id, |t| {
                t.thread_dir = if t.kind == Kind::Tab {
                    t.worktree_path.clone()
                } else {
                    thread::thread_dir(&t.worktree_path, &project.slug, &t.id)
                };
            })?;
        }
        return Ok(());
    }
    if record.kind == Kind::Tab {
        let folder = thread::threads_dir_for_write(project)?.join(&record.id);
        crate::claude_trust::check_folder(ctx, &record.launch.kind, false, &folder)?;
        let task = std::fs::read_to_string(thread::task_path(project, &record.id))?;
        let stub = Thread {
            thread_dir: folder.to_string_lossy().into_owned(),
            ..record.clone()
        };
        let brief = format!(
            "plain: {}\n\n{}",
            record.plain,
            thread::brief_for(project, &stub, &task, false)?
        );
        let (folder, hash, base) = prepare_managed_git_folder(ctx.runner, &folder, &brief)?;
        thread::store_artifact(project, &std::fs::read(folder.join("brief.md"))?)?;
        thread::update(project, &record.id, |t| {
            t.worktree_path = folder.to_string_lossy().into_owned();
            t.thread_dir = t.worktree_path.clone();
            t.branch = "main".into();
            t.base = base;
            t.launch.brief_hash = hash;
        })?;
        return Ok(());
    }
    let path = if record.is_remote() {
        let profile = remote::machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            record.machine_route(),
        )?;
        let machine = remote::machine_declaration(&ctx.config_dir, &profile.label)?;
        let (settings, _) = project.read_project_md()?;
        let (repo, url) = box_repo_row(&ctx.config_dir, &settings, &profile.label, &record.repo)?;
        let path = format!("{repo}/.worktrees/{}", record.id);
        let _lock = project::box_lock(&ctx.root, &profile.id, &repo)?;
        {
            let _lock = crate::git::lock(ctx.runner, &record.repo)?;
            crate::git::exclude_plugin_paths_locked(ctx.runner, &record.repo)?;
            ensure_branch(ctx.runner, &record.repo, &record.branch, &record.base)?;
        }
        push_branch(ctx.runner, &record.repo, &url, &record.branch, &record.base)?;
        remote::provision(
            ctx.runner,
            &profile.target,
            &remote::Provision {
                path: &machine.path,
                box_repo: &repo,
                worktree: &path,
                branch: &record.branch,
                base: &record.base,
                publish_url: &url,
            },
        )?;
        path
    } else {
        let path = format!("{}/.worktrees/{}", record.repo, record.id);
        let _lock = crate::git::lock(ctx.runner, &record.repo)?;
        crate::git::exclude_plugin_paths_locked(ctx.runner, &record.repo)?;
        if Path::new(&path).exists() {
            // A crash may leave an already-created checkout. Verify its git
            // registration and branch; never reset it to the frozen start SHA.
            let listed = crate::repo::Git::new(ctx.runner, &record.repo)
                .with_timeout(GIT_TIMEOUT)
                .run(&["worktree", "list", "--porcelain"])?;
            if !listed.split("\n\n").any(|row| {
                row.lines().any(|line| line == format!("worktree {path}"))
                    && row
                        .lines()
                        .any(|line| line == format!("branch refs/heads/{}", record.branch))
            }) {
                bail!("placement_checkout_mismatch: {path}");
            }
        } else {
            ensure_branch(ctx.runner, &record.repo, &record.branch, &record.base)?;
            crate::repo::Git::new(ctx.runner, &record.repo)
                .with_timeout(GIT_TIMEOUT)
                .run(&["worktree", "add", &path, &record.branch])?;
        }
        path
    };
    thread::update(project, &record.id, |t| {
        t.worktree_path = path;
        t.partial = Some("tab_create".into());
    })?;
    Ok(())
}

/// The one terminal/card binding path, regardless of how placement was queued.
fn bind_terminal(ctx: &Ctx, project: &Project, view: &SessionView, record: &Thread) -> Result<()> {
    let runner = ctx.runner;
    let (settings, _) = project.read_project_md()?;
    let label = project::display_name(&settings.name, &project.slug);
    let machine = if record.is_remote() {
        Some(remote::declaration_for_route(
            runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            record.machine_route(),
        )?)
    } else {
        None
    };
    let herdr = view.herdr.on_machine(record.machine_route());
    let evidence = if crate::launch::execution_requested(&record.launch) {
        crate::launch::execution_evidence(
            project,
            record,
            &machine
                .as_ref()
                .map_or_else(|| ctx.root.display().to_string(), |m| m.root.clone()),
        )?
    } else {
        Vec::new()
    };
    let publication = if machine.is_some() && crate::launch::execution_requested(&record.launch) {
        Some(box_repo_row(
            &ctx.config_dir,
            &settings,
            &record.machine,
            &record.repo,
        )?)
    } else {
        None
    };
    let execution = crate::launch::bind_execution_with_evidence(
        ctx,
        record,
        machine.as_ref(),
        &evidence,
        publication,
    )?;
    let mut spec = crate::contracts::RoleSpec {
        kind: record.launch.kind.clone(),
        args: execution.args.clone(),
        env: record.launch.env.clone(),
        ready_timeout_ms: record.launch.ready_timeout_ms,
    };
    if execution.advisory.is_some() {
        spec.env
            .retain(|value| !value.starts_with("HERDR_ADE_EXECUTION="));
        spec.env.push("HERDR_ADE_EXECUTION=advisory".into());
    }
    let attempt = record.attempt.max(1);
    let env = project::tab_env(
        &project.slug,
        &record.id,
        attempt,
        &record.launch.brief_hash,
        machine.as_ref(),
        &spec,
    );
    // The agent launch supplies its exclusive wrapper PATH after bashrc has
    // run. A pane-level PATH cannot enforce this on an interactive shell.
    // One project owns one workspace on this machine. Starts can provision
    // repositories independently, but find-or-create is serialized so two
    // simultaneous lanes cannot both observe "missing" and create duplicates.
    let folder = Path::new(&record.worktree_path);
    let created = if record.partial.as_deref() == Some("lane_card") && !record.pane_id.is_empty() {
        let pane = herdr
            .pane_list()?
            .into_iter()
            .find(|p| thread::pane_matches(record, p))
            .context("placement_identity_mismatch: bound terminal disappeared")?;
        crate::herdr::Created {
            workspace_id: pane.workspace_id,
            tab_id: pane.tab_id,
            pane_id: pane.pane_id,
        }
    } else if record.is_remote() {
        let _lock =
            project::remote_workspace_lock(&ctx.root, &project.slug, record.machine_route())?;
        let matching: Vec<_> = herdr
            .workspace_list()?
            .into_iter()
            .filter(|w| w.label == label)
            .collect();
        if matching.len() > 1 {
            bail!("remote_workspace_duplicate: multiple workspaces for {label}");
        }
        match matching.first() {
            Some(w) => herdr.tab_create_env(&w.workspace_id, folder, &record.id, false, &env)?,
            None => herdr.workspace_create_env(folder, &label, false, &env)?,
        }
    } else {
        let coord = project
            .coordinator()
            .context("project coordinator missing")?;
        herdr.tab_create_env(&coord.workspace_id, folder, &record.id, false, &env)?
    };
    let cwd = herdr
        .pane_cwd(&created.pane_id)
        .unwrap_or_else(|_| record.worktree_path.clone());
    let cwd = if cwd.is_empty() {
        record.worktree_path.clone()
    } else {
        cwd
    };
    // Persist ownership before any later call can fail. Failed-start cleanup
    // can now close this exact workspace instead of leaking an unrecorded one.
    thread::update(project, &record.id, |t| {
        t.cwd = cwd.clone();
        crate::launch::apply_execution(project, t, &execution);
        t.workspace_id = created.workspace_id.clone();
        t.tab_id = created.tab_id.clone();
        t.pane_id = created.pane_id.clone();
        t.partial = Some("lane_card".into());
    })?;
    if record.is_remote() {
        herdr.tab_rename(&created.tab_id, &format!("{} starting…", record.id))?;
    }

    // The box lane nests from placement onward. Its agent start does not
    // pass --parent; the pane carries the machine-qualified token instead.
    if let Some(coord) = project.coordinator().filter(|_| record.is_remote()) {
        herdr
            .pane_set_parent(&created.pane_id, &parent_token(record, &coord.pane_id))
            .map_err(|error| {
                anyhow::anyhow!(
                    "could not link box pane {} to its coordinator: {error}",
                    created.pane_id
                )
            })?;
    }

    // Step 5: box seals authenticate this exact binding against its card.
    if let Some(machine) = machine {
        let (box_repo, publish_url) =
            box_repo_row(&ctx.config_dir, &settings, &record.machine, &record.repo)?;
        if record.role == "reviewer" && !record.review_id.is_empty() {
            stage_box_review(ctx, project, record, &machine.root, &machine.target)?;
        }
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
        let start_line = thread::launch_prompt(&remote_prefix, &project.slug, record);
        let card = crate::contracts::LaneCard {
            project: project.slug.clone(),
            thread: record.id.clone(),
            attempt,
            brief_hash: record.launch.brief_hash.clone(),
            role: record.role.clone(),
            kind: record.launch.kind.clone(),
            pane_id: created.pane_id.clone(),
            machine_label: record.machine.clone(),
            machine_id: record.machine_id.clone(),
            box_repo: box_repo.clone(),
            box_worktree: record.worktree_path.clone(),
            brief_commit: record.base.clone(),
            paths: record.paths.clone(),
            branch: record.branch.clone(),
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
            &machine.target,
            &project.slug,
            &card_path,
            &toml::to_string(&card)?,
        )?;
    }
    Ok(())
}

/// The integration branch whose exact head becomes the lane's code base:
/// `--base`, else the branch the repository has checked out.
fn integration_branch(runner: &dyn Runner, record: &Thread) -> Result<String> {
    if !record.base.is_empty() {
        return Ok(record.base.clone());
    }
    crate::repo::Git::new(runner, &record.repo).with_timeout(GIT_TIMEOUT).run(&["symbolic-ref", "--short", "HEAD"])
    .context(
        "integration_branch_required: the repository is on a detached HEAD; pass --base <branch>",
    )
}

/// Creates the lane branch at `sha`, tolerating a retry that left it at the
/// same commit. Never moves an existing ref (D9).
fn ensure_branch(runner: &dyn Runner, repo: &str, branch: &str, sha: &str) -> Result<()> {
    if let Some(existing) = crate::repo::Git::new(runner, repo)
        .with_timeout(Duration::from_secs(5))
        .branch_head(branch)?
    {
        if existing == sha {
            return Ok(());
        }
        bail!("lane branch {branch} already exists at {existing}, not {sha}");
    }
    crate::repo::Git::new(runner, repo)
        .with_timeout(GIT_TIMEOUT)
        .run(&["branch", branch, sha])?;
    Ok(())
}

/// Pushes the lane branch by URL, never by remote name and never with force
/// (SPEC-remote §4.2 step 2).
fn push_branch(runner: &dyn Runner, repo: &str, url: &str, branch: &str, sha: &str) -> Result<()> {
    crate::repo::Git::new(runner, repo).run(&[
        "push",
        "--quiet",
        url,
        &format!("{sha}:refs/heads/{branch}"),
    ])?;
    Ok(())
}

/// Makes the project-owned git folder used by a thread with no code
/// repository. Its first commit contains only `brief.md`; report and library
/// deliverables stay untracked and are captured by `done` and retirement.
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
        crate::repo::Git::new(runner, &folder)
            .with_timeout(GIT_TIMEOUT)
            .run(&["for-each-ref", "--format=%(objectname)", "refs/heads/main"])?
            .lines()
            .next()
            .filter(|head| !head.is_empty())
            .map(str::to_string)
    } else {
        None
    };
    if let Some(head) = head {
        let tracked = crate::repo::Git::new(runner, &folder)
            .with_timeout(GIT_TIMEOUT)
            .run(&["ls-tree", "--name-only", "HEAD", "--", "brief.md"])?;
        if tracked != "brief.md" || !brief_path.is_file() {
            bail!(
                "managed_folder_invalid: {} has a first commit without brief.md",
                folder.display()
            );
        }
        let bytes = std::fs::read(&brief_path)?;
        exclude_paths_from_git(runner, &folder_text, &["/report.md", "/library/"])?;
        return Ok((folder, thread::sha256_hex(&bytes), head));
    }

    project::write_atomic(&brief_path, brief.as_bytes())?;
    if !folder.join(".git").is_dir() {
        crate::repo::Git::new(runner, &folder)
            .with_timeout(GIT_TIMEOUT)
            .run(&["init", "-q", "-b", "main"])?;
    }
    let git = crate::repo::Git::new(runner, &folder).with_timeout(GIT_TIMEOUT);
    git.run(&["add", "--", "brief.md"])?;
    git.run(&[
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
    ])?;
    let head = git.run(&["rev-parse", "HEAD"])?;
    exclude_paths_from_git(runner, &folder_text, &["/report.md", "/library/"])?;
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
    exclude_paths_from_git(runner, cwd, &[".herdr-project/"])
}

fn exclude_paths_from_git(runner: &dyn Runner, cwd: &str, patterns: &[&str]) -> Result<()> {
    let Ok(path) = crate::repo::Git::new(runner, cwd)
        .with_timeout(GIT_TIMEOUT)
        .run(&["rev-parse", "--git-path", "info/exclude"])
    else {
        return Ok(()); // not inside a git repository
    };
    let path = Path::new(cwd).join(path);
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<_> = patterns
        .iter()
        .filter(|pattern| !current.lines().any(|line| line.trim() == **pattern))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for pattern in missing {
        text.push_str(pattern);
        text.push('\n');
    }
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
        t.brief_submitted = false;
        t.brief_submitted_at.clear();
        // While retry still names the old pane, the courier can import its
        // receipt again. Placement starts unacknowledged; only an explicit
        // same-attempt parked-session resume retains its conversation.
        if t.bootstrap != "resuming" {
            t.bootstrap.clear();
        }
        t.launch_attempts = 0;
        t.startup_wait_started = project::now();
        t.recovery_pending = false;
        t.partial = None;
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

/// Drive a persisted start, retry or reopen without re-picking or resetting work.
pub fn place_recovery(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if record.kind == Kind::Adopted {
        bail!("an adopted thread is not placed by the binary");
    }
    if !record.provider_wait_started.is_empty() {
        return Ok(());
    }
    crate::plan::check_attempt_prerequisites(project, &record.id)?;
    if record.recovery_pending
        && matches!(
            record.failure_class,
            crate::contracts::FailureClass::Provider
                | crate::contracts::FailureClass::LostConnection
        )
    {
        let machine = if record.is_remote() {
            record.machine_route()
        } else {
            crate::contracts::MACHINE_LOCAL
        };
        let ready = if record.is_remote() {
            box_launch_ready_for(ctx, machine, &record.launch)
        } else {
            crate::doctor::recipe_ready_local(ctx, &record.launch)
        };
        let dependency_machine = dependency_machine(ctx, machine)?;
        crate::adapters::notify_auth(&ctx.root, project, &dependency_machine, &record.launch)?;
        ready?;
    }
    if record.launch_attempts >= thread::MAX_LAUNCH_ATTEMPTS {
        fail_start(
            ctx,
            project,
            &record.id,
            &format!("recovery_placement_exhausted: {}", record.error),
            crate::contracts::FailureClass::Unknown,
            false,
        )?;
        return Ok(());
    }
    thread::update(project, &record.id, |t| t.launch_attempts += 1)?;
    let result = (|| -> Result<()> {
        let view = require_session(ctx, project)?;
        let herdr = view.herdr.on_machine(record.machine_route());
        if !record.tab_id.is_empty() && record.partial.as_deref() != Some("lane_card") {
            // Only this failed/parked attempt is stopped, never an adopted process.
            if let Some(pane) = herdr
                .pane_list()?
                .iter()
                .find(|p| p.pane_id == record.pane_id)
            {
                if pane.workspace_id != record.workspace_id || pane.tab_id != record.tab_id {
                    bail!("recovery_identity_mismatch: old pane was reused");
                }
                close_pane(ctx, project, record)?;
            }
            thread::update(project, &record.id, |t| {
                t.pane_id.clear();
                t.tab_id.clear();
                t.workspace_id.clear();
            })?;
        }
        if record.launch.brief_hash.is_empty() && record.kind == Kind::Worktree {
            // Historical pre-placement records have no frozen brief/base yet.
            let task = std::fs::read_to_string(thread::task_path(project, &record.id))?;
            let integration = integration_branch(ctx.runner, record)?;
            let base = crate::git::rev_parse(ctx.runner, &record.repo, &integration)?;
            let saved = thread::update(project, &record.id, |t| t.base = base)?;
            freeze_start(ctx, project, &saved, &task)?;
        }
        place_and_brief(ctx, project, &view, &record.id)?;
        if record.is_remote() {
            ticker::request_remote_poll(&ctx.root, project, record.machine_route())?;
        }
        Ok(())
    })();
    if let Err(error) = &result {
        let message = format!("{error:#}");
        thread::update(project, &record.id, |t| {
            t.error = message.clone();
            if remote::is_unreachable(&message) || message.starts_with("disk_low:") {
                t.launch_attempts = record.launch_attempts;
            }
        })?;
    }
    result
}

/// Typed recovery result shared by human and JSON rendering.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RetryOutcome {
    pub thread: String,
    pub attempt: u32,
    pub pane_id: String,
    pub recipe: String,
    pub machine: String,
    pub state: RetryState,
    /// The pane was still showing the old startup block when the ready
    /// window had expired. Keep this visible on the recovery result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetryState {
    Queued,
    Starting,
    Delivered,
}

impl RetryOutcome {
    fn from_thread(record: Thread, screen: Option<String>) -> Self {
        let state = if record.recovery_pending {
            RetryState::Queued
        } else if record.brief_submitted {
            RetryState::Delivered
        } else {
            RetryState::Starting
        };
        Self {
            thread: record.id,
            attempt: record.attempt,
            pane_id: if state == RetryState::Queued {
                String::new()
            } else {
                record.pane_id
            },
            recipe: record.launch.recipe_id,
            machine: if record.machine.is_empty() {
                "local".into()
            } else {
                record.machine
            },
            state,
            screen,
        }
    }

    pub fn message(&self) -> String {
        let placement = match self.state {
            RetryState::Queued => format!(
                "queued for placement on {}; its brief is delivered when the agent is ready",
                self.machine
            ),
            RetryState::Starting => format!(
                "in pane {}; startup is pending; its brief is delivered when the agent is ready",
                self.pane_id
            ),
            RetryState::Delivered => format!("in pane {}; its brief was delivered", self.pane_id),
        };
        format!("{} attempt {} is {placement}", self.thread, self.attempt)
    }
}

/// Start the same task as a new bounded recovery attempt. Unlike the removed
/// `restart` command this deliberately replaces a live, blocked, or stuck
/// process. Its durable failure class decides whether recovery stays on the
/// same recipe within its retry budget, or waits for evidence.
pub fn retry(ctx: &Ctx, slug: &str, id: &str, reason: &str) -> Result<RetryOutcome> {
    retry_inner(ctx, slug, id, reason, false)
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
    retry_inner(ctx, slug, id, reason, true)
}

fn retry_inner(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    reason: &str,
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
        ticker::ensure(ctx)?;
        let placed = thread::load(&project, id)?;
        return Ok(RetryOutcome::from_thread(placed, None));
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
        prompt(ctx, slug, id, reason)?;
        return Ok(RetryOutcome::from_thread(thread::load(&project, id)?, None));
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
    let mut launch = if record.launch_attempts == 0 || record.partial.is_some() {
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
        t.status = Status::Starting;
        t.prompt_pending = false;
        t.launch_attempts = 0;
        t.startup_wait_started.clear();
        t.bootstrap.clear();
        t.partial = Some("placement".into());
        t.error.clear();
        t.last_failure = reason.to_string();
        t.cleanup_pending = false;
        t.cleanup_reason.clear();
        t.retirement = None;
        t.recovery_pending = true;
        Ok(())
    })?;

    ticker::ensure(ctx)?;
    let placed = thread::load(&project, id)?;
    Ok(RetryOutcome::from_thread(placed, screen))
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
        t.retirement = None;
        thread::bind_identity(t, &socket, agent, process.clone());
        Ok(())
    })?;
    let coordinator_pane = project.coordinator().map(|c| c.pane_id).unwrap_or_default();
    herdr.pane_set_parent(pane_id, &parent_token(&rebound, &coordinator_pane))?;
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
    cancel_with_retention(ctx, slug, id, reason, false)
}

pub(crate) fn cancel_preserving_checkout(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    reason: &str,
) -> Result<CancelOutcome> {
    cancel_with_retention(ctx, slug, id, reason, true)
}

fn cancel_with_retention(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    reason: &str,
    keep_checkout: bool,
) -> Result<CancelOutcome> {
    let project = Project::load(&ctx.root, slug)?;
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(crate::refusal::error(
            format!("cancel_reason_missing: say why {id} is being stopped"),
            format!("ha thread cancel {slug} {id} --reason \"<why stop>\""),
        ));
    }
    crate::review::require_resolvable(&project, id)?;
    let mut record = thread::load(&project, id)?;
    if record.cancellation_reason.is_empty() {
        record.cancellation_reason = reason.into();
    }
    let recorded_reason = record.cancellation_reason.clone();
    let request = record.retirement_request(RetirementRequest {
        authority: RetirementAuthority::Cancel,
        keep_checkout,
        ..Default::default()
    });
    // Failure here is not cleanup failure: cancellation has not happened.
    // Only after this write succeeds may retirement become best effort.
    begin_retirement(&project, &record, &request, "cancelled")?;
    let outcome = retire(
        ctx,
        &project,
        &record,
        request,
        "cancelled",
        false,
        &mut CleanupViews::default(),
    )
    .unwrap_or_else(|error| retirement_failure(&project, &record, error));
    refresh_plan(ctx, &project);
    Ok(CancelOutcome {
        thread: outcome.thread,
        state: outcome.state,
        reason: recorded_reason,
        pane: outcome.pane,
        worktree: outcome.worktree,
        worktree_reason: outcome.worktree_reason,
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
    let record = thread::load(project, id).unwrap_or_else(|_| Thread {
        id: id.into(),
        ..Default::default()
    });
    let request = record.retirement.clone().unwrap_or(RetirementRequest {
        authority: if record.cancellation_reason.is_empty() {
            RetirementAuthority::Resolve
        } else {
            RetirementAuthority::Cancel
        },
        ..Default::default()
    });
    let outcome = retire(ctx, project, &record, request, reason, true, views)
        .unwrap_or_else(|error| retirement_failure(project, &record, error));
    refresh_plan(ctx, project);
    outcome
}

/// Retry a bounded slice of durable cleanup, including after a landed pile.
pub(crate) fn retry_pending_cleanup(ctx: &Ctx, project: &Project) -> Result<()> {
    let mut views = CleanupViews::default();
    let mut pending = thread::list_live(project)
        .into_iter()
        .filter(|record| record.cleanup_pending)
        .collect::<Vec<_>>();
    // Every attempt updates the thread. Oldest first prevents an unreachable
    // member at the front of the pile from starving the rest of the queue.
    pending.sort_by(|a, b| a.updated.cmp(&b.updated).then(a.id.cmp(&b.id)));
    for record in pending.into_iter().take(CLEANUP_BATCH_SIZE) {
        // A moved-tip refusal can become resolvable after a reviewed box seal
        // lands; let the pinned branch checks decide on each retry.
        let reason = if record.resolved_reason.is_empty() {
            "automatic"
        } else {
            &record.resolved_reason
        };
        resolve_automatically_with_views(ctx, project, &record.id, reason, &mut views);
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

/// Enter steers pi at its next tool boundary. Other adapters have not proven
/// that contract, so keep their working-turn input in the durable queue.
pub(crate) fn can_steer(record: &Thread, state: &str) -> bool {
    let kind = if record.launch.kind.is_empty() {
        &record.agent
    } else {
        &record.launch.kind
    };
    state == "working" && kind == "pi"
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
        // A successful old submission can arrive after recovery carried its
        // uncertain instruction. Settle that receipt only before resubmission.
        if saved.carried_from_attempt == follow_up.attempt
            && saved.state == FollowUpState::Queued
            && saved.text == follow_up.text
        {
            saved.attempt = follow_up.attempt;
            saved.carried_from_attempt = 0;
            saved.state = FollowUpState::Uncertain;
        }
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
            if thread.error.starts_with("connection_resume_uncertain:") {
                // Recovery shares delivery receipts, but retains its rolling
                // resume budget and dependency evidence until activity returns.
                thread.error.clear();
                thread.connection_resumes.push(project::now());
            } else {
                thread.connection_resumes.clear();
                thread.failure_class = crate::contracts::FailureClass::Unknown;
                thread.provider_failure_kind = None;
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// Herdr's PTY/activity errors may follow submission. An exec failure or an
/// explicit pre-submission refusal proves no prompt was written and permits
/// another delivery attempt.
pub(crate) fn prompt_refused_before_submission(error: &crate::herdr::HerdrError) -> bool {
    matches!(
        error.code.as_str(),
        "agent_not_ready"
            | "agent_blocked"
            | "agent_not_found"
            | "empty_agent_prompt"
            | "exec_failed"
    )
}

pub fn prompt(ctx: &Ctx, slug: &str, id: &str, text: &str) -> Result<PromptOutcome> {
    send_lane_input(ctx, &Project::load(&ctx.root, slug)?, id, Some(text), None)
}

/// Queue new input or drain the oldest queued input. CLI, recovery and ticker
/// share ordering, reopen decisions, transport and delivery receipts.
pub(crate) fn send_lane_input(
    ctx: &Ctx,
    project: &Project,
    id: &str,
    text: Option<&str>,
    observed: Option<(&Herdr<'_>, &[Agent], &Thread)>,
) -> Result<PromptOutcome> {
    let text = text.map(str::trim);
    if text == Some("") {
        bail!("the text is empty");
    }
    let _prompt_lock = thread::prompt_lock(project, id)?;
    let mut record = thread::load(project, id)?;
    let attempt = record.attempt.max(1);
    if text.is_none()
        && (!awaiting_follow_up(&record)
            || observed.is_some_and(|(_, _, lane)| {
                lane.attempt != record.attempt || lane.pane_id != record.pane_id
            }))
    {
        return Ok(PromptOutcome::Queued { attempt });
    }
    if text.is_some() {
        match record.status {
            Status::Resolved => {
                return Err(crate::refusal::error(
                    format!("{id} is resolved"),
                    format!("ha thread show {} {id}", project.slug),
                ));
            }
            Status::Failed => {
                return Err(crate::refusal::error(
                    format!("{id} is gone"),
                    format!(
                        "ha thread retry {} {id} --reason \"<why replace attempt>\"",
                        project.slug
                    ),
                ));
            }
            Status::Starting | Status::Open => {}
        }
    }
    let reopening = if text.is_some() && record.parked {
        require_session(ctx, project)?;
        ticker::ensure(ctx)?;
        Some(parked_session_available(ctx, &record)?)
    } else {
        None
    };
    let queued = record.parked
        || record.status != Status::Open
        || record.prompt_pending
        || awaiting_bootstrap(&record);
    // Preserve CLI refusals before staging when this input can be sent now.
    // A ticker drain uses the same check; it never types at a bare shell.
    let view = if observed.is_none() && !queued && (text.is_none() || !awaiting_follow_up(&record))
    {
        Some(require_session(ctx, project)?)
    } else {
        None
    };
    let kind = if record.launch.kind.is_empty() {
        &record.agent
    } else {
        &record.launch.kind
    };
    let resumable = crate::adapters::declaration(&ctx.config_dir, kind)
        .is_ok_and(|adapter| adapter.blocked_error_resumable);
    let state = if let Some((_, agents, _)) = observed {
        prompt_state(&record, agents, resumable)?
    } else if let Some(view) = &view {
        let (agents, _) = lists_for(view, &record)?;
        prompt_state(&record, &agents, resumable)?
    } else {
        String::new()
    };
    if let Some(text) = text {
        let events = crate::events::checked(project)?;
        record = thread::update_checked(project, id, |current| {
            if current.status != record.status
                || current.attempt != record.attempt
                || current.pane_id != record.pane_id
                || current.parked != record.parked
            {
                bail!("prompt_attempt_changed: {id} changed during prompt preparation");
            }
            // The project lock also covers review membership allocation. A
            // correction holds the old seal and queues its input in ONE write.
            crate::review::require_follow_up(project, id)?;
            current.review_after = crate::events::latest_done_event(&events, id, attempt)
                .map(|event| event.id.clone())
                .unwrap_or_default();
            current.last_group = Group::Working.token().into();
            if let Some(resuming) = reopening {
                reopen_parked(current, resuming);
            }
            current.follow_ups.push(FollowUp {
                attempt,
                text: if reopening == Some(false) {
                    reopening_prompt(&record, text)
                } else {
                    text.into()
                },
                state: FollowUpState::Queued,
                waiting_event: latest_waiting_event_id(&events, id, attempt).unwrap_or_default(),
                queued_at: project::now(),
                ..FollowUp::default()
            });
            Ok(())
        })?;
        if queued || (view.is_none() && observed.is_none()) {
            return Ok(PromptOutcome::Queued { attempt });
        }
    }
    if queued || (state == "working" && !can_steer(&record, &state)) {
        return Ok(PromptOutcome::Queued { attempt });
    }
    let Some((index, follow_up)) = record
        .follow_ups
        .iter()
        .enumerate()
        .find(|(_, f)| {
            f.attempt == attempt
                && matches!(f.state, FollowUpState::Queued | FollowUpState::Uncertain)
        })
        .map(|(index, f)| (index, f.clone()))
    else {
        return Ok(PromptOutcome::Queued { attempt });
    };
    if follow_up.state == FollowUpState::Uncertain {
        return Ok(PromptOutcome::Queued { attempt });
    }
    sync_box_corrections(ctx, project, &record)?;
    thread::update_checked(project, id, |current| {
        if current.status != Status::Open
            || current.attempt != record.attempt
            || current.pane_id != record.pane_id
            || current.prompt_pending
            || awaiting_bootstrap(current)
            || current.follow_ups.get(index) != Some(&follow_up)
        {
            bail!("queued follow-up changed before delivery");
        }
        current.follow_ups[index].state = FollowUpState::Uncertain;
        Ok(())
    })?;
    let after_seal =
        crate::events::latest_done_event(&crate::events::for_thread(project, id), id, attempt)
            .map(|event| event.id.clone())
            .unwrap_or_default();
    let herdr = if let Some((herdr, _, _)) = observed {
        herdr.on_machine(record.machine_route())
    } else {
        view.as_ref()
            .expect("live delivery view")
            .herdr
            .on_machine(record.machine_route())
    };
    let steering = can_steer(&record, &state);
    let result = if steering {
        // The API acknowledges only after the paste AND encoded Enter have
        // been written. Waiting for "working" here would merely observe the
        // already-active turn, not strengthen that submission receipt.
        herdr.agent_prompt(&record.pane_id, &follow_up.text)
    } else if state == "blocked" {
        herdr.pane_submit_text(&record.pane_id, &follow_up.text)
    } else {
        herdr.agent_prompt_wait_started(
            &record.pane_id,
            &follow_up.text,
            thread::agent_start_timeout(&record.launch)
                .min(crate::herdr::AGENT_START_TIMEOUT.as_millis() as u64),
        )
    };
    if let Err(error) = result {
        if prompt_refused_before_submission(&error) {
            thread::update(project, id, |current| {
                if let Some(saved) = current.follow_ups.get_mut(index)
                    && saved.attempt == attempt
                    && saved.text == follow_up.text
                    && saved.state == FollowUpState::Uncertain
                {
                    saved.state = FollowUpState::Queued;
                }
            })?;
        } else if !steering {
            let _ = crate::inbox::write(
                project,
                "prompt-uncertain",
                id,
                &format!(
                    "{id} attempt {attempt} may have received a follow-up; check the lane before sending it again"
                ),
                "",
            );
        }
        if steering {
            let _ = crate::inbox::write(
                project,
                "steering-queued",
                id,
                &format!(
                    "{id} attempt {attempt}: steering delivery unconfirmed; the note remains queued ({error})"
                ),
                "",
            );
            return Ok(PromptOutcome::Queued { attempt });
        }
        return Err(anyhow::anyhow!("{error}"));
    }
    record_follow_up_delivery(project, id, index, &follow_up, &after_seal)?;
    if state == "blocked" {
        thread::update(project, id, |current| {
            if current.attempt.max(1) == attempt {
                current.error.clear();
            }
        })?;
    }
    Ok(PromptOutcome::Sent {
        attempt,
        agent_state: state,
    })
}

/// Called under review allocation's project lock. Its earlier pile snapshot
/// may predate a correction queued during git preparation.
pub(crate) fn retain_current_pile_members(
    project: &Project,
    members: &mut Vec<crate::review::Member>,
) -> Result<()> {
    let events = crate::events::checked(project)?;
    let mut current = Vec::new();
    for member in members.drain(..) {
        let lane = thread::load(project, &member.thread)?;
        if lane.status != Status::Resolved
            && lane.attempt.max(1) == member.attempt
            && lane.merged_sha.is_empty()
            && crate::review::sealed(&events, &lane).is_some_and(|event| event.id == member.event)
        {
            current.push(member);
        }
    }
    *members = current;
    Ok(())
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
    if let Some(outcome) = crate::task::attest_finished(ctx, &project, &record, reason)? {
        return Ok(outcome);
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
        usage: None,
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
    let mut views = CleanupViews::default();
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    if args.reopen {
        if record.status != Status::Resolved {
            bail!("{id} is not resolved");
        }
        let reopened = thread::update(&project, id, |t| {
            t.status = Status::Open;
            t.resolved_reason.clear();
            t.cleanup_pending = false;
            t.cleanup_reason.clear();
            t.retirement = None;
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
    let request = record.retirement_request(RetirementRequest {
        skip_copy: args.skip_copy,
        discard_uncopied: args.discard_uncopied,
        keep_pane: args.keep_pane,
        ..Default::default()
    });
    let outcome = retire(ctx, &project, &record, request, "manual", false, &mut views);
    refresh_plan(ctx, &project);
    outcome
}

fn begin_retirement(
    project: &Project,
    record: &Thread,
    request: &RetirementRequest,
    reason: &str,
) -> Result<Thread> {
    thread::update(project, &record.id, |t| {
        if t.cancellation_reason.is_empty() {
            t.cancellation_reason = record.cancellation_reason.clone();
        }
        t.status = Status::Resolved;
        t.resolved_reason = reason.into();
        t.prompt_pending = false;
        t.cleanup_pending = true;
        t.cleanup_reason.clear();
        t.retirement = Some(request.clone());
    })
}

/// One obligation, from preservation and authority checks through the last
/// scratch/build effect. Intentional retention exempts checkout/ref deletion;
/// failed preservation or any owed external effect leaves the obligation live.
fn retire(
    ctx: &Ctx,
    project: &Project,
    before: &Thread,
    mut request: RetirementRequest,
    reason: &str,
    durable: bool,
    views: &mut CleanupViews,
) -> Result<ResolveOutcome> {
    let record = before.clone();
    let id = record.id.as_str();
    let attempted = (|| -> Result<ResolveOutcome> {
        if request.authority == RetirementAuthority::Resolve {
            let events = crate::events::for_thread(project, id);
            if follow_up_pending_for_seal(
                &record,
                crate::events::latest_done_event(&events, id, record.attempt.max(1)),
            ) {
                bail!(
                    "follow_up_pending: {id} must finish the queued follow-up and seal again before resolution"
                );
            }
        }
        if request.authority != RetirementAuthority::Retained {
            crate::review::require_resolvable(project, id)?;
        }
        // Authorization must precede the terminal transition: marking a lane
        // resolved closes its follow-ups, which must not authorize a retry.
        if durable {
            begin_retirement(project, &record, &request, reason)?;
        }
        let removable = removable_folder(project, &record);
        let already_removed = removable && !worktree_exists(ctx, project, &record)?;

        // After deletion, resume from the saved preservation receipt rather
        // than trying to read links from the now-absent checkout.
        let preservation_complete =
            request.preserved && (already_removed || record.worktree_path.is_empty());
        let mut removal_refusal = request
            .keep_checkout
            .then(|| "diagnostic retention: checkout and ref kept".to_string());
        let (mut final_copy, mut copy_notes) = if preservation_complete && !request.skip_copy {
            ("complete".to_string(), Vec::new())
        } else if request.skip_copy {
            ("skipped".to_string(), Vec::new())
        } else {
            let copied = final_copy(ctx, project, &record);
            match copied.outcome {
                CopyOutcome::Complete => ("complete".to_string(), Vec::new()),
                CopyOutcome::Partial(notes) => {
                    if removable
                        && !already_removed
                        && !request.discard_uncopied
                        && request.authority != RetirementAuthority::Retained
                    {
                        removal_refusal = Some(
                        "copy_incomplete: the worktree was kept because some files were not copied; pass --discard-uncopied to accept that loss"
                            .to_string(),
                    );
                    }
                    ("partial".to_string(), notes)
                }
                CopyOutcome::Failed(error) if request.keep_checkout => {
                    // Preservation is retryable; it must not keep a cancelled
                    // agent running. Its untouched checkout is still evidence.
                    (
                        "pending".into(),
                        vec![format!("final copy pending: {error}")],
                    )
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
        let preserved = thread::load(project, id)?;
        let preservation_pending = if !preservation_complete
            && let Err(error) = preserve_report_links(ctx, project, &preserved)
        {
            let detail = format!("linked_files_not_kept: {error:#}");
            copy_notes.push(detail.clone());
            final_copy = "partial".into();
            removal_refusal = Some(detail);
            true
        } else {
            final_copy == "pending"
        };

        if removable && !already_removed {
            if request.keep_pane && removal_refusal.is_none() {
                removal_refusal = Some(
                "worktree_in_use: the worktree was kept because --keep-pane leaves its pane open"
                    .into(),
            );
            }
            if removal_refusal.is_none() && request.authority != RetirementAuthority::Cancel {
                removal_refusal = finished_worktree_reason(ctx, project, &record)?;
                if request.authority == RetirementAuthority::Retained
                    && let Some(detail) = &removal_refusal
                {
                    bail!("{id} is not finished; not removing its worktree: {detail}");
                }
            }
            if removal_refusal.is_none() {
                let inspection = inspect_worktree_for_removal(ctx, project, &record)?;
                if !inspection.dirty.is_empty() {
                    let detail = format!(
                        "worktree_dirty: uncommitted changes in {}; not removing ({})",
                        record.worktree_path,
                        inspection.dirty.join(", ")
                    );
                    if request.authority != RetirementAuthority::Cancel {
                        return Err(crate::refusal::error(
                            detail,
                            format!("ha thread show {} {id}", project.slug),
                        ));
                    }
                    removal_refusal = Some(detail);
                } else if request.authority != RetirementAuthority::Retained {
                    removal_refusal = inspection.ignored_reason(&record.worktree_path);
                }
            }
            if removal_refusal.is_none() {
                if request.authority == RetirementAuthority::Retained {
                    let tip = crate::branches::require_published_tip(ctx, project, &record)?;
                    if !request.retained_tip.is_empty() && request.retained_tip != tip {
                        bail!("{id} moved beyond its retained-removal pin");
                    }
                    request.retained_tip = tip;
                }
                removal_in_use_gate(
                    ctx,
                    project,
                    &record,
                    views,
                    request.authority == RetirementAuthority::Cancel,
                )?;
            }
        }
        request.preserved = !preservation_pending && removal_refusal.is_none();
        let resolved = begin_retirement(project, &record, &request, reason)?;
        if let Ok(Some(view)) = views.for_thread(ctx, project, &resolved) {
            clear_thread_tokens(&view.herdr, &resolved);
        }
        let pane = if request.keep_pane {
            "kept_open"
        } else if close_pane_with_views(ctx, project, &resolved, views)? {
            "closed"
        } else {
            "already_gone"
        };
        if removable && (already_removed || removal_refusal.is_none()) {
            if !already_removed {
                if request.authority == RetirementAuthority::Retained {
                    remove_worktree_force_ignored(ctx, project, &resolved)?;
                } else {
                    remove_worktree(ctx, project, &resolved)?;
                }
            }
            thread::update(project, id, |t| t.worktree_path.clear())?;
        }
        if !request.keep_checkout && (already_removed || removal_refusal.is_none()) {
            crate::branches::resolved_thread(ctx, project, &resolved)?;
        }
        if !request.keep_checkout {
            remove_finished_build_folder(ctx, project, &resolved)?;
            remove_scratch_session(ctx, &resolved)?;
        }
        thread::update(project, id, |t| {
            t.cleanup_pending = preservation_pending;
            t.cleanup_reason = if preservation_pending {
                removal_refusal.clone().unwrap_or_default()
            } else {
                String::new()
            };
            if !preservation_pending && !request.keep_checkout {
                t.retirement = None;
            }
        })?;
        Ok(retirement_outcome(
            project,
            &record,
            final_copy,
            copy_notes,
            pane,
            if already_removed {
                None
            } else {
                removal_refusal
            },
        ))
    })();
    if let Err(error) = &attempted {
        let _ = thread::update(project, id, |t| {
            if t.cleanup_pending {
                t.cleanup_reason = format!("{error:#}");
            }
        });
    }
    attempted
}

fn retirement_outcome(
    project: &Project,
    before: &Thread,
    final_copy: String,
    copy_notes: Vec<String>,
    pane: &str,
    worktree_reason: Option<String>,
) -> ResolveOutcome {
    let record = thread::load(project, &before.id).unwrap_or_else(|_| Thread {
        cleanup_pending: true,
        ..before.clone()
    });
    let removed = removable_folder(project, before) && record.worktree_path.is_empty();
    ResolveOutcome {
        thread: record.id.clone(),
        state: if record.cleanup_pending {
            "cleanup_pending"
        } else {
            match record.status {
                Status::Resolved if !record.cancellation_reason.is_empty() => "cancelled",
                Status::Resolved => "resolved",
                Status::Open => "open",
                Status::Failed => "failed",
                Status::Starting => "starting",
            }
        }
        .into(),
        final_copy,
        copy_notes,
        pane: pane.into(),
        worktree: if removed {
            "removed"
        } else if worktree_reason.is_some() && !record.worktree_path.is_empty() {
            "kept"
        } else if record.kind == Kind::Worktree || managed_git_folder(project, &record) {
            "not_recorded"
        } else {
            "not_applicable"
        }
        .into(),
        worktree_path: if removed {
            before.worktree_path.clone()
        } else {
            record.worktree_path
        },
        worktree_reason,
        branch: record.branch,
    }
}

fn retirement_failure(project: &Project, before: &Thread, error: anyhow::Error) -> ResolveOutcome {
    let detail = format!("{error:#}");
    retirement_outcome(
        project,
        before,
        "pending".into(),
        vec![detail.clone()],
        "cleanup_pending",
        Some(detail.clone()),
    )
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

pub(crate) fn retry_command(slug: &str, id: &str) -> String {
    format!(
        "ha thread retry {} {} --reason \"retry failed startup\"",
        remote::quote(slug),
        remote::quote(id)
    )
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
                || t.parked
                || t.recovery_pending
                || (class != crate::contracts::FailureClass::ProcessGone
                    && !t.report_hash.is_empty())
        }) {
            return Ok(());
        }
        if expected.is_some() && attempt_sealed(project, t) {
            return Ok(());
        }
        matched = true;
        let next = if recovery.is_some() {
            format!(
                "automatic same-recipe retry selected for attempt {}; wait for startup",
                t.attempt.max(1).saturating_add(1)
            )
        } else {
            retry_command(&project.slug, id)
        };
        let gone = class == crate::contracts::FailureClass::ProcessGone
            && !t.parked
            && !attempt_sealed(project, t);
        if gone || recovery.is_none() {
            let reason = crate::steps::short_error(
                &recovery_error.clone().unwrap_or_else(|| reason.to_string()),
            );
            let label = if gone {
                format!("GONE {id} attempt {}", t.attempt.max(1))
            } else {
                format!("FAILED {id}")
            };
            t.start_notices.push(crate::steps::Notice {
                line: format!("{label}: {reason} — next: {next}"),
                submitted: false,
            });
        }
        t.status = Status::Failed;
        t.prompt_pending = false;
        t.startup_wait_started.clear();
        t.provider_wait_started.clear();
        t.error = recovery_error.clone().unwrap_or_else(|| reason.to_string());
        t.failure_class = class;
        t.provider_failure_kind = provider_kind.clone();
        t.recovery_pending = recovery.is_some();
        if let Some(mut selected) = recovery.clone() {
            t.attempt = t.attempt.max(1).saturating_add(1);
            selected.attempt = t.attempt;
            selected.brief_hash = t.launch.brief_hash.clone();
            t.launch = selected;
            // Placement gets its own bounded counter for the selected attempt.
            t.launch_attempts = 0;
            t.partial = Some("placement".into());
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

/// Pi reports a session file on the machine that owns the lane. Never ask pi
/// to resume an absent file (or an old opaque id); that can exit before ready.
fn parked_session_available(ctx: &Ctx, record: &Thread) -> Result<bool> {
    let Some(session) = record
        .identity
        .agent_session
        .as_deref()
        .filter(|s| !s.is_empty())
    else {
        return Ok(false);
    };
    if crate::adapters::resume_args(&record.launch, Some(session)).is_none() {
        return Ok(false);
    }
    if record.launch.kind != "pi" {
        return Ok(true);
    }
    // Herdr's pi identity is an absolute path. Historical ids have no proven
    // file on this machine, so restart with the preserved brief and correction.
    if !Path::new(session).is_absolute() {
        return Ok(false);
    }
    if !record.is_remote() {
        return Ok(Path::new(session).is_file());
    }
    let profile = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?;
    let out = remote::ssh(
        ctx.runner,
        &profile.target,
        &format!(
            "if test -f {}; then printf 'present'; else printf 'missing'; fi",
            remote::quote(session)
        ),
        None,
        Duration::from_secs(20),
    )?;
    if !out.success() {
        bail!(
            "resume_session_check: {}: {}",
            record.machine,
            out.error_text()
        );
    }
    match out.stdout.trim() {
        "present" => Ok(true),
        "missing" => Ok(false),
        other => bail!(
            "resume_session_check: {}: unexpected reply {other:?}",
            record.machine
        ),
    }
}

fn reopening_prompt(record: &Thread, text: &str) -> String {
    format!(
        "Read your frozen brief at {}/brief.md and sealed report at {}. Continue in the same folder.\n\n{}",
        record.thread_dir,
        record.report_path(),
        text
    )
}

/// Recheck at submission too: placement may have waited since the reopen.
/// Connection/observation errors leave the same queued attempt untouched.
pub(crate) fn resume_launch_record(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
) -> Result<Thread> {
    if record.bootstrap != "resuming"
        || record.launch.kind != "pi"
        || parked_session_available(ctx, record)?
    {
        return Ok(record.clone());
    }
    thread::update(project, &record.id, |current| {
        if current.attempt != record.attempt
            || current.pane_id != record.pane_id
            || current.launch_attempts > 0
            || current.bootstrap != "resuming"
        {
            return;
        }
        current.bootstrap.clear();
        for follow_up in &mut current.follow_ups {
            if follow_up.attempt == current.attempt.max(1)
                && follow_up.state == FollowUpState::Queued
            {
                follow_up.text = reopening_prompt(record, &follow_up.text);
            }
        }
    })
}

/// Bring a completed lane back without provisioning its branch or replacing
/// its frozen task. The old agent session id is kept across the pane close.
fn reopen_parked(t: &mut Thread, resuming: bool) {
    t.parked = false;
    t.status = Status::Starting;
    t.recovery_pending = true;
    t.launch_attempts = 0;
    t.startup_wait_started.clear();
    t.identity.process = None;
    t.brief_submitted = false;
    t.brief_submitted_at.clear();
    t.partial = Some("placement".into());
    t.prompt_pending = false;
    // Readiness of the new process, not reopening, earns a delivery receipt.
    t.bootstrap = if resuming {
        "resuming".into()
    } else {
        String::new()
    };
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
    use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};

    let parser = Parser::new(report);
    let mut found = Vec::new();
    // Definitions have their own spans, including unused definitions. Rewrite
    // each once, not every reference use. Ignore escaped closing label brackets.
    for (_, def) in parser.reference_definitions().iter() {
        let source = &report[def.span.clone()];
        let label_end = source
            .match_indices("]:")
            .find(|(i, _)| (i - source[..*i].trim_end_matches('\\').len()).is_multiple_of(2))
            .unwrap()
            .0;
        found.push((
            markdown_destination_range(report, def.span.start, def.span.start + label_end + 2),
            def.dest.to_string(),
        ));
    }
    let mut inline = Vec::new();
    let mut child_end = 0;
    // Feed only parser-recognized HTML to the attribute tokenizer, retaining
    // byte offsets and continuity across multiline HTML blocks.
    let mut html = vec![b' '; report.len()];
    for (event, span) in parser.into_offset_iter() {
        match event {
            Event::Start(
                Tag::Link {
                    link_type: LinkType::Inline,
                    dest_url,
                    ..
                }
                | Tag::Image {
                    link_type: LinkType::Inline,
                    dest_url,
                    ..
                },
            ) => {
                child_end = span.start;
                inline.push((span, dest_url));
                continue;
            }
            Event::End(TagEnd::Link | TagEnd::Image)
                if inline.last().is_some_and(|(s, _)| *s == span) =>
            {
                let (_, dest) = inline.pop().unwrap();
                // Child spans end before the closing label bracket; titles and
                // code in labels cannot be mistaken for the destination.
                let start = child_end + report[child_end..span.end].find("](").unwrap() + 2;
                found.push((
                    markdown_destination_range(report, span.start, start),
                    dest.to_string(),
                ));
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                let mut offset = span.start;
                // Owned HTML has container prefixes removed. Each emitted
                // line remains the exact suffix of its original source line.
                for (source, line) in report[span.clone()]
                    .split_inclusive('\n')
                    .zip(text.split_inclusive('\n'))
                {
                    html[offset + source.len() - line.len()..offset + source.len()]
                        .copy_from_slice(line.as_bytes());
                    offset += source.len();
                }
            }
            _ => (),
        }
        child_end = span.end;
    }
    html_destinations(std::str::from_utf8(&html).unwrap(), &mut found);
    found.sort_by_key(|(range, _)| range.start);
    found
}

/// Locate only the URL token of a parser-validated link, keeping its title and
/// delimiters intact. Recognition, code exclusion and decoding belong to Parser.
fn markdown_destination_range(
    report: &str,
    source_start: usize,
    mut start: usize,
) -> std::ops::Range<usize> {
    let bytes = report.as_bytes();
    // Skip parser-validated blockquote prefixes on continuation lines.
    let line_start = report[..source_start].rfind('\n').map_or(0, |i| i + 1);
    let quotes = report[line_start..source_start].matches('>').count();
    while bytes.get(start).is_some_and(u8::is_ascii_whitespace) {
        start += 1;
        if bytes[start - 1] == b'\n' {
            for _ in 0..quotes {
                while matches!(bytes.get(start), Some(b' ' | b'\t')) {
                    start += 1;
                }
                start += usize::from(bytes.get(start) == Some(&b'>'));
            }
        }
    }
    let angle = bytes.get(start) == Some(&b'<');
    start += usize::from(angle);
    let mut end = start;
    let mut depth = 0;
    while let Some(&b) = bytes.get(end) {
        match b {
            b'\\' if bytes.get(end + 1).is_some_and(u8::is_ascii_punctuation) => end += 1,
            b'>' if angle => break,
            b'(' if !angle => depth += 1,
            b')' if !angle && depth == 0 => break,
            b')' if !angle => depth -= 1,
            b if !angle && b.is_ascii_whitespace() => break,
            _ => (),
        }
        end += 1;
    }
    start..end
}

fn html_destinations(report: &str, found: &mut Vec<(std::ops::Range<usize>, String)>) {
    let mut emitter = html5gum::DefaultEmitter::<usize>::new_with_span();
    emitter.naively_switch_states(true);
    for token in html5gum::Tokenizer::new_with_emitter(report, emitter).flatten() {
        let html5gum::Token::StartTag(tag) = token else {
            continue;
        };
        for (name, value) in tag.attributes {
            if !matches!(name.as_slice(), b"src" | b"href" | b"srcset") {
                continue;
            }
            // Attribute spans include the name and one trailing delimiter
            // (closing quote or unquoted separator), not just the decoded URL.
            let end = value.span.end - 1;
            let Some((_, raw)) = report[value.span.start..end].split_once('=') else {
                continue;
            };
            let raw = raw.trim_ascii_start();
            let raw = raw.strip_prefix(['\'', '"']).unwrap_or(raw);
            let range = end - raw.len()..end;
            if name.as_slice() != b"srcset" {
                found.push((range, String::from_utf8_lossy(&value).into_owned()));
            } else {
                // Keep raw URL offsets; descriptors are not paths and data
                // URLs may contain commas. Decode each candidate as HTML.
                let mut rest = &report[range.clone()];
                loop {
                    rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
                    if rest.is_empty() {
                        break;
                    }
                    let data = rest.starts_with("data:");
                    let len = rest
                        .find(|c: char| c.is_ascii_whitespace() || (!data && c == ','))
                        .unwrap_or(rest.len());
                    let start = range.end - rest.len();
                    let candidate = format!("<img src='{}'>", rest[..len].replace('\'', "&#39;"));
                    let mut decoded = Vec::new();
                    html_destinations(&candidate, &mut decoded);
                    found.push((start..start + len, decoded.pop().unwrap().1));
                    rest = rest[len..].split_once(',').map_or("", |(_, tail)| tail);
                }
            }
        }
    }
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

fn linked_read<T: serde::de::DeserializeOwned>(
    ctx: &Ctx,
    record: &Thread,
    request: crate::box_helper::Request,
) -> Result<T> {
    if !record.is_remote() {
        return crate::box_helper::local(ctx, request);
    }
    let profile = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        record.machine_route(),
    )?;
    crate::box_helper::call(
        ctx.runner,
        &profile.target,
        &remote::machine_declaration(&ctx.config_dir, &profile.label)?,
        request,
        Duration::from_secs(90),
        None,
    )
}

fn linked_file_missing(ctx: &Ctx, record: &Thread, relative: &std::path::Path) -> Result<bool> {
    linked_read(
        ctx,
        record,
        crate::box_helper::Request::Missing {
            root: record.thread_dir.clone().into(),
            relative: relative.into(),
        },
    )
}

pub(crate) struct LinkedFiles {
    pub(crate) directory: bool,
    pub(crate) files: std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
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

pub(crate) fn checked_link_path(root: &Path, path: &Path) -> Result<std::fs::Metadata> {
    let relative = path
        .strip_prefix(root)
        .context("linked path is not inside the thread folder")?;
    anyhow::ensure!(
        relative
            .components()
            .all(|p| matches!(p, std::path::Component::Normal(_))),
        "linked path is not inside the thread folder"
    );
    let info = std::fs::symlink_metadata(path)
        .with_context(|| format!("linked path missing or unreadable: {}", path.display()))?;
    for ancestor in path.ancestors().take_while(|p| *p != root) {
        anyhow::ensure!(
            !std::fs::symlink_metadata(ancestor)?
                .file_type()
                .is_symlink(),
            "linked path is not a regular file or folder (symlink): {}",
            ancestor.display()
        );
    }
    anyhow::ensure!(
        path.canonicalize()?.starts_with(root.canonicalize()?),
        "linked path is not inside the thread folder: {}",
        path.display()
    );
    Ok(info)
}

// Both hosts use this walk; only the bounded, hashed transport differs.
pub(crate) fn read_linked(
    root: &Path,
    relative: &Path,
    offset: Option<u64>,
) -> Result<LinkedFiles> {
    let path = root.join(relative);
    {
        let mut pending = vec![path.clone()];
        let mut paths = Vec::new();
        let mut total = 0_u64;
        let mut directory = false;
        while let Some(item) = pending.pop() {
            let info = checked_link_path(root, &item)?;
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
                paths.push((item, info.len()));
            } else {
                bail!(
                    "linked path is not a regular file or folder: {}",
                    item.display()
                );
            }
        }
        paths.sort();
        let mut files = std::collections::BTreeMap::new();
        let mut read_total = 0_u64;
        let mut skip = offset.unwrap_or(0);
        for (item, size) in paths {
            // Chunking slices the same sorted inventory; no per-file SSH trip.
            use std::io::{Read, Seek};
            if offset.is_some() && skip >= size {
                skip -= size;
                continue;
            }
            checked_link_path(root, &item)?;
            let mut source = std::fs::File::open(&item)?;
            source.seek(std::io::SeekFrom::Start(skip))?;
            let limit = if offset.is_some() {
                (size - skip).min(crate::box_helper::CHUNK as u64 - read_total)
            } else {
                LINKED_FILES_CAP - read_total + 1
            };
            let mut bytes = Vec::new();
            source.take(limit).read_to_end(&mut bytes)?;
            skip = 0;
            read_total += bytes.len() as u64;
            if read_total > LINKED_FILES_CAP {
                bail!("linked files over cap (200 MiB); worktree kept");
            }
            files.insert(item.strip_prefix(root)?.to_path_buf(), bytes);
            if offset.is_some() && read_total == crate::box_helper::CHUNK as u64 {
                break;
            }
        }
        Ok(LinkedFiles { directory, files })
    }
}

fn linked_files(ctx: &Ctx, record: &Thread, relative: &Path) -> Result<LinkedFiles> {
    if !record.is_remote() {
        return read_linked(Path::new(&record.thread_dir), relative, None);
    }
    let path = Path::new(&record.thread_dir).join(relative);
    let probe = |offset| crate::box_helper::Request::Linked {
        root: record.thread_dir.clone().into(),
        relative: relative.into(),
        offset,
    };
    let manifest: crate::box_helper::Manifest = linked_read(ctx, record, probe(None))?;
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
    for offset in (0..total).step_by(crate::box_helper::CHUNK) {
        let chunk: Vec<u8> = linked_read(ctx, record, probe(Some(offset)))?;
        if chunk.len() as u64 != (total - offset).min(crate::box_helper::CHUNK as u64) {
            bail!(
                "remote linked files changed during copy: {}",
                path.display()
            );
        }
        bytes.extend(chunk);
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
    let hash = linked_read(
        ctx,
        record,
        crate::box_helper::Request::RepoLink {
            root: record.worktree_path.clone().into(),
            relative: relative.into(),
        },
    )?;
    verify_repo_link_hash(ctx, project, record, relative, path, hash)
}

pub(crate) fn repo_link_hash(runner: &dyn Runner, root: &Path, relative: &Path) -> Result<String> {
    let path = &root.join(relative);
    let spec = format!(":(literal){}", relative.to_string_lossy());
    if !checked_link_path(root, path)?.is_file() {
        bail!(
            "linked path is not a regular repo file: {}; worktree kept",
            path.display()
        );
    }
    let git = crate::repo::Git::new(runner, root).with_timeout(GIT_TIMEOUT);
    if git.stdout(&["ls-files", "-z", "--", &spec])?.is_empty() {
        bail!(
            "linked path is an untracked repo file: {}; worktree kept",
            path.display()
        );
    }
    if !git
        .stdout(&[
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=all",
            "--",
            &spec,
        ])?
        .is_empty()
    {
        bail!(
            "linked path is an uncommitted repo file: {}; worktree kept",
            path.display()
        );
    }
    git.run(&["hash-object", "--no-filters", "--", &path.to_string_lossy()])
}

fn verify_repo_link_hash(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
    relative: &Path,
    path: &Path,
    hash: String,
) -> Result<()> {
    let spec = format!(":(literal){}", relative.to_string_lossy());
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
    let blob = crate::repo::Git::new(ctx.runner, &record.repo)
        .with_timeout(GIT_TIMEOUT)
        .run(&[
            "ls-tree",
            "--format=%(objecttype) %(objectname)",
            &head,
            "--",
            &spec,
        ])?;
    if blob != format!("blob {hash}") {
        bail!(
            "linked repo file content is not committed on integration branch `{integration}`: {}; worktree kept",
            path.display()
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
    let events = crate::events::for_thread(project, &record.id);
    let sealed = crate::events::latest_done_event(&events, &record.id, record.attempt.max(1))
        .and_then(|event| event.payload.done.as_ref())
        .map(|done| &done.artifact);
    let Some(sealed) = sealed else {
        let text = if record.is_remote() {
            if linked_file_missing(ctx, record, Path::new("report.md"))? {
                return Ok(());
            }
            String::from_utf8(linked_bytes(ctx, record, Path::new("report.md"))?)?
        } else {
            let draft = Path::new(&record.thread_dir).join("report.md");
            match std::fs::read_to_string(&draft) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => bail!(
                    "could not inspect {}: {error}; worktree kept",
                    draft.display()
                ),
            }
        };
        return draft_has_existing_links(ctx, project, record, &text).and_then(|existing| {
            if existing {
                let machine = if record.is_remote() { "box " } else { "" };
                bail!(
                    "the {machine}report links to files but has no sealed artifact; worktree kept"
                );
            }
            Ok(())
        });
    };
    // Read and validate the seal, never an earlier rewritten report.
    let path = crate::events::artifact_path(project, sealed);
    if !std::fs::symlink_metadata(&path).is_ok_and(|info| info.is_file()) {
        bail!("the sealed report artifact is missing or damaged; worktree kept");
    }
    let bytes = std::fs::read(path)?;
    if thread::sha256_hex(&bytes) != *sealed {
        bail!("sealed report artifact is damaged: {sealed}");
    }
    let text = String::from_utf8(bytes)?;
    let destinations = report_destinations(&text);
    let mut replacements = Vec::new();
    let mut library_replacements = Vec::new();
    let mut files = std::collections::BTreeMap::new();
    let mut total = 0_u64;
    let mut missing = Vec::new();
    for (range, dest) in destinations {
        let relative = match linked_relative_path(project, record, &dest)? {
            Some(ReportLink::Thread(relative)) => relative,
            Some(ReportLink::Repo { relative, source }) => {
                // Git keeps this content on integration; the library copy is
                // outside the worktree, so point it at the kept repository.
                repo_link_kept(ctx, project, record, &relative, &source)?;
                let path = Path::new(&record.repo).join(&relative);
                let encoded: String = path
                    .to_string_lossy()
                    .bytes()
                    .map(|byte| {
                        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
                            (byte as char).to_string()
                        } else {
                            format!("%{byte:02X}")
                        }
                    })
                    .collect();
                let suffix = &dest[dest.split(['#', '?']).next().unwrap_or(&dest).len()..];
                library_replacements.push((range, format!("{encoded}{suffix}")));
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
        library_replacements.push((
            range.clone(),
            format!("../../.state/artifacts/{hash}{suffix}"),
        ));
        replacements.push((range, format!("{hash}{suffix}")));
    }
    thread::update(project, &record.id, |t| {
        t.missing_report_links = missing.clone()
    })?;
    for (relative, bytes) in files {
        thread::store_artifact(project, &bytes)
            .with_context(|| format!("could not preserve {}", relative.display()))?;
    }
    let mut library_report = text.clone();
    for (range, dest) in library_replacements.into_iter().rev() {
        library_report.replace_range(range, &dest);
    }
    if !replacements.is_empty() {
        let mut rewritten = text;
        for (range, dest) in replacements.into_iter().rev() {
            rewritten.replace_range(range, &dest);
        }
        let hash = thread::store_artifact(project, rewritten.as_bytes())?;
        thread::update(project, &record.id, |t| {
            t.final_report_hash = hash.clone();
            t.final_report_seal = sealed.clone();
        })?;
    }
    let _lock = project.lock()?;
    let target = project.dir().join("library").join(&record.id);
    std::fs::create_dir_all(&target)?;
    project::write_atomic(&target.join("report.md"), library_report.as_bytes())
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
    let Some(lane_head) = crate::repo::Git::new(ctx.runner, &record.repo)
        .with_timeout(Duration::from_secs(5))
        .branch_head(&record.branch)?
    else {
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
    let integration_head = crate::repo::Git::new(ctx.runner, &record.repo)
        .with_timeout(Duration::from_secs(5))
        .branch_head(&integration)?
        .with_context(|| format!("integration branch `{integration}` is missing"))?;
    if crate::repo::Git::new(ctx.runner, &record.repo)
        .with_timeout(Duration::from_secs(20))
        .is_ancestor(&lane_head, &integration_head)?
    {
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
    crate::box_helper::call(
        ctx.runner,
        &profile.target,
        &machine,
        crate::box_helper::Request::Inspect {
            path: record.worktree_path.clone(),
            disposable,
            report_stored: report_artifact_stored,
        },
        GIT_TIMEOUT,
        None,
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
    let request = record.retirement_request(RetirementRequest {
        authority: RetirementAuthority::Retained,
        ..Default::default()
    });
    let outcome = retire(
        ctx,
        &project,
        &record,
        request,
        &record.resolved_reason,
        false,
        &mut CleanupViews::default(),
    )?;
    if outcome.state == "cleanup_pending" || outcome.worktree != "removed" {
        bail!(
            "{}",
            outcome
                .worktree_reason
                .as_deref()
                .unwrap_or("cleanup pending")
        );
    }
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
        crate::repo::Git::new(ctx.runner, &repo)
            .with_timeout(Duration::from_secs(30))
            .run(&["worktree", "remove", "--force", &record.worktree_path])?;
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
    cancelling: bool,
) -> Result<()> {
    let Some(view) = views.for_thread(ctx, project, record)? else {
        return Ok(());
    };
    let agents = &view.agents;
    let panes = &view.panes;
    if agents.iter().any(|agent| {
        (thread::agent_matches(record, agent)
            || Path::new(&agent.cwd).starts_with(&record.worktree_path))
            && !(thread::agent_matches(record, agent) && (cancelling || agent.ready()))
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

/// Reparent verified live box lanes when the coordinator binding changes.
pub(crate) fn relink_binding(ctx: &Ctx, project: &Project, pane: &str) -> Result<()> {
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
        let parent = parent_token(&lane, pane);
        if agent.parent() != Some(parent.as_str()) {
            herdr.pane_set_parent(&lane.pane_id, &parent)?;
        }
    }
    Ok(())
}

/// Lineage repair (SPEC-ADE D3); lineage is local.
pub fn tick(project: &Project, herdr: &Herdr, agents: &[Agent]) -> Result<()> {
    let Some(coordinator) = project.coordinator() else {
        return Ok(());
    };
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
        if agent.parent() != Some(coordinator.pane_id.as_str()) {
            herdr.pane_set_parent(&record.pane_id, &coordinator.pane_id)?;
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
                result.note = "process gone: pane parked until requested".into();
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
    if t.recovery_pending && t.provider_wait_started.is_empty() {
        return Row {
            thread: t.clone(),
            group: recorded,
            note: "starting (placement queued)".into(),
        };
    }
    // Placement was refused before a pane existed. Do not diagnose a missing
    // process for a start that has never launched, even with no live session.
    if t.launch_attempts == 0 && !t.provider_wait_started.is_empty() {
        if t.error.contains("version_skew:") {
            return Row {
                thread: t.clone(),
                group: recorded,
                note: format!(
                    "{}; the start is queued and retries after the box install",
                    t.error
                ),
            };
        }
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
    // Liveness is independent of restart eligibility: a waiting seal keeps
    // its attempt even when the bound agent has left an identity-matched shell.
    let agent_gone = live.pane_exists
        && live.agent_state.is_none()
        && !view.agents.iter().any(|agent| agent.pane_id == t.pane_id)
        && thread::can_check_process_gone(t, now)
        && view
            .herdr
            .pane_process_info(&t.pane_id)
            .is_ok_and(|info| info.agent_gone(&t.pane_id));
    let group = if agent_gone {
        Group::WaitingOnYou
    } else {
        thread::group(&fresh, &live, now)
    };
    let note = if t.status == Status::Failed {
        format!("{}: {}", t.failure_class.plain(), t.error)
    } else if !t.startup_wait_started.is_empty() {
        "starting (checking agent readiness)".to_string()
    } else if agent_gone || !live.pane_exists {
        "process gone: pane or agent is absent".to_string()
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

fn report_summary(project: &Project, record: &Thread) -> Result<String> {
    let events = crate::events::for_thread(project, &record.id);
    let latest = crate::events::latest_event(&events, &record.id, record.attempt.max(1));
    let report = crate::events::latest_done_event(&events, &record.id, record.attempt.max(1))
        .and_then(|event| event.payload.done.as_ref())
        .map(|done| std::path::absolute(crate::events::artifact_path(project, &done.artifact)))
        .transpose()?;
    Ok(format!(
        "report: {} · {}",
        report.map_or_else(|| "no report yet".into(), |path| path.display().to_string()),
        crate::usage::summary(latest.and_then(|event| event.usage.as_ref()))
    ))
}

pub fn print_show(ctx: &Ctx, slug: &str, id: &str) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let record = thread::load(&project, id)?;
    let row = rows(ctx, &project)
        .into_iter()
        .find(|row| row.thread.id == id)
        .context("thread disappeared while reading its live state")?;
    println!("{}", report_summary(&project, &record)?);
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
        // SSH remains fake; the actual Rust endpoint and filesystem/Git mechanics run.
        let env = crate::paths::Env::for_test(world.home.path(), &[]);
        let root = world.home.path().to_path_buf();
        world.runner.on_fn(
            |cmd| cmd.program == "ssh" && cmd.display().contains("HERDR_ADE_BOX_INPUT"),
            move |cmd| {
                let ctx = Ctx {
                    env: &env,
                    root: root.clone(),
                    config_dir: root.join("cfg"),
                    runner: &crate::runner::RealRunner,
                    detached_ticker: false,
                };
                crate::box_helper::tests::respond(&ctx, cmd.stdin.as_deref().unwrap())
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
                usage: None,
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
            let report = "[Figure](<../../figures/3d/gaba-dose/curves.svg#plot> \"title\")\n<a HREF = '../../figures/3d/gaba-dose/curves.svg#plot'>Figure</a>\n";
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
            let library_report = std::fs::read_to_string(
                fx.project
                    .dir()
                    .join("library")
                    .join(&lane.id)
                    .join("report.md"),
            )
            .unwrap();
            assert_eq!(
                library_report,
                report.replace("../../figures/", &format!("{}/figures/", fx.repo.display()))
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
            assert_eq!(
                error.to_string(),
                format!(
                    "worktree_dirty: uncommitted changes in {}; not removing (README.md)",
                    lane.worktree_path
                ),
                "{remote}"
            );
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
                let dest = match case {
                    "untracked" => {
                        std::fs::write(root.join("untracked.svg"), "<svg/>").unwrap();
                        "../../untracked.svg"
                    }
                    "modified" | "staged" => {
                        std::fs::write(root.join(figure), "changed").unwrap();
                        if case == "staged" {
                            git(root, &["add", figure]);
                        }
                        "../../figures/3d/gaba-dose/curves.svg"
                    }
                    "lane-only" => {
                        commit_file(root, "lane-only.svg", "new", "lane-only figure");
                        "../../lane-only.svg"
                    }
                    "different-blob" => {
                        commit_file(root, figure, "different", "change figure only in lane");
                        "../../figures/3d/gaba-dose/curves.svg"
                    }
                    "outside" => "../../../outside.svg",
                    "symlink" => {
                        std::fs::write(fx.world.home.path().join("outside.svg"), "outside")
                            .unwrap();
                        symlink(
                            fx.world.home.path().join("outside.svg"),
                            root.join("escape.svg"),
                        )
                        .unwrap();
                        "../../escape.svg"
                    }
                    "symlink-parent" => {
                        let outside = fx.world.home.path().join("outside-dir");
                        std::fs::create_dir_all(outside.join("nested")).unwrap();
                        std::fs::write(outside.join("outside.svg"), "outside").unwrap();
                        symlink(outside.join("nested"), root.join("link")).unwrap();
                        "../../link/../outside.svg"
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
    fn typed_linked_transfer_refuses_changed_bytes_lengths_and_escaping_manifests() {
        for defect in ["hash", "length", "escape", "cap"] {
            let world = crate::scenarios::World::new();
            world.runner.on_fn(|cmd| cmd.program == "ssh" && cmd.display().contains("HERDR_ADE_BOX_INPUT"), move |cmd| {
                let input: serde_json::Value = serde_json::from_str(cmd.stdin.as_deref().unwrap())?;
                let body = if input["request"]["Linked"]["offset"].is_null() {
                    serde_json::json!({"directory":false,"files":[{"path":if defect == "escape" { "../outside" } else { "plot.bin" },"size":if defect == "cap" { LINKED_FILES_CAP + 1 } else { 3 },"hash":thread::sha256_hex(b"abc")}]})
                } else { serde_json::json!(if defect == "length" { b"x".to_vec() } else { b"bad".to_vec() }) };
                Ok(crate::runner::fake::ok(&crate::box_helper::tests::ready(body)))
            });
            linked_test_box(&world);
            let record = Thread {
                machine: "buildbox".into(),
                thread_dir: "/box/lane".into(),
                ..Default::default()
            };
            let error = linked_files(&world.ctx(), &record, Path::new("plot.bin"))
                .err()
                .unwrap()
                .to_string();
            assert!(
                error.contains(match defect {
                    "hash" => "changed during copy",
                    "length" => "changed during copy",
                    "escape" => "not inside",
                    "cap" => "over cap",
                    _ => unreachable!(),
                }),
                "{defect}: {error}"
            );
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
            let report = "Screenshots: [library](<library/#shots> \"title\") `![literal](missing.png)`\n\n> [refs]:\n> library/&#35;shots \"reference title\"\n>\n> [refs]\n";
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
            let library = project.dir().join("library").join(&lane.id);
            let library_report = std::fs::read_to_string(library.join("report.md")).unwrap();
            assert_eq!(
                library_report,
                format!(
                    "Screenshots: [library](<../../.state/artifacts/{index_hash}#shots> \"title\") `![literal](missing.png)`\n\n> [refs]:\n> ../../.state/artifacts/{index_hash}#shots \"reference title\"\n>\n> [refs]\n"
                )
            );
            assert_eq!(
                std::fs::read_to_string(library.join("../../.state/artifacts").join(&index_hash))
                    .unwrap(),
                index
            );
            assert!(rewritten.contains("#shots"));
            if remote {
                assert_eq!(world.runner.count("HERDR_ADE_BOX_INPUT"), 4);
            }
        }
    }

    #[test]
    fn sealed_report_without_links_is_in_library_and_show_names_the_done_path() {
        for remote in [false, true] {
            let world = crate::scenarios::World::new();
            let project = world.project("demo", "a.sock");
            let lane = world.thread(&project, world.home.path(), |t| {
                if remote {
                    t.machine = "buildbox".into();
                }
            });
            assert_eq!(
                report_summary(&project, &lane).unwrap(),
                "report: no report yet · usage unknown"
            );
            seal_linked_report(&project, &lane, "A finished report.\n");
            preserve_report_links(&world.ctx(), &project, &lane).unwrap();
            assert_eq!(
                std::fs::read_to_string(
                    project
                        .dir()
                        .join("library")
                        .join(&lane.id)
                        .join("report.md")
                )
                .unwrap(),
                "A finished report.\n"
            );
            let event = crate::events::for_thread(&project, &lane.id).pop().unwrap();
            let path = std::path::absolute(crate::events::artifact_path(
                &project,
                &event.payload.done.as_ref().unwrap().artifact,
            ))
            .unwrap();
            assert!(
                crate::events::typed_line(&project, &event)
                    .unwrap()
                    .contains(&path.display().to_string())
            );
            assert_eq!(
                report_summary(&project, &lane).unwrap(),
                format!("report: {} · usage unknown", path.display())
            );
        }
    }

    #[test]
    fn show_prints_the_latest_seals_usage_next_to_its_report() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |_| {});
        seal_linked_report(&project, &lane, "Report.\n");
        let mut event = crate::events::for_thread(&project, &lane.id).pop().unwrap();
        event.id = format!("{}-1-new", lane.id);
        event.op = event.id.clone();
        event.created = "2099-01-01T00:00:00Z".into();
        event.usage = Some(crate::usage::Usage {
            input: 200000,
            cache_read: 4900000,
            total: 5100000,
            ..Default::default()
        });
        crate::events::seal_create_if_absent(&project, &event).unwrap();
        assert!(
            report_summary(&project, &lane)
                .unwrap()
                .ends_with(" · 5.1M tokens (4.9M cached)")
        );
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
            for name in ["missing", "escape", "special", "library"] {
                assert!(linked_files(&world.ctx(), &record, std::path::Path::new(name)).is_err());
            }
            std::fs::remove_file(root.join("library/symlink")).unwrap();
            for name in ["a", "b"] {
                std::fs::File::create(root.join("library").join(name))
                    .unwrap()
                    .set_len(101 * 1024 * 1024)
                    .unwrap();
            }
            assert!(linked_files(&world.ctx(), &record, std::path::Path::new("library")).is_err());
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
            assert!(linked_relative_path(&project, &lane, dest).is_err());
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
    fn rebind_sets_current_coordinator_parent_on_both_machines() {
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

            project
                .update_coordinator(|c| c.pane_id = "w1:p9".into())
                .unwrap();
            rebind(&world.ctx(), "demo", &lane.id, "w1:p2").unwrap();

            let expected = if machine.is_empty() {
                "parent=w1:p9"
            } else {
                "parent=Local:w1:p9"
            };
            assert!(world.runner.calls.borrow().iter().any(|cmd| {
                cmd.display().contains("pane report-metadata w1:p2")
                    && cmd.display().contains("--token")
                    && cmd.args.iter().any(|arg| arg == expected)
                    && (machine.is_empty() || cmd.display().contains("--machine buildbox"))
            }));
        }
    }

    #[test]
    fn replacement_binding_reparents_only_verified_box_lanes() {
        use crate::runner::fake::ok;
        use crate::scenarios::{World, agent_json, pane_json};

        for pid in [42, 99] {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            let lane = world.thread(&project, Path::new("/box/lane"), |t| {
                t.machine = "buildbox".into();
                t.identity.process = Some(crate::contracts::ProcessIdentity {
                    pid,
                    argv0: "claude".into(),
                });
            });
            *world.agents.borrow_mut() = format!(
                "[{}]",
                agent_json("w2", "w2:t1", "w2:p1", &lane.cwd, &lane.agent_name, "idle")
            );
            *world.panes.borrow_mut() =
                format!("[{}]", pane_json("w2", "w2:t1", "w2:p1", &lane.cwd));
            world.runner.on(
                "pane process-info",
                ok(r#"{"result":{"process_info":{"foreground_processes":[{"pid":42,"name":"claude","argv0":"claude"}]}}}"#),
            );
            project
                .update_coordinator(|c| c.pane_id = "w1:p9".into())
                .unwrap();
            relink_binding(&world.ctx(), &project, "w1:p9").unwrap();
            assert_eq!(
                world.runner.count("--machine buildbox pane report-metadata w2:p1 --source herdr-ade --token parent=Local:w1:p9"),
                usize::from(pid == 42),
            );
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
                usage: None,
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
        let report = "Résumé: ![real](visible.png)\n```text\nError [ERR_MODULE_NOT_FOUND]: Cannot find package 'yaml' imported from ...\n![x](hidden.png)\n```\n~~~\n[missing]: also-hidden.png\n~~~\n`![inline](inline.png)` and ``[label]: invisible.png``\n\n    [indented](hidden.png)\n\n> ~~~\n> [nested](hidden.png)\n> ~~~\n\n\\[escaped](hidden.png)\nsrc=hidden.png [invalid]: hidden.png\n<!-- <img src='hidden.png'> -->\n";
        let links = report_destinations(report);
        assert_eq!(
            links.into_iter().map(|(_, dest)| dest).collect::<Vec<_>>(),
            vec!["visible.png"]
        );
    }

    #[test]
    fn report_url_spans_preserve_surrounding_markdown_and_html() {
        for (report, expected) in [
            (
                "É [*x*](<a b#f> \"title ](x)\")",
                "É [*x*](<KEPT#f> \"title ](x)\")",
            ),
            (
                r"[![x](i)](dir/) [a](a\(b\))",
                "[![x](KEPT)](KEPT) [a](KEPT)",
            ),
            (
                "[r]: <a b> 't'\n\n[r] [r]\n",
                "[r]: <KEPT> 't'\n\n[r] [r]\n",
            ),
            ("[unused]: path.png\n", "[unused]: KEPT\n"),
            (r"[a\]:b]: path.png", r"[a\]:b]: KEPT"),
            (
                "<IMG data-src=x SRC = 'a&amp;b' href=dir/#f>",
                "<IMG data-src=x SRC = 'KEPT' href=KEPT#f>",
            ),
            (
                "<div>\n<img\n src=x srcset='a 1x,b 2x'>\n</div>",
                "<div>\n<img\n src=KEPT srcset='KEPT 1x,KEPT 2x'>\n</div>",
            ),
            (
                "<img srcset='data:x,a 1x,b&amp;c 2x'>",
                "<img srcset='data:x,a 1x,KEPT 2x'>",
            ),
            (r"[x](a&amp;b.png) [x](a\(b\).png)", "[x](KEPT) [x](KEPT)"),
            ("> <img\n> src='x'>\n", "> <img\n> src='KEPT'>\n"),
            (
                "<img\r\n SRC=é.png href=\"'x\">",
                "<img\r\n SRC=KEPT href=\"KEPT\">",
            ),
            ("> [x]:\n> a.png\n\n> [x]\n", "> [x]:\n> KEPT\n\n> [x]\n"),
            (
                "[]() ![x]() [x](<> \"title\")",
                "[](KEPT) ![x](KEPT) [x](<KEPT> \"title\")",
            ),
            (
                "> [r]:\n> a&amp;b.png\n\n> [r]\n",
                "> [r]:\n> KEPT\n\n> [r]\n",
            ),
            (
                "> > [r]:\n> > a&amp;b.png\n\n> > [r]\n",
                "> > [r]:\n> > KEPT\n\n> > [r]\n",
            ),
            (
                "- > [r]:\n  > a\\(b\\).png\n\n  > [r]\n",
                "- > [r]:\n  > KEPT\n\n  > [r]\n",
            ),
            ("> [x](\n> a&amp;b.png)", "> [x](\n> KEPT)"),
            (
                "> <img\r\n> src='a&amp;b.png'>\r\n",
                "> <img\r\n> src='KEPT'>\r\n",
            ),
            (
                "[outer ![inner][r]](out.png)\n\n[r]: in.png\n",
                "[outer ![inner][r]](KEPT)\n\n[r]: KEPT\n",
            ),
            (
                "<script>\n<img src='hidden.png'>\n</script>\n<img src='real.png'>",
                "<script>\n<img src='hidden.png'>\n</script>\n<img src='KEPT'>",
            ),
        ] {
            let mut rewritten = report.to_string();
            for (range, dest) in report_destinations(report).into_iter().rev() {
                if dest.starts_with("data:") {
                    continue;
                }
                let suffix = dest.find('#').map_or("", |i| &dest[i..]);
                rewritten.replace_range(range, &format!("KEPT{suffix}"));
            }
            assert_eq!(rewritten, expected, "{report}");
        }
        let decoded = report_destinations(r"[x](a\(b\).png) <img src='a&amp;b.png'>");
        assert_eq!(
            decoded.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>(),
            ["a(b).png", "a&b.png"]
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
    fn failed_placement_is_bounded_and_notifies_with_an_executable_retry() {
        let world = crate::scenarios::World::new();
        let project = crate::project::create(&world.root, "demo", "", vec![]).unwrap();
        let reviewer = thread::allocate(&project, |t| {
            t.role = "reviewer".into();
            t.status = Status::Failed;
            t.recovery_pending = true;
            t.launch_attempts = 0;
        })
        .unwrap();
        for count in 1..=thread::MAX_LAUNCH_ATTEMPTS {
            let pending = thread::load(&project, &reviewer.id).unwrap();
            let error = place_recovery(&world.ctx(), &project, &pending).unwrap_err();
            assert!(error.to_string().contains("not reachable"), "{error:#}");
            assert_eq!(
                thread::load(&project, &reviewer.id)
                    .unwrap()
                    .launch_attempts,
                count
            );
        }
        crate::recovery::tick(&world.ctx(), &project).unwrap();
        let failed = thread::load(&project, &reviewer.id).unwrap();
        assert_eq!(failed.status, Status::Failed);
        assert!(!failed.recovery_pending);
        assert!(failed.error.contains("not reachable"));
        assert!(
            failed.start_notices[0]
                .line
                .contains(&retry_command("demo", &reviewer.id))
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
            usage: None,
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
        assert!(prompt_state(&t, &[], false).is_err());
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
    fn working_pi_notes_are_steering_on_local_and_box_routes() {
        use crate::runner::fake::ok;
        use crate::scenarios::{World, agent_json};
        for machine in ["", "box"] {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            let lane = world.thread(&project, world.home.path(), |t| {
                t.agent = "pi".into();
                t.launch.kind = "pi".into();
                t.machine = machine.into();
                t.prompt_pending = false;
                t.bootstrap = "acknowledged".into();
            });
            let agents: Vec<Agent> = serde_json::from_str(&format!(
                "[{}]",
                agent_json(
                    "w2",
                    "w2:t1",
                    "w2:p1",
                    &lane.cwd,
                    &lane.agent_name,
                    "working"
                )
                .replace("\"claude\"", "\"pi\"")
            ))
            .unwrap();
            *world.agents.borrow_mut() = serde_json::to_string(&agents).unwrap();
            world.runner.on("agent prompt", ok(r#"{"result":{}}"#));
            world.runner.on("machine list --json", ok(r#"[{"id":"box","label":"box","target":"box","session":"default","enabled":true}]"#));
            let herdr = Herdr::new(world.env.herdr_bin(), "a.sock", &world.runner);
            let outcome = if machine.is_empty() {
                prompt(&world.ctx(), "demo", &lane.id, "steer at next boundary").unwrap()
            } else {
                send_lane_input(
                    &world.ctx(),
                    &project,
                    &lane.id,
                    Some("steer at next boundary"),
                    Some((&herdr, &agents, &lane)),
                )
                .unwrap()
            };
            assert_eq!(
                outcome,
                PromptOutcome::Sent {
                    attempt: 1,
                    agent_state: "working".into()
                }
            );
            let saved = thread::load(&project, &lane.id).unwrap();
            assert_eq!(saved.follow_ups[0].state, FollowUpState::Delivered);
            let calls = world.runner.calls.borrow();
            let call = calls
                .iter()
                .find(|c| c.display().contains("agent prompt"))
                .unwrap();
            let request: serde_json::Value =
                serde_json::from_str(call.stdin.as_deref().unwrap()).unwrap();
            assert!(request["params"].get("wait").is_none());
            assert_eq!(call.program == "ssh", machine == "box");
            assert!(!calls.iter().any(|c| c.display().contains("pane send-text")));
        }
    }

    #[test]
    fn unconfirmed_pi_steering_is_retained_and_reported() {
        use crate::runner::fake::{fail, timeout};
        use crate::scenarios::{World, agent_json};
        for (reply, expected) in [
            (
                fail(
                    1,
                    r#"{"error":{"code":"agent_blocked","message":"dialog opened"}}"#,
                ),
                FollowUpState::Queued,
            ),
            (
                fail(
                    1,
                    r#"{"error":{"code":"exec_failed","message":"could not execute herdr: Argument list too long (os error 7)"}}"#,
                ),
                FollowUpState::Queued,
            ),
            (timeout(), FollowUpState::Uncertain),
        ] {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            let lane = world.thread(&project, world.home.path(), |t| {
                t.agent = "pi".into();
                t.launch.kind = "pi".into();
                t.prompt_pending = false;
                t.bootstrap = "acknowledged".into();
            });
            *world.agents.borrow_mut() = format!(
                "[{}]",
                agent_json(
                    "w2",
                    "w2:t1",
                    "w2:p1",
                    &lane.cwd,
                    &lane.agent_name,
                    "working"
                )
            );
            world.runner.on("agent prompt", reply);
            assert_eq!(
                prompt(&world.ctx(), "demo", &lane.id, "do not lose me").unwrap(),
                PromptOutcome::Queued { attempt: 1 }
            );
            let saved = thread::load(&project, &lane.id).unwrap();
            assert_eq!(saved.follow_ups[0].state, expected);
            assert_eq!(saved.follow_ups[0].text, "do not lose me");
            assert!(saved.follow_ups[0].delivered_at.is_empty());
            assert!(
                crate::inbox::unhandled(&project)
                    .iter()
                    .any(|i| i.summary.contains("steering delivery unconfirmed"))
            );
            if expected == FollowUpState::Uncertain {
                assert_eq!(
                    send_lane_input(&world.ctx(), &project, &lane.id, None, None).unwrap(),
                    PromptOutcome::Queued { attempt: 1 }
                );
                assert_eq!(world.runner.count("agent prompt"), 1);
            }
        }
    }

    #[test]
    fn working_claude_keeps_the_queue() {
        use crate::scenarios::{World, agent_json};
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |t| {
            t.prompt_pending = false;
            t.bootstrap = "acknowledged".into();
        });
        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json(
                "w2",
                "w2:t1",
                "w2:p1",
                &lane.cwd,
                &lane.agent_name,
                "working"
            )
        );
        assert_eq!(
            prompt(&world.ctx(), "demo", &lane.id, "later").unwrap(),
            PromptOutcome::Queued { attempt: 1 }
        );
        assert_eq!(world.runner.count("agent prompt"), 0);
        assert_eq!(
            thread::load(&project, &lane.id).unwrap().follow_ups[0].state,
            FollowUpState::Queued
        );
    }

    #[test]
    fn answered_post_seal_notes_confirm_once_only_with_unchanged_clean_head() {
        use crate::testkit::{commit_file, fixture};
        for role in ["worker", "reviewer"] {
            for change in ["none", "commit", "dirty", "seal", "queued", "recent"] {
                let fx = fixture();
                let (id, sha) = fx.lane(1);
                let seal = fx.seal_done(
                    &id,
                    1,
                    1,
                    &sha,
                    if role == "reviewer" {
                        "MERGE"
                    } else {
                        "finished"
                    },
                );
                let lane = thread::update(&fx.project, &id, |t| {
                    t.role = role.into();
                    t.review_after = seal.clone();
                    t.follow_ups.push(FollowUp {
                        attempt: 1,
                        text: "already fixed?".into(),
                        state: FollowUpState::Delivered,
                        after_seal: seal.clone(),
                        delivered_at: "2026-09-18T11:00:00Z".into(),
                        ..Default::default()
                    });
                    if change == "queued" {
                        t.follow_ups[0].state = FollowUpState::Queued;
                    }
                    if change == "recent" {
                        t.follow_ups[0].delivered_at = project::now();
                    }
                })
                .unwrap();
                let folder = Path::new(&lane.worktree_path);
                if change == "commit" {
                    commit_file(folder, "correction.txt", "new", "correction");
                }
                if change == "dirty" {
                    std::fs::write(folder.join("untracked.txt"), "new").unwrap();
                }
                if change == "seal" {
                    fx.seal_done(&id, 1, 2, &sha, "new verdict");
                }
                crate::ticker::restore_unchanged_seal(&fx.world.ctx(), &fx.project, &lane).unwrap();
                let saved = thread::load(&fx.project, &id).unwrap();
                if change == "none" {
                    assert_eq!(saved.follow_ups[0].state, FollowUpState::Closed);
                    assert!(saved.review_after.is_empty());
                    assert_eq!(saved.start_notices.len(), 1);
                    assert!(
                        saved.start_notices[0]
                            .line
                            .contains("confirmed existing seal")
                    );
                    let events = crate::events::for_thread(&fx.project, &id);
                    assert!(!follow_up_pending_for_seal(
                        &saved,
                        crate::events::latest_done_event(&events, &id, 1)
                    ));
                    assert!(crate::review::sealed(&events, &saved).is_some());
                    crate::ticker::restore_unchanged_seal(&fx.world.ctx(), &fx.project, &saved)
                        .unwrap();
                    assert_eq!(
                        thread::load(&fx.project, &id).unwrap().start_notices.len(),
                        1
                    );
                } else {
                    assert_ne!(saved.follow_ups[0].state, FollowUpState::Closed, "{change}");
                    assert_eq!(saved.review_after, seal);
                    assert!(saved.start_notices.is_empty());
                    let events = crate::events::for_thread(&fx.project, &id);
                    let old = events.iter().find(|e| e.id == seal).unwrap();
                    assert!(follow_up_pending_for_seal(&saved, Some(old)));
                    if change == "seal" {
                        assert!(!follow_up_pending_for_seal(
                            &saved,
                            crate::events::latest_done_event(&events, &id, 1)
                        ));
                        assert!(crate::review::sealed(&events, &saved).is_some());
                    }
                }
            }
        }
    }

    #[test]
    fn unreadable_newer_seal_cannot_confirm_an_older_answered_seal() {
        let fx = crate::testkit::fixture();
        let (id, sha) = fx.lane(1);
        let old = fx.seal_done(&id, 1, 1, &sha, "MERGE");
        let lane = thread::update(&fx.project, &id, |t| {
            t.role = "reviewer".into();
            t.review_after = old.clone();
            t.follow_ups.push(FollowUp {
                attempt: 1,
                text: "check the verdict".into(),
                state: FollowUpState::Delivered,
                after_seal: old.clone(),
                delivered_at: "2026-09-18T11:00:00Z".into(),
                ..Default::default()
            });
        })
        .unwrap();
        let newer = fx.seal_done(&id, 1, 2, &sha, "REJECT");
        std::fs::write(
            crate::events::dir(&fx.project).join(format!("{newer}.toml")),
            "id = 'truncated",
        )
        .unwrap();
        assert!(
            crate::ticker::restore_unchanged_seal(&fx.world.ctx(), &fx.project, &lane).is_err()
        );
        let saved = thread::load(&fx.project, &id).unwrap();
        assert_eq!(saved.review_after, old);
        assert_eq!(saved.follow_ups[0].state, FollowUpState::Delivered);
        assert!(saved.start_notices.is_empty());
    }

    #[test]
    fn cli_and_ticker_share_staging_transport_and_receipts() {
        use crate::runner::fake::ok;
        use crate::scenarios::{World, agent_json};
        for ticker_delivery in [false, true] {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            let lane = world.thread(&project, world.home.path(), |t| {
                t.prompt_pending = ticker_delivery;
                t.bootstrap = "acknowledged".into();
            });
            *world.agents.borrow_mut() = format!(
                "[{}]",
                agent_json("w2", "w2:t1", "w2:p1", &lane.cwd, &lane.agent_name, "idle")
            );
            let sending_project = project.clone();
            let sending_id = lane.id.clone();
            world.runner.on_fn(
                |cmd| cmd.display().contains("agent prompt"),
                move |_| {
                    let saved = thread::load(&sending_project, &sending_id).unwrap();
                    assert_eq!(saved.follow_ups.len(), 1);
                    assert_eq!(saved.follow_ups[0].state, FollowUpState::Uncertain);
                    assert_eq!(saved.follow_ups[0].text, "same correction");
                    Ok(ok(r#"{"result":{}}"#))
                },
            );
            let outcome = prompt(&world.ctx(), "demo", &lane.id, "same correction").unwrap();
            if ticker_delivery {
                assert!(matches!(outcome, PromptOutcome::Queued { .. }));
                thread::update(&project, &lane.id, |t| t.prompt_pending = false).unwrap();
                assert!(matches!(
                    send_lane_input(&world.ctx(), &project, &lane.id, None, None).unwrap(),
                    PromptOutcome::Sent { .. }
                ));
            } else {
                assert!(matches!(outcome, PromptOutcome::Sent { .. }));
            }
            let saved = thread::load(&project, &lane.id).unwrap();
            assert_eq!(world.runner.count("agent prompt"), 1);
            assert_eq!(saved.follow_ups[0].state, FollowUpState::Delivered);
            assert!(!saved.follow_ups[0].delivered_at.is_empty());
            assert!(matches!(
                send_lane_input(&world.ctx(), &project, &lane.id, None, None).unwrap(),
                PromptOutcome::Queued { .. }
            ));
            assert_eq!(world.runner.count("agent prompt"), 1);
        }
    }

    #[test]
    fn correction_during_review_preparation_excludes_the_stale_pile_snapshot() {
        use crate::runner::{RealRunner, Runner};
        use crate::testkit::{fixture, git};
        let fx = fixture();
        let (id, sha) = fx.lane(1);
        let event = fx.seal_done(&id, 1, 1, &sha, "finished");
        let base = git(&fx.repo, &["rev-parse", "main"]);
        thread::update(&fx.project, &id, |t| {
            t.base = base;
            t.parked = true;
            t.prompt_pending = false;
        })
        .unwrap();
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos[0].branch = Some("main".into());
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        let env = fx.world.env.clone();
        let root = fx.world.root.clone();
        let config_dir = fx.world.ctx().config_dir;
        let corrected_id = id.clone();
        let corrected_project = fx.project.clone();
        // Review has already selected the old seal. Inject the CLI correction
        // while it prepares git evidence, immediately before membership commit.
        let runner = crate::runner::fake::FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.program == "git" && cmd.display().contains("diff --name-only"),
            move |cmd| {
                let runner = crate::runner::fake::FakeRunner::new();
                runner.on(
                    "agent list",
                    crate::runner::fake::ok(r#"{"result":{"agents":[]}}"#),
                );
                runner.on(
                    "pane list",
                    crate::runner::fake::ok(r#"{"result":{"panes":[]}}"#),
                );
                let ctx = Ctx {
                    env: &env,
                    root: root.clone(),
                    config_dir: config_dir.clone(),
                    runner: &runner,
                    detached_ticker: false,
                };
                assert!(matches!(
                    prompt(&ctx, "demo", &corrected_id, "fix this before review").unwrap(),
                    PromptOutcome::Queued { .. }
                ));
                let saved = thread::load(&corrected_project, &corrected_id).unwrap();
                assert_eq!(saved.review_after, event);
                assert_eq!(saved.follow_ups[0].state, FollowUpState::Queued);
                RealRunner.run(cmd)
            },
        );
        runner.on_fn(|cmd| cmd.program == "git", |cmd| RealRunner.run(cmd));
        let ctx = Ctx {
            runner: &runner,
            ..fx.world.ctx()
        };
        assert!(crate::review::start(&ctx, "demo", None).unwrap().is_none());
        assert!(crate::review::list(&fx.project).unwrap().is_empty());
        let saved = thread::load(&fx.project, &id).unwrap();
        assert!(!saved.parked);
        assert_eq!(saved.status, Status::Starting);
        assert!(!parkable(&fx.project, &saved));
        assert_eq!(saved.follow_ups.len(), 1);
        assert!(saved.follow_ups[0].text.ends_with("fix this before review"));
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
                usage: None,
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
        let session = folder.join("session-42.jsonl");
        std::fs::write(&session, b"saved pi session").unwrap();
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
            t.identity.agent_session = Some(session.to_string_lossy().into_owned());
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
        project
            .update_coordinator(|c| c.pane_id = "w1:p9".into())
            .unwrap();
        let ctx = world.ctx();
        let outcome = prompt(&ctx, "demo", &lane.id, "Fix the rejection").unwrap();
        assert!(
            matches!(outcome, PromptOutcome::Queued { .. }),
            "{outcome:?}"
        );
        let reopened = thread::load(&project, &lane.id).unwrap();
        assert!(!reopened.parked);
        assert!(!reopened.prompt_pending);
        assert!(reopened.recovery_pending);
        assert_eq!(reopened.bootstrap, "resuming");
        assert_eq!(world.runner.count("tab create"), 0);
        assert_eq!(world.runner.count("agent start"), 0);
        assert_eq!(
            reopened.identity.agent_session.as_deref(),
            Some(session.to_str().unwrap())
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
        thread::update(&project, &lane.id, |t| {
            t.launch.brief_hash = thread::store_artifact(&project, b"brief").unwrap();
        })
        .unwrap();
        let reopened = place_started(&ctx, &project, &thread::load(&project, &lane.id).unwrap());
        *world.agents.borrow_mut() = "[]".into();
        *world.panes.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::pane_json("w1", "w1:t2", "w1:p2", &reopened.cwd)
        );
        thread::update(&project, &lane.id, |t| t.error = "provider ready".into()).unwrap();
        ticker::launch_thread_with_wait(&ctx, &project, &lane.id, Duration::ZERO).unwrap();
        assert!(world.runner.calls.borrow().iter().any(|c| {
            let text = c.display();
            text.contains("agent start")
                && text.contains(&format!("--session {}", session.display()))
                && text.contains("--parent w1:p9")
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
        assert_eq!(retried.state, RetryState::Queued);
        assert!(retried.pane_id.is_empty());
        assert!(thread::load(&project, &lane.id).unwrap().recovery_pending);
        assert_eq!(
            thread::load(&project, &lane.id).unwrap().worktree_path,
            lane.worktree_path
        );
        assert_eq!(world.runner.count("agent prompt"), 0);
        let retried_lane = thread::load(&project, &lane.id).unwrap();
        assert_eq!(retried_lane.follow_ups.len(), 1);
        assert_eq!(retried_lane.follow_ups[0].text, "Repair the conflict");
    }

    fn place_started(ctx: &Ctx, project: &Project, record: &Thread) -> Thread {
        place_recovery(ctx, project, record).unwrap();
        thread::load(project, &record.id).unwrap()
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
        let accepted_at = std::time::Instant::now();
        let started = start(
            &ctx,
            "demo",
            StartArgs {
                title: "Repair".into(),
                repo: Some(repo.clone()),
                machine: None,
                base: None,
                task: "Repair the lane.".into(),
                attach: Vec::new(),
                paths: Vec::new(),
                workflow: None,
                recipe: None,
                task_id: String::new(),
                review_id: String::new(),
            },
        )
        .unwrap();
        eprintln!(
            "start acceptance returned in {:?}; no checkout, pane or agent created",
            accepted_at.elapsed()
        );
        assert!(!started.prompt_pending);
        assert!(!Path::new(&started.thread_dir).join("brief.md").exists());
        assert_eq!(started.status, Status::Starting);
        assert!(started.recovery_pending);
        assert!(started.worktree_path.is_empty());
        assert_eq!(world.runner.count("tab create"), 0);
        assert!(!world.runner.calls.borrow().iter().any(|cmd| {
            cmd.display().contains("agent start") && !cmd.display().contains("--help")
        }));

        drop(install);
        let wt = repo.clone() + "/.worktrees/" + &started.id;
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
                attach: Vec::new(),
                paths: Vec::new(),
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
        assert_eq!(started.status, Status::Starting);
        assert!(started.recovery_pending);
        assert!(!wt.exists(), "start must not create a checkout");
        assert_eq!(world.runner.count("tab create"), 0);
        let started = place_started(&ctx, &project, &started);
        assert!(wt.is_dir(), "ticker placement creates the checkout");
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
        assert!(brief.contains(lead_brief), "{brief}");
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
            .map(|c| {
                serde_json::from_str::<serde_json::Value>(c.stdin.as_deref().unwrap()).unwrap()
                        ["params"]["text"]
                        .as_str()
                        .unwrap()
                        .to_string()
            })
            .collect();
        assert_eq!(prompts.len(), 1, "{prompts:?}");
        assert!(prompts[0].contains(&format!(".herdr-project/demo-{}/brief.md", started.id)));
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
        assert!(retried.recovery_pending);
        let retried = place_started(&ctx, &project, &retried);
        assert_eq!(retried.attempt, 2);
        assert_eq!(retried.launch.kind, kind);
        assert_eq!(retried.launch.attempt, 2);
        assert_eq!(retried.launch.work_retries, 0);
        assert_eq!(retried.launch.same_recipe_retries, 1);
        assert_eq!(retried.launch.brief_hash, started.launch.brief_hash);
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
            attach: Vec::new(),
            paths: Vec::new(),
            repo: Some(repo),
            machine: None,
            base: None,
            task: "Do the thing.".into(),
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
                if crate::box_helper::tests::is_doctor(cmd) {
                    return Ok(crate::doctor::boundary_diagnostic_output(
                        cmd, 99_999_999, None,
                    ));
                }
                if script.contains("uname -s") {
                    return Ok(ok("Linux\n"));
                }
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
            attach: Vec::new(),
            paths: Vec::new(),
            repo,
            machine,
            base: None,
            task: "Do the thing.".into(),
            workflow: None,
            recipe: None,
            task_id: String::new(),
            review_id: String::new(),
        }
    }

    #[test]
    fn attachments_are_frozen_staged_and_kept_on_retry_and_reopen() {
        for remote in [false, true] {
            let (fx, _) = box_fixture();
            write_config(&fx, &lane_config());
            stub_box(&fx);
            crate::prompt::record_test_request(
                &fx.project,
                "q-facts",
                "Keep global facts retrievable.",
            )
            .unwrap();
            let fact = crate::note::add(
                &fx.project,
                crate::note::Kind::Memory,
                "Settled global history, not this lane's task.",
                "q-facts",
                None,
                vec![],
            )
            .unwrap();
            let name = "named ' input.bin";
            let source = fx.world.home.path().join(name);
            let bytes = b"\0\xffnamed bytes\n";
            std::fs::write(&source, bytes).unwrap();
            let mut args = start_args(
                Some(fx.repo.to_string_lossy().into_owned()),
                Some(if remote { "buildbox" } else { "local" }.into()),
            );
            args.attach = vec![source.to_string_lossy().into_owned()];
            args.paths = vec!["src/**".into(), "progress.txt".into()];
            let started = start(&fx.world.ctx(), "demo", args).unwrap();
            assert_eq!(started.status, Status::Starting);
            assert!(started.worktree_path.is_empty());
            assert_eq!(fx.world.runner.count("tab create"), 0);
            assert_eq!(fx.world.runner.count("workspace create"), 0);
            let brief = String::from_utf8(
                thread::artifact(&fx.project, &started.launch.brief_hash).unwrap(),
            )
            .unwrap();
            let path = format!("{}/attachments/{name}", started.thread_dir);
            assert!(
                brief.contains(&format!("- Attachment: `{path}`")),
                "{brief}"
            );
            std::fs::write(&source, b"changed after acceptance").unwrap();
            assert!(brief.contains("# Writable paths\n\n- `src/**`\n- `progress.txt`"));
            assert!(brief.contains("run `ha done`."));
            assert!(!brief.contains("--report"));
            assert!(!brief.contains(&fact.text));
            let pointer = brief
                .lines()
                .find(|line| line.starts_with("- Unscoped facts"))
                .unwrap()
                .split('`')
                .nth(1)
                .unwrap()
                .to_string();
            let frozen_facts = if remote {
                let filename = Path::new(&pointer).file_name().unwrap().to_str().unwrap();
                thread::artifact(&fx.project, &started.attachments[filename]).unwrap()
            } else {
                std::fs::read(&pointer).unwrap()
            };
            assert!(String::from_utf8_lossy(&frozen_facts).contains(&fact.text));
            crate::note::retire(
                &fx.project,
                &fact.id,
                "q-facts",
                "History no longer current.",
            )
            .unwrap();
            let check = |lane: &Thread| {
                assert_eq!(lane.paths, started.paths);
                assert!(thread::in_start_window(lane, jiff::Timestamp::now()));
                assert_eq!(
                    thread::artifact(&fx.project, &lane.attachments[name]).unwrap(),
                    bytes
                );
                if remote {
                    let calls = fx.world.runner.calls.borrow();
                    let card = calls
                        .iter()
                        .filter_map(|call| call.stdin.as_deref())
                        .filter_map(|text| toml::from_str::<crate::contracts::LaneCard>(text).ok())
                        .rfind(|card| card.thread == lane.id)
                        .unwrap();
                    assert_eq!(card.paths, lane.paths);
                    assert_eq!(card.attempt, lane.attempt);
                    let encoded: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                    assert!(fx.world.runner.calls.borrow().iter().any(|call| {
                        call.program == "ssh"
                            && call.stdin.as_deref() == Some(&encoded)
                            && call
                                .args
                                .last()
                                .unwrap()
                                .contains(&format!("{}/attachments/", lane.thread_dir))
                    }));
                    let encoded: String = frozen_facts.iter().map(|b| format!("{b:02x}")).collect();
                    assert!(fx.world.runner.calls.borrow().iter().any(|call| {
                        call.program == "ssh"
                            && call.stdin.as_deref() == Some(&encoded)
                            && call.args.last().unwrap().contains(&pointer)
                    }));
                } else {
                    assert_eq!(std::fs::read(&path).unwrap(), bytes);
                    assert_eq!(std::fs::read(&pointer).unwrap(), frozen_facts);
                }
            };
            let placed = place_started(&fx.world.ctx(), &fx.project, &started);
            check(&placed);
            // A crash after binding but before finishing the card reuses that
            // exact terminal, rather than leaking a second pane.
            *fx.world.panes.borrow_mut() = serde_json::json!([{
                "workspace_id": placed.workspace_id, "tab_id": placed.tab_id,
                "pane_id": placed.pane_id, "cwd": placed.cwd
            }])
            .to_string();
            let partial = thread::update(&fx.project, &placed.id, |t| {
                t.partial = Some("lane_card".into());
                t.recovery_pending = true;
                t.status = Status::Starting;
            })
            .unwrap();
            let tabs = fx.world.runner.count("tab create");
            let workspaces = fx.world.runner.count("workspace create");
            let placed = place_started(&fx.world.ctx(), &fx.project, &partial);
            assert_eq!(fx.world.runner.count("tab create"), tabs);
            assert_eq!(fx.world.runner.count("workspace create"), workspaces);
            assert_eq!(placed.pane_id, partial.pane_id);
            if !remote {
                let checkout = Path::new(&placed.worktree_path);
                std::fs::write(checkout.join("progress.txt"), b"keep this work").unwrap();
                crate::testkit::git(checkout, &["add", "progress.txt"]);
                crate::testkit::git(checkout, &["commit", "-qm", "lane progress"]);
                thread::update(&fx.project, &placed.id, |t| {
                    t.status = Status::Failed;
                    t.startup_wait_started.clear();
                    t.partial = Some("placement".into());
                    t.launch_attempts = thread::MAX_LAUNCH_ATTEMPTS;
                })
                .unwrap();
            }
            retry(
                &fx.world.ctx(),
                "demo",
                &placed.id,
                "retry before agent submission",
            )
            .unwrap();
            let retried = place_started(
                &fx.world.ctx(),
                &fx.project,
                &thread::load(&fx.project, &placed.id).unwrap(),
            );
            check(&retried);
            let parked = thread::update(&fx.project, &retried.id, |t| {
                t.parked = true;
                t.prompt_pending = false;
                t.identity.agent_session = Some("saved-session".into());
            })
            .unwrap();
            prompt(&fx.world.ctx(), "demo", &parked.id, "continue").unwrap();
            let reopened = place_started(
                &fx.world.ctx(),
                &fx.project,
                &thread::load(&fx.project, &placed.id).unwrap(),
            );
            check(&reopened);
            assert_eq!(reopened.attachments, started.attachments);
            assert_eq!(reopened.launch.brief_hash, started.launch.brief_hash);
            assert_eq!(
                reopened.launch.same_recipe_retries,
                started.launch.same_recipe_retries
            );
            if !remote {
                assert_eq!(
                    std::fs::read(Path::new(&reopened.worktree_path).join("progress.txt")).unwrap(),
                    b"keep this work"
                );
            }
            assert_eq!(
                fx.world
                    .runner
                    .calls
                    .borrow()
                    .iter()
                    .filter(|call| call.program == "git" && call.display().contains("worktree add"))
                    .count(),
                if remote { 0 } else { 1 }
            );
        }
    }

    #[test]
    fn bad_attachments_base_and_paths_are_refused_before_allocating_a_lane() {
        let fx = crate::testkit::fixture();
        for path in ["/src/**", "../src/**", "src/**.rs", "src/[ab].rs"] {
            let mut args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
            args.paths = vec![path.into()];
            assert!(
                start(&fx.world.ctx(), "demo", args)
                    .unwrap_err()
                    .to_string()
                    .contains("gate_paths_invalid")
            );
            assert!(thread::list(&fx.project).is_empty());
        }
        let file = fx.world.home.path().join("input.bin");
        std::fs::write(&file, b"input").unwrap();
        let second = fx.world.home.path().join("second/input.bin");
        std::fs::create_dir_all(second.parent().unwrap()).unwrap();
        std::fs::write(&second, b"another input").unwrap();
        for files in [
            vec!["/missing-input.bin".into()],
            vec![
                file.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned(),
            ],
        ] {
            let mut args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
            args.attach = files;
            assert!(
                start(&fx.world.ctx(), "demo", args)
                    .unwrap_err()
                    .to_string()
                    .contains("attachment_")
            );
            assert!(thread::list(&fx.project).is_empty());
        }
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(LINKED_FILES_CAP + 1)
            .unwrap();
        let mut args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
        args.attach = vec![file.to_string_lossy().into_owned()];
        assert!(
            start(&fx.world.ctx(), "demo", args)
                .unwrap_err()
                .to_string()
                .contains("over cap")
        );
        let mut args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
        args.base = Some("missing-branch".into());
        assert!(
            start(&fx.world.ctx(), "demo", args)
                .unwrap_err()
                .to_string()
                .contains("integration_branch_required")
        );
        assert!(thread::list(&fx.project).is_empty());
        assert_eq!(fx.world.runner.count("tab create"), 0);
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
    fn remote_retry_discards_old_bootstrap_and_submission_before_and_after_placement() {
        let (fx, _) = box_fixture();
        write_config(&fx, &lane_config());
        stub_box(&fx);
        let ctx = fx.world.ctx();
        let started = start(
            &ctx,
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        let started = place_started(&ctx, &fx.project, &started);
        thread::update(&fx.project, &started.id, |t| {
            t.status = Status::Failed;
            t.bootstrap = "acknowledged".into();
            t.brief_submitted = true;
            t.brief_submitted_at = project::now();
            t.launch_attempts = 1;
            t.startup_wait_started.clear();
            t.failure_class = crate::contracts::FailureClass::ProcessGone;
        })
        .unwrap();
        retry(&ctx, "demo", &started.id, "remote agent killed").unwrap();
        let queued = thread::load(&fx.project, &started.id).unwrap();
        assert_eq!(queued.attempt, started.attempt + 1);
        assert!(queued.bootstrap.is_empty());
        assert!(!queued.brief_submitted);
        assert!(queued.brief_submitted_at.is_empty());

        // The old pane remains bound until placement. A courier pass in this
        // interval can import its old receipt again; that is not new work.
        thread::update(&fx.project, &started.id, |t| {
            t.bootstrap = "acknowledged".into();
        })
        .unwrap();
        let placed = place_started(&ctx, &fx.project, &queued);
        assert_eq!(placed.attempt, queued.attempt);
        assert_eq!(placed.worktree_path, started.worktree_path);
        assert_eq!(placed.launch.brief_hash, started.launch.brief_hash);
        assert!(placed.bootstrap.is_empty());
        assert!(!placed.brief_submitted);
        assert!(placed.brief_submitted_at.is_empty());
        assert!(placed.prompt_pending);
        assert_eq!(placed.launch_attempts, 0);
    }

    #[test]
    fn retry_keeps_a_placed_machine_and_dispatches_an_unplaced_lane() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, &lane_config());
        stub_box(&fx);
        let args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
        let started = start(&fx.world.ctx(), "demo", args).unwrap();
        let started = place_started(&fx.world.ctx(), &fx.project, &started);
        assert_eq!(started.machine, "buildbox");
        // A placed attempt keeps its saved machine when retried.
        thread::update(&fx.project, &started.id, |t| {
            t.failure_class = crate::contracts::FailureClass::ProcessGone;
        })
        .unwrap();
        let result = retry(&fx.world.ctx(), "demo", &started.id, "process disappeared").unwrap();
        assert_eq!(result.state, RetryState::Queued);
        assert!(
            result.pane_id.is_empty(),
            "never expose the old attempt's pane"
        );
        assert!(
            result
                .message()
                .contains("queued for placement on buildbox")
        );
        assert!(!result.message().contains("was delivered"));
        assert!(!result.message().contains(&started.pane_id));
        let placed = thread::load(&fx.project, &started.id).unwrap();
        assert_eq!(placed.machine, "buildbox");

        // A selected local machine is also a placement even though its saved
        // machine is empty. Do not move an explicitly local start to the box.
        let mut local_args = start_args(
            Some(fx.repo.to_string_lossy().into_owned()),
            Some("local".into()),
        );
        local_args.base = Some("main".into());
        let local = start(&fx.world.ctx(), "demo", local_args).unwrap();
        let local = place_started(&fx.world.ctx(), &fx.project, &local);
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

        // A failed start before it acquired any machine or work is dispatched
        // again, using the current routing pick and the box mapping.
        let args = start_args(Some(fx.repo.to_string_lossy().into_owned()), None);
        let unplaced = start(&fx.world.ctx(), "demo", args).unwrap();
        thread::update(&fx.project, &unplaced.id, |t| {
            t.machine.clear();
            t.machine_id.clear();
            t.placement_reason.clear();
            t.launch.machine = "local".into();
            t.recovery_pending = false;
            t.status = Status::Failed;
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
    }

    #[test]
    fn box_disk_floor_refuses_before_creating_work_and_recovers() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, &lane_config());
        let free = std::rc::Rc::new(std::cell::Cell::new(5_u64));
        let current = free.clone();
        fx.world.runner.on_fn(
            |cmd| cmd.program == "ssh" && crate::box_helper::tests::is_doctor(cmd),
            move |cmd| {
                Ok(crate::doctor::boundary_diagnostic_output(
                    cmd,
                    current.get() * 1_000_000,
                    None,
                ))
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
        assert!(error.starts_with("disk_low:"), "{error}");
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
        assert!(started.worktree_path.is_empty());
        assert!(started.recovery_pending);
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
        let started = place_started(&fx.world.ctx(), &fx.project, &started);
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
        let second = place_started(&fx.world.ctx(), &fx.project, &second);
        assert_eq!(second.workspace_id, started.workspace_id);
        let calls = fx.world.runner.calls.borrow();
        for lane in [&started, &second] {
            assert!(calls.iter().any(|call| {
                call.display().contains(&format!(
                    "--machine buildbox-id pane report-metadata {} --source herdr-ade --token parent=Local:w1:p1",
                    lane.pane_id
                ))
            }));
        }
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
    fn reopened_box_lane_sets_current_machine_qualified_parent() {
        use crate::runner::fake::ok;
        let (fx, _) = box_fixture();
        write_config(&fx, &lane_config());
        stub_box(&fx);
        let started = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        let started = place_started(&fx.world.ctx(), &fx.project, &started);
        let parked = thread::update(&fx.project, &started.id, |t| {
            t.parked = true;
            t.identity.agent_session = Some("session-42".into());
        })
        .unwrap();
        fx.project
            .update_coordinator(|c| c.pane_id = "w1:p9".into())
            .unwrap();
        fx.world.runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"workspace_id":"w1","tab_id":"w1:t3","pane_id":"w1:p3","agent_status":"idle"}}}"#),
        );
        prompt(&fx.world.ctx(), "demo", &parked.id, "Fix the rejection").unwrap();
        let reopened = place_started(
            &fx.world.ctx(),
            &fx.project,
            &thread::load(&fx.project, &started.id).unwrap(),
        );
        *fx.world.panes.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::pane_json("w1", "w1:t3", "w1:p3", &reopened.cwd)
        );
        ticker::launch_thread_with_wait(&fx.world.ctx(), &fx.project, &started.id, Duration::ZERO)
            .unwrap();
        assert!(fx.world.runner.calls.borrow().iter().any(|call| {
            call.display().contains("--machine buildbox-id pane report-metadata w1:p3 --source herdr-ade --token parent=Local:w1:p9")
        }));
        let calls = fx.world.runner.calls.borrow();
        let launch = calls
            .iter()
            .find(|call| {
                call.display().contains("agent start") && !call.display().contains("--help")
            })
            .unwrap()
            .display();
        assert!(launch.contains("--resume session-42"), "{launch}");
        assert!(!launch.contains("--parent"), "{launch}");
    }

    #[test]
    fn untrusted_managed_claude_worktree_and_agy_use_the_mac_without_box_readiness_checks() {
        let (fx, _remote) = box_fixture();
        write_config(
            &fx,
            &format!("{ROUTED_BOX_CONFIG}{}", crate::remote::TEST_MACHINE),
        );
        stub_box(&fx);
        let config = fx.world.home.path().join(".claude.json");
        std::fs::write(&config, r#"{"projects":{}}"#).unwrap();
        let repo = Some(fx.repo.to_string_lossy().into_owned());
        let claude = start(&fx.world.ctx(), "demo", start_args(repo.clone(), None)).unwrap();
        assert_eq!(
            std::fs::read_to_string(config).unwrap(),
            r#"{"projects":{}}"#
        );
        assert_eq!(claude.launch.recipe_id, "test_claude");
        assert_eq!(claude.kind, Kind::Worktree);
        assert!(claude.worktree_path.is_empty());
        assert!(claude.recovery_pending);
        assert!(claude.machine.is_empty());
        assert_eq!(claude.launch.machine, "local");

        let mut args = start_args(repo, None);
        args.task = "+++\nproduct = \"web-research\"\n+++\nCompare the published results.".into();
        let agy = start(&fx.world.ctx(), "demo", args).unwrap();
        assert_eq!(agy.launch.recipe_id, "agy_gemini_flash");
        assert!(agy.machine.is_empty());
        assert_eq!(agy.launch.machine, "local");
        assert!(agy.launch.args.iter().any(|arg| arg == "--new-project"));

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
    fn killed_waiting_agent_has_retry_advice_without_restarting_or_losing_seal() {
        use crate::runner::fake::{fail, ok};
        let fx = crate::testkit::fixture();
        let lane = fx
            .world
            .thread(&fx.project, &fx.world.home.path().join("lane"), |t| {
                t.attempt = 1;
                t.launch_attempts = 1;
                t.bootstrap = "acknowledged".into();
                t.identity.workspace_id = t.workspace_id.clone();
                t.identity.tab_id = t.tab_id.clone();
                t.identity.pane_id = t.pane_id.clone();
                t.identity.process = Some(crate::contracts::ProcessIdentity {
                    pid: 42,
                    argv0: "claude".into(),
                });
            });
        thread::update(&fx.project, &lane.id, |t| {
            t.bootstrap = "acknowledged".into()
        })
        .unwrap();
        let seal = fx.seal_waiting(&lane.id, 1, 1, "Input needed");
        let seal_path = fx
            .project
            .state_dir()
            .join("events")
            .join(format!("{seal}.toml"));
        let seal_bytes = std::fs::read(&seal_path).unwrap();
        *fx.world.panes.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::pane_json(&lane.workspace_id, &lane.tab_id, &lane.pane_id, &lane.cwd,)
        );
        let ctx = fx.world.ctx();
        let probe = std::rc::Rc::new(std::cell::RefCell::new(ok("")));
        let answer = probe.clone();
        fx.world.runner.on_fn(
            |cmd| cmd.display().contains("pane process-info"),
            move |_| Ok(answer.borrow().clone()),
        );
        for (output, bound, gone) in [
            (
                ok(
                    r#"{"result":{"process_info":{"pane_id":"w2:p1","foreground_processes":[{"pid":7,"name":"bash"}]}}}"#,
                ),
                true,
                true,
            ),
            (
                ok(r#"{"result":{"process_info":{"pane_id":"w2:p1","foreground_processes":[]}}}"#),
                true,
                true,
            ),
            (fail(1, "process probe unavailable"), true, false),
            (
                ok(
                    r#"{"result":{"process_info":{"pane_id":"w2:p1","foreground_processes":[{"pid":8,"name":"opaque-tool"}]}}}"#,
                ),
                true,
                false,
            ),
            (
                ok(r#"{"result":{"process_info":{"pane_id":"other","foreground_processes":[]}}}"#),
                true,
                false,
            ),
            (
                ok(r#"{"result":{"process_info":{"pane_id":"w2:p1","foreground_processes":[]}}}"#),
                false,
                false,
            ),
        ] {
            *probe.borrow_mut() = output;
            let current = thread::update(&fx.project, &lane.id, |t| {
                t.identity.pane_id = if bound {
                    lane.pane_id.clone()
                } else {
                    "other".into()
                };
            })
            .unwrap();
            let rows = rows(&ctx, &fx.project);
            let view = crate::project_view::View::capture(
                &fx.project,
                &project::Settings::default(),
                Some(rows),
                None,
            );
            let text = view.render(&["Current work"]);
            assert!(text.contains("waiting seal retained"), "{text}");
            if gone {
                assert!(text.contains("agent gone, waiting seal kept"), "{text}");
                assert!(
                    text.contains("ha thread retry demo t-0001 --reason"),
                    "{text}"
                );
                assert!(!text.contains("process unknown"), "{text}");
            } else {
                assert!(text.contains("process unknown"), "{text}");
                assert!(!text.contains("ha thread retry"), "{text}");
            }
            let session = session_view(&ctx, &fx.project).unwrap();
            assert!(
                crate::ticker::observation_pass(
                    &ctx,
                    &fx.project,
                    crate::ticker::ObservationView {
                        machine_id: "",
                        threads: &[current],
                        agents: &session.agents,
                        panes: &session.panes,
                        boot_id: "",
                        now: jiff::Timestamp::now(),
                    }
                )
                .is_empty()
            );
            let held = thread::load(&fx.project, &lane.id).unwrap();
            assert_eq!(held.status, Status::Open);
            assert_eq!(held.attempt, 1);
            assert!(!held.recovery_pending);
            assert_eq!(held.bootstrap, "acknowledged");
            assert_eq!(std::fs::read(&seal_path).unwrap(), seal_bytes);
            assert!(attempt_sealed(&fx.project, &held));
        }
        assert_eq!(fx.world.runner.count("agent start"), 0);
        assert_eq!(fx.world.runner.count("tab close"), 0);
        assert_eq!(fx.world.runner.count("workspace close"), 0);
        assert_eq!(crate::plan::counts(&fx.project).unwrap(), (0, 0));
    }

    #[test]
    fn first_placement_reads_as_starting_not_an_unknown_retry() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        for machine in ["", "box"] {
            for attempt in [1, 2] {
                let lane = Thread {
                    id: "t-0771".into(),
                    status: Status::Open,
                    machine: machine.into(),
                    attempt,
                    recovery_pending: true,
                    ..Default::default()
                };
                assert_eq!(
                    thread::recorded_group(&lane, jiff::Timestamp::now()),
                    Group::Working
                );
                assert_eq!(
                    thread::group(&lane, &thread::Live::default(), jiff::Timestamp::now()),
                    Group::Working
                );
                let shown = row(&lane, None, jiff::Timestamp::now());
                assert!(shown.note.starts_with("starting"));
                let view = crate::project_view::View::capture(
                    &project,
                    &project::Settings::default(),
                    Some(vec![shown]),
                    None,
                );
                let rendered = view.render(&[]);
                assert!(rendered.contains("[Working]"), "{rendered}");
                assert!(rendered.contains("starting"), "{rendered}");
                assert!(!rendered.contains("[Unknown]"), "{rendered}");
                assert_eq!(
                    rendered.contains("automatic retry selected"),
                    attempt > 1,
                    "{rendered}"
                );
            }
        }
    }

    #[test]
    fn lane_waits_for_provider_and_starts_on_next_pass_without_routing_retry() {
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
            |cmd| cmd.program == "ssh" && crate::box_helper::tests::is_doctor(cmd),
            move |cmd| {
                Ok(crate::doctor::boundary_diagnostic_output(
                    cmd,
                    99_999_999,
                    (!state.get()).then_some("provider readiness probe timed out"),
                ))
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
        let other = thread::allocate(&fx.project, |t| {
            t.launch = waiting.launch.clone();
            t.machine = waiting.machine.clone();
            t.role = "lane".into();
            t.provider_wait_started = waiting.provider_wait_started.clone();
        })
        .unwrap();
        let before = fx.world.runner.count("HERDR_ADE_BOX_INPUT");
        crate::ticker::resume_provider_starts(
            &fx.world.ctx(),
            &fx.project,
            &mut std::collections::BTreeMap::new(),
            |error| panic!("{error:#}"),
        );
        assert_eq!(fx.world.runner.count("HERDR_ADE_BOX_INPUT") - before, 0);
        // The observation survives another pass, not merely its local map.
        thread::update(&fx.project, &other.id, |t| t.status = Status::Resolved).unwrap();
        ready.set(true);
        crate::adapters::expire_dependency_probe(
            &fx.world.root,
            waiting.machine_route(),
            &waiting.launch,
        );
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
        let started = place_started(&fx.world.ctx(), &fx.project, &started);
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
        let started = place_started(&fx.world.ctx(), &fx.project, &started);
        assert!(
            started
                .worktree_path
                .starts_with(&fx.repo.to_string_lossy().to_string())
        );
    }

    #[test]
    fn failed_resolve_tail_keeps_cleanup_pending_until_retry_finishes() {
        use crate::runner::fake::{FakeRunner, fail, ok};
        for authority in ["automatic", "manual", "cancel"] {
            let world = crate::scenarios::World::new();
            let project = world.project("demo", "a.sock");
            let lane = thread::allocate(&project, |t| {
                t.kind = Kind::Adopted;
                t.status = Status::Open;
                t.cwd = world.home.path().to_string_lossy().into_owned();
                t.worktree_path = t.cwd.clone();
            })
            .unwrap();
            let runner = FakeRunner::new();
            runner.on("session list", ok(&format!(r#"{{"sessions":[{{"name":"scratch-{}","running":false,"socket_path":"/scratch.sock"}}]}}"#, lane.id)));
            runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
            runner.on("pane list", ok(r#"{"result":{"panes":[]}}"#));
            let failing = std::rc::Rc::new(std::cell::Cell::new(true));
            let flag = failing.clone();
            runner.on_fn(
                |cmd| cmd.display().contains("session delete"),
                move |_| {
                    Ok(if flag.get() {
                        fail(1, "busy")
                    } else {
                        ok("{}")
                    })
                },
            );
            let mut ctx = world.ctx();
            ctx.runner = &runner;
            let notes = match authority {
                "automatic" => {
                    let outcome = resolve_automatically(&ctx, &project, &lane.id, "finished");
                    assert_eq!(outcome.state, "cleanup_pending");
                    outcome.copy_notes
                }
                "cancel" => {
                    let outcome = cancel(&ctx, "demo", &lane.id, "stop").unwrap();
                    assert_eq!(outcome.state, "cleanup_pending");
                    vec![outcome.worktree_reason.unwrap()]
                }
                _ => vec![
                    resolve(&ctx, "demo", &lane.id, &ResolveArgs::default())
                        .unwrap_err()
                        .to_string(),
                ],
            };
            let pending = thread::load(&project, &lane.id).unwrap();
            assert!(pending.cleanup_pending);
            assert!(pending.retirement.is_some());
            assert!(
                notes
                    .iter()
                    .any(|note| note.contains("could not delete scratch session")),
                "{notes:?}"
            );
            failing.set(false);
            retry_pending_cleanup(&ctx, &project).unwrap();
            let finished = thread::load(&project, &lane.id).unwrap();
            assert!(!finished.cleanup_pending);
            assert!(finished.retirement.is_none());
            assert_eq!(finished.status, Status::Resolved);
            assert_eq!(finished.cwd, lane.cwd);
            assert_eq!(finished.worktree_path, lane.worktree_path);
            assert_eq!(finished.identity, lane.identity);
            assert!(Path::new(&lane.worktree_path).exists());
            let outcome = resolve(&ctx, "demo", &lane.id, &ResolveArgs::default()).unwrap();
            assert_eq!(
                outcome.state,
                if authority == "cancel" {
                    "cancelled"
                } else {
                    "resolved"
                }
            );
            if authority == "automatic" {
                failing.set(true);
                assert!(resolve(&ctx, "demo", &lane.id, &ResolveArgs::default()).is_err());
                assert!(thread::load(&project, &lane.id).unwrap().cleanup_pending);
                resolve(
                    &ctx,
                    "demo",
                    &lane.id,
                    &ResolveArgs {
                        reopen: true,
                        ..Default::default()
                    },
                )
                .unwrap();
                retry_pending_cleanup(&ctx, &project).unwrap();
                let reopened = thread::load(&project, &lane.id).unwrap();
                assert_eq!(reopened.status, Status::Open);
                assert!(!reopened.cleanup_pending);
                assert!(reopened.retirement.is_none());
            }
        }
    }

    #[test]
    fn retirement_retries_after_linked_sources_and_checkout_are_removed() {
        let (fx, lane) = repo_link_fixture(false);
        let root = Path::new(&lane.thread_dir);
        std::fs::write(root.join("plot.txt"), "kept plot").unwrap();
        seal_linked_report(&fx.project, &lane, "[Plot](plot.txt)\n");
        struct TailFailure<'a> {
            runner: &'a dyn Runner,
            fail: std::cell::Cell<bool>,
            id: String,
        }
        impl Runner for TailFailure<'_> {
            fn run(&self, cmd: &Cmd) -> Result<crate::runner::Output> {
                use crate::runner::fake::{fail, ok};
                if cmd.display().contains("session list --json") {
                    return Ok(ok(&format!(
                        r#"{{"sessions":[{{"name":"scratch-{}","running":false,"socket_path":"/scratch.sock"}}]}}"#,
                        self.id
                    )));
                }
                if self.fail.get() && cmd.display().contains("session delete") {
                    return Ok(fail(1, "busy"));
                }
                self.runner.run(cmd)
            }
        }
        let runner = TailFailure {
            runner: &fx.world.runner,
            fail: std::cell::Cell::new(true),
            id: lane.id.clone(),
        };
        let mut ctx = fx.world.ctx();
        ctx.runner = &runner;
        let error = remove_kept_worktree(&ctx, "demo", &lane.id).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("could not delete scratch session")
        );
        assert!(!root.exists());
        let pending = thread::load(&fx.project, &lane.id).unwrap();
        assert!(pending.cleanup_pending);
        assert!(pending.retirement.as_ref().unwrap().preserved);
        assert!(pending.worktree_path.is_empty());
        runner.fail.set(false);
        retry_pending_cleanup(&ctx, &fx.project).unwrap();
        let saved = thread::load(&fx.project, &lane.id).unwrap();
        assert!(!saved.cleanup_pending, "{}", saved.cleanup_reason);
        let plot = thread::sha256_hex(b"kept plot");
        assert_eq!(
            std::fs::read(crate::events::artifact_path(&fx.project, &plot)).unwrap(),
            b"kept plot"
        );
        let report =
            std::fs::read_to_string(thread::final_report_path(&fx.project, &saved).unwrap())
                .unwrap();
        assert_eq!(report, format!("[Plot]({plot})\n"));
    }

    #[test]
    fn automatic_retirement_cannot_close_a_follow_up_to_authorize_its_retry() {
        let fx = crate::testkit::fixture();
        let (id, _) = fx.lane(1);
        thread::update(&fx.project, &id, |lane| {
            lane.follow_ups.push(FollowUp {
                attempt: 1,
                text: "Finish the correction".into(),
                state: FollowUpState::Queued,
                ..Default::default()
            });
        })
        .unwrap();
        for _ in 0..2 {
            let outcome = resolve_automatically(&fx.world.ctx(), &fx.project, &id, "finished");
            assert!(outcome.copy_notes[0].contains("follow_up_pending"));
            let saved = thread::load(&fx.project, &id).unwrap();
            assert_eq!(saved.status, Status::Open);
            assert_eq!(saved.follow_ups[0].state, FollowUpState::Queued);
            assert!(!saved.cleanup_pending);
            assert!(Path::new(&saved.worktree_path).exists());
        }
    }

    #[test]
    fn cancellation_stops_its_agent_but_not_another_checkout_user() {
        use crate::scenarios::{agent_json, pane_json};
        let fx = crate::testkit::fixture();
        let (id, _) = fx.lane(1);
        let lane = thread::load(&fx.project, &id).unwrap();
        *fx.world.panes.borrow_mut() = format!(
            "[{}]",
            pane_json(&lane.workspace_id, &lane.tab_id, &lane.pane_id, &lane.cwd,)
        );
        *fx.world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json("w9", "w9:t1", "w9:p1", &lane.cwd, "other", "working",)
        );
        let outcome = cancel(&fx.world.ctx(), "demo", &id, "stop").unwrap();
        assert_eq!(outcome.state, "cleanup_pending");
        assert!(outcome.worktree_reason.unwrap().contains("worktree_in_use"));
        assert!(Path::new(&lane.worktree_path).exists());
        assert_eq!(fx.world.runner.count("tab close"), 0);
        *fx.world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json(
                &lane.workspace_id,
                &lane.tab_id,
                &lane.pane_id,
                &lane.cwd,
                &lane.agent_name,
                "working",
            )
        );
        retry_pending_cleanup(&fx.world.ctx(), &fx.project).unwrap();
        let finished = thread::load(&fx.project, &id).unwrap();
        assert!(!finished.cleanup_pending, "{}", finished.cleanup_reason);
        assert!(finished.worktree_path.is_empty());
        assert!(!Path::new(&lane.worktree_path).exists());
        assert_eq!(fx.world.runner.count("tab close"), 1);
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
        let herdr = Herdr::new("herdr", "test.sock", &runner);
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
            let herdr = Herdr::new("herdr", "test.sock", &runner);
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
            let herdr = Herdr::new("herdr", "test.sock", &runner);
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
        let herdr = Herdr::new("herdr", "test.sock", &runner);
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
        let herdr = Herdr::new("herdr", "test.sock", &changed);
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
    fn a_native_provider_failure_waits_on_the_selected_machine_without_fallback() {
        let (fx, _) = box_fixture();
        write_config(&fx, &lane_config());
        fx.world.runner.on_fn(
            |cmd| {
                cmd.program == "ssh"
                    && cmd
                        .stdin
                        .as_ref()
                        .is_some_and(|input| input.contains("\"kind\":\"claude\""))
            },
            |cmd| {
                Ok(crate::doctor::boundary_diagnostic_output(
                    cmd,
                    99_999_999,
                    Some("Usage limit reached"),
                ))
            },
        );
        stub_box(&fx);
        let waiting = start(
            &fx.world.ctx(),
            "demo",
            start_args(Some(fx.repo.to_string_lossy().into_owned()), None),
        )
        .unwrap();
        assert_eq!(waiting.machine, "buildbox");
        assert_eq!(waiting.machine_id, "buildbox-id");
        assert!(!waiting.provider_wait_started.is_empty());
        assert_eq!(waiting.launch_attempts, 0);
        assert!(waiting.pane_id.is_empty());
        assert!(
            fx.world
                .runner
                .calls
                .borrow()
                .iter()
                .all(|cmd| cmd.program != "claude")
        );
        assert_eq!(fx.world.runner.count("workspace create"), 0);
        assert_eq!(fx.world.runner.count("tab create"), 0);
    }

    #[test]
    fn install_skew_records_job_intent_waits_without_expiry_then_places_same_attempt() {
        for explicit in [false, true] {
            let (fx, _) = box_fixture();
            write_config(&fx, &lane_config());
            let ready = std::rc::Rc::new(std::cell::Cell::new(false));
            let installed = ready.clone();
            fx.world.runner.on_fn(
                |cmd| cmd.program == "ssh" && crate::box_helper::tests::is_doctor(cmd),
                move |cmd| {
                    Ok(if installed.get() {
                        crate::doctor::boundary_diagnostic_output(cmd, 99_999_999, None)
                    } else {
                        crate::runner::fake::ok(r#"{"status":"Skew","build":"0.1.0+old.1"}"#)
                    })
                },
            );
            stub_box(&fx);
            crate::prompt::record_test_request(
                &fx.project,
                "q-skew",
                "Start on the box during install.",
            )
            .unwrap();
            let job = crate::task::add(
                &fx.project,
                "Start on the box",
                vec!["request:q-skew".into()],
                vec!["The start survives install skew.".into()],
                Some(fx.repo.to_string_lossy().into_owned()),
                None,
            )
            .unwrap();
            let mut args = start_args(
                Some(fx.repo.to_string_lossy().into_owned()),
                explicit.then(|| "buildbox".into()),
            );
            args.task_id = job.id.clone();
            let waiting = start(&fx.world.ctx(), "demo", args).unwrap();
            assert_eq!(waiting.machine, "buildbox");
            assert_eq!(waiting.launch_attempts, 0);
            assert!(waiting.pane_id.is_empty());
            assert!(waiting.error.contains("build old") || waiting.error.contains("0.1.0+old.1"));
            assert!(
                crate::task::load(&fx.project, &job.id)
                    .unwrap()
                    .attempts
                    .contains(&waiting.id)
            );
            thread::update(&fx.project, &waiting.id, |t| {
                t.provider_wait_started = "2020-01-01T00:00:00Z".into()
            })
            .unwrap();
            crate::ticker::resume_provider_starts(
                &fx.world.ctx(),
                &fx.project,
                &mut BTreeMap::new(),
                |error| panic!("{error:#}"),
            );
            let held = thread::load(&fx.project, &waiting.id).unwrap();
            assert_eq!(held.status, Status::Starting);
            assert_eq!(held.attempt, waiting.attempt);
            assert_eq!(fx.world.runner.count("workspace create"), 0);
            assert!(crate::events::list(&fx.project).is_empty());
            ready.set(true);
            crate::ticker::resume_provider_starts(
                &fx.world.ctx(),
                &fx.project,
                &mut BTreeMap::new(),
                |error| panic!("{error:#}"),
            );
            let placed = thread::load(&fx.project, &waiting.id).unwrap();
            assert_eq!(placed.attempt, waiting.attempt);
            assert!(placed.provider_wait_started.is_empty());
            assert!(!placed.pane_id.is_empty());
            assert_eq!(thread::list(&fx.project).len(), 1);
        }
    }

    #[test]
    fn an_unreachable_box_defers_without_fallback_or_refusal() {
        let (fx, _remote) = box_fixture();
        write_config(&fx, &lane_config());
        fx.world.runner.on_fn(
            |cmd| {
                cmd.program == "ssh"
                    && cmd
                        .stdin
                        .as_ref()
                        .is_some_and(|input| input.contains("\"kind\":\"claude\""))
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
