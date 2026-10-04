//! Real, bounded post-install plumbing checks. Only run-owned scratch state is read.
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::runner::{Cmd, Output, Runner};

const COMMAND: Duration = Duration::from_secs(60);
const OBSERVE: Duration = Duration::from_secs(180);
const WHOLE_RUN: Duration = Duration::from_secs(720);
const SHUTDOWN: Duration = Duration::from_secs(120);

struct BoundedRunner<'a> {
    inner: &'a dyn Runner,
    deadline: Instant,
}
impl Runner for BoundedRunner<'_> {
    fn run(&self, cmd: &Cmd) -> Result<Output> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!("whole journey deadline (12 minutes)");
        }
        let mut cmd = cmd.clone();
        cmd.timeout = cmd.timeout.min(left);
        cmd.own_group = true;
        self.inner.run(&cmd)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Passes {
    pid: u32,
    build: String,
    started: String,
    passes: Vec<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
struct InstallRequest {
    pid: u32,
    build: String,
    started: String,
}

fn installed_ticker(root: &Path) -> Result<InstallRequest> {
    match crate::ticker::lock_state(root) {
        crate::ticker::LockState::Held(info) => Ok(InstallRequest {
            pid: info.pid,
            build: info.version,
            started: info.started,
        }),
        _ => bail!("installed ticker ownership is unknown"),
    }
}

fn pass_path(root: &Path, pid: u32) -> std::path::PathBuf {
    root.join(".ticker.first-passes")
        .join(format!("{pid}.json"))
}

pub(crate) fn ticker_started(root: &Path, started: &str) -> Result<()> {
    std::fs::create_dir_all(root.join(".ticker.first-passes"))?;
    project::write_json(
        &pass_path(root, std::process::id()),
        &Passes {
            pid: std::process::id(),
            build: crate::VERSION.into(),
            started: started.into(),
            passes: Vec::new(),
        },
    )
}

// Called only at the end of a FULL pass, never from partial health publication.
pub(crate) fn ticker_pass(root: &Path, failures: &[String]) -> Result<()> {
    let path = pass_path(root, std::process::id());
    let Some(mut record) = project::read_json::<Passes>(&path) else {
        return Ok(());
    };
    if record.pid == std::process::id() && record.passes.len() < 2 {
        record.passes.push(failures.to_vec());
        project::write_json(&path, &record)?;
    }
    Ok(())
}

fn observation_result(record: &Passes, expected: &str) -> Option<String> {
    if !crate::build::same_commit(&record.build, expected) {
        return None;
    }
    let first = record.passes.first()?;
    if first.is_empty() {
        return Some("ticker first full pass: PASS (zero missing observations)".into());
    }
    let second = record.passes.get(1)?;
    Some(format!(
        "ticker first full pass: FAIL: {}; {}",
        first.join("; "),
        if second.is_empty() {
            "transient: cleared by second full pass".into()
        } else {
            format!("second full pass still missing: {}", second.join("; "))
        }
    ))
}

#[derive(Debug, Serialize, Deserialize)]
struct Step {
    name: String,
    seconds: f64,
    passed: bool,
    evidence: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Report {
    project: String,
    #[serde(default)]
    created: bool,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    observation: String,
    steps: Vec<Step>,
}

impl Report {
    fn step(&mut self, name: &str, action: impl FnOnce() -> Result<String>) -> Result<()> {
        let started = Instant::now();
        let result = action();
        self.steps.push(Step {
            name: name.into(),
            seconds: started.elapsed().as_secs_f64(),
            passed: result.is_ok(),
            evidence: match &result {
                Ok(s) => s.clone(),
                Err(e) => format!("{e:#}"),
            },
        });
        result.map(|_| ())
    }

    fn notice(&self) -> String {
        let mut text = self
            .steps
            .iter()
            .map(|s| {
                format!(
                    "{} {} ({:.1}s): {}",
                    if s.passed { "PASS" } else { "FAIL" },
                    s.name,
                    s.seconds,
                    s.evidence
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        if self.steps.iter().any(|s| !s.passed) {
            if self.deleted {
                text.push_str(
                    "; project already removed; run-owned scratch kept for cleanup inspection",
                );
            } else if self.created {
                text.push_str(&format!(
                    "; kept {} for inspection; delete: ha delete {}",
                    self.project, self.project
                ));
            } else {
                text.push_str("; scratch kept for inspection; no project created by this run (do not delete a colliding project)");
            }
        }
        text
    }
}

#[derive(Serialize, Deserialize)]
struct Owned {
    slug: String,
    reviewer_recipe: String,
    device: u64,
    inode: u64,
}

fn project_identity(project: &Project) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(project.dir())?;
    if !metadata.is_dir() {
        bail!("journey project is not an owned directory");
    }
    Ok((metadata.dev(), metadata.ino()))
}

// No global routing changes: only a project with this run-owned marker gets the
// small reviewer. Historical projects/records remain untouched.
pub(crate) fn reviewer_recipe(project: &Project) -> Option<String> {
    let owned: Owned = project::read_json(&project.record_file("journey.json"))?;
    (owned.slug == project.slug
        && project.slug.starts_with("journey-")
        && project_identity(project).ok() == Some((owned.device, owned.inode)))
    .then_some(owned.reviewer_recipe)
}

fn cheap_recipe(config: &crate::launch::LaunchConfig, claude: bool) -> Result<String> {
    // Recipes have no numeric price field. Use the configured small model tier;
    // non-Claude bootstraps avoid pre-trusting the project (N7 tests a worktree).
    config
        .recipes
        .iter()
        .filter(|(_, r)| {
            r.enabled
                && if claude {
                    r.kind == "claude"
                } else {
                    r.kind == "pi"
                }
        })
        .min_by_key(|(id, r)| {
            let text = format!("{} {}", id, r.args.join(" ")).to_lowercase();
            (
                if text.contains("haiku") {
                    0
                } else if text.contains("mini") || text.contains("flash") {
                    1
                } else if text.contains("low") {
                    2
                } else {
                    3
                },
                (*id).clone(),
            )
        })
        .map(|(id, _)| id.clone())
        .context("no enabled small recipe for the required driver")
}

fn command(ctx: &Ctx, cmd: Cmd) -> Result<String> {
    let out = ctx.runner.run(&cmd.clone().own_group())?;
    if !out.success() {
        bail!(
            "{} {:?}: exit {:?}, timeout={}; {}",
            cmd.program,
            cmd.args,
            out.code,
            out.timed_out,
            out.error_text()
        );
    }
    Ok(out.stdout.trim().into())
}

fn ha(ctx: &Ctx, args: &[&str]) -> Result<String> {
    command(
        ctx,
        Cmd::new(
            ctx.env.home.join(".local/bin/herdr-ade").to_string_lossy(),
            COMMAND,
        )
        .args(["--root", &ctx.root.to_string_lossy(), "--json"])
        .args(args.iter().copied())
        .env_remove("HERDR_ADE_LAUNCH")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_TAB_ID")
        .env_remove("HERDR_WORKSPACE_ID"),
    )
}

fn git(ctx: &Ctx, cwd: &Path, args: &[&str]) -> Result<String> {
    command(
        ctx,
        Cmd::new("git", COMMAND).cwd(cwd).args(args.iter().copied()),
    )
}

fn poll<T>(
    name: &str,
    timeout: Duration,
    mut check: impl FnMut() -> Result<(Option<T>, String)>,
) -> Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        let (value, evidence) = check()?;
        if let Some(value) = value {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            bail!("{name} deadline: {evidence}");
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn bounded_poll<T>(
    deadline: Instant,
    name: &str,
    timeout: Duration,
    check: impl FnMut() -> Result<(Option<T>, String)>,
) -> Result<T> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        bail!("whole journey deadline (12 minutes): {name}");
    }
    poll(name, timeout.min(left), check)
}

#[derive(Clone)]
struct OwnedSession {
    name: String,
    socket: std::path::PathBuf,
    inode: u64,
}

#[derive(Default)]
struct RunResources {
    server: Option<Child>,
    session: Option<OwnedSession>,
}

fn observe_session(ctx: &Ctx, name: &str) -> Result<OwnedSession> {
    let info = crate::herdr::session_list(&ctx.env.herdr_bin(), ctx.runner)?
        .into_iter()
        .find(|s| s.name == name && s.running)
        .context("owned session not running")?;
    let inode = crate::ticker::socket_inode(&info.socket_path);
    if inode == 0 {
        bail!("owned session socket identity unavailable");
    }
    Ok(OwnedSession {
        name: info.name,
        socket: info.socket_path,
        inode,
    })
}

fn stop_session(ctx: &Ctx, owned: &OwnedSession) -> Result<()> {
    let sessions = crate::herdr::session_list(&ctx.env.herdr_bin(), ctx.runner)?;
    if let Some(current) = sessions.iter().find(|s| s.name == owned.name) {
        if current.socket_path != owned.socket {
            bail!("session socket path changed; nothing stopped");
        }
        if current.running {
            if crate::ticker::socket_inode(&current.socket_path) != owned.inode {
                bail!("session incarnation changed; nothing stopped");
            }
            command(
                ctx,
                Cmd::new(ctx.env.herdr_bin(), COMMAND).args(["session", "stop", &owned.name]),
            )?;
        }
    }
    let sessions = crate::herdr::session_list(&ctx.env.herdr_bin(), ctx.runner)?;
    if sessions.iter().any(|s| s.name == owned.name && s.running) {
        bail!("owned session is still running");
    }
    Ok(())
}

fn cleanup_effect(errors: &mut Vec<String>, name: &str, action: impl FnOnce() -> Result<()>) {
    if let Err(error) = action() {
        errors.push(format!("{name}: {error:#}"));
    }
}

fn guard_lane(
    ctx: &Ctx,
    project: &Project,
    lane: &crate::thread::Thread,
    session: Option<&OwnedSession>,
) -> Result<()> {
    if lane.pane_id.is_empty() {
        return Ok(());
    }
    let socket = project.coordinator().map(|c| c.socket).unwrap_or_default();
    if !lane.is_remote() {
        let session = session.context("local session ownership unavailable")?;
        if Path::new(&socket) != session.socket
            || crate::ticker::socket_inode(&session.socket) != session.inode
        {
            bail!("local session incarnation changed; nothing stopped");
        }
    }
    let herdr = crate::herdr::Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner)
        .on_machine(lane.machine_route());
    if herdr
        .agent_list()?
        .iter()
        .any(|a| a.pane_id == lane.pane_id && !crate::thread::agent_matches(lane, a))
    {
        bail!("{} pane occupant changed; nothing stopped", lane.id);
    }
    Ok(())
}

// Cleanup has its own reserved budget. Use the existing cancellation/retirement
// drivers; retain checkouts even when they are clean or final copying fails.
fn shutdown_failed_run(
    ctx: &Ctx,
    report: &Report,
    resources: &mut RunResources,
    deadline: Instant,
) -> Result<String> {
    let bounded = BoundedRunner {
        inner: ctx.runner,
        deadline,
    };
    let ctx = Ctx {
        runner: &bounded,
        root: ctx.root.clone(),
        config_dir: ctx.config_dir.clone(),
        env: ctx.env,
        detached_ticker: false,
    };
    let mut errors = Vec::new();
    let mut lanes = Vec::new();
    let mut coordinator = None;
    if report.created && !report.deleted {
        let project = owned_project(&ctx, &report.project)?;
        // Pausing prevents new allocations; it is NOT the process shutdown.
        project.set_status(project::Status::Paused)?;
        coordinator = project.coordinator();
        project.update_coordinator(|c| c.closed_by_rolf_at = project::now())?;
        let (records, gaps) = crate::thread::list_with_errors(&project);
        lanes = records;
        errors.extend(gaps.into_iter().map(|e| format!("lane records: {e:#}")));
        let reviews = match crate::review::list(&project) {
            Ok(reviews) => reviews,
            Err(error) => {
                errors.push(format!("review records: {error:#}"));
                Vec::new()
            }
        };
        // A cancelled review releases its members before their cancellations.
        // Never roll back an already-landed review or change publication facts.
        for review in reviews {
            cleanup_effect(&mut errors, &review.id, || {
                if let Some(id) = &review.reviewer
                    && let Some(lane) = lanes.iter().find(|lane| &lane.id == id)
                {
                    guard_lane(&ctx, &project, lane, resources.session.as_ref())?;
                }
                crate::review::cancel_for_diagnosis(&ctx, &project, &review.id)
            });
        }
        for lane in &lanes {
            cleanup_effect(&mut errors, &lane.id, || {
                guard_lane(&ctx, &project, lane, resources.session.as_ref())?;
                let cancelled = crate::threads::cancel_preserving_checkout(
                    &ctx,
                    &project.slug,
                    &lane.id,
                    "journey failed or timed out; retain diagnosis evidence",
                );
                // Even pending preservation must not leave the process alive.
                // close_pane rechecks the recorded workspace/tab/pane identity.
                let current = crate::thread::load(&project, &lane.id)?;
                crate::threads::close_pane(&ctx, &project, &current)?;
                let cancelled = cancelled?;
                if cancelled.state == "cleanup_pending" {
                    bail!(
                        "preservation/retirement pending; {}",
                        current.cleanup_reason
                    );
                }
                Ok(())
            });
        }
        if let Some(c) = &coordinator {
            cleanup_effect(&mut errors, "coordinator", || {
                if c.socket.is_empty() {
                    return Ok(());
                }
                let session = resources
                    .session
                    .as_ref()
                    .context("coordinator session ownership unavailable")?;
                if Path::new(&c.socket) != session.socket
                    || crate::ticker::socket_inode(&session.socket) != session.inode
                {
                    bail!("coordinator session incarnation changed; nothing stopped");
                }
                let herdr = crate::herdr::Herdr::new(ctx.env.herdr_bin(), &c.socket, ctx.runner);
                let agents = herdr.agent_list()?;
                if agents
                    .iter()
                    .any(|a| a.pane_id == c.pane_id && !crate::coordinator::agent_matches(c, a))
                {
                    bail!("coordinator occupant changed; nothing stopped");
                }
                if herdr
                    .pane_list()?
                    .iter()
                    .any(|p| crate::coordinator::pane_matches(c, p))
                {
                    herdr.tab_close(&c.tab_id)?;
                }
                crate::coordinator::close(&ctx, &project.slug)
            });
        }
        // Remote sessions are shared: verify only this run's recorded panes,
        // and never stop the saved machine's whole session.
        for lane in lanes
            .iter()
            .filter(|lane| lane.is_remote() && !lane.pane_id.is_empty())
        {
            cleanup_effect(&mut errors, &format!("{} stopped receipt", lane.id), || {
                let herdr = crate::herdr::Herdr::new(ctx.env.herdr_bin(), "", ctx.runner)
                    .on_machine(lane.machine_route());
                if herdr.pane_list()?.iter().any(|p| p.pane_id == lane.pane_id)
                    || herdr
                        .agent_list()?
                        .iter()
                        .any(|a| a.pane_id == lane.pane_id)
                {
                    bail!("run-owned remote process/pane is still present");
                }
                Ok(())
            });
        }
    }
    // Startup can time out before returning the session receipt. Discovery is
    // authorized only by this run's still-live server child, never by a name alone.
    if resources.session.is_none()
        && let Some(server) = resources.server.as_mut()
        && server.try_wait()?.is_none()
    {
        resources.session = Some(observe_session(
            &ctx,
            &format!("scratch-{}", report.project),
        )?);
    }
    if let Some(session) = &resources.session {
        cleanup_effect(&mut errors, "isolated session", || {
            let herdr = crate::herdr::Herdr::new(ctx.env.herdr_bin(), &session.socket, ctx.runner);
            let current = crate::herdr::session_list(&ctx.env.herdr_bin(), ctx.runner)?;
            if current.iter().any(|s| s.name == session.name && s.running) {
                if crate::ticker::socket_inode(&session.socket) != session.inode {
                    bail!("session incarnation changed");
                }
                let agents = herdr.agent_list()?;
                if agents.iter().any(|a| {
                    !lanes
                        .iter()
                        .filter(|l| !l.is_remote())
                        .any(|l| crate::thread::agent_matches(l, a))
                        && !coordinator
                            .as_ref()
                            .is_some_and(|c| crate::coordinator::agent_matches(c, a))
                }) {
                    bail!("unowned agent in isolated session; session not stopped");
                }
                // The run-created session also owns its initial shells and
                // panes allocated just before a command wrapper timed out.
                // Stop that exact incarnation, not merely the observed agents.
            }
            stop_session(&ctx, session)
        });
    }
    if let Some(server) = resources.server.as_mut() {
        cleanup_effect(&mut errors, "server exit receipt", || {
            bounded_poll(deadline, "owned server exit", COMMAND, || {
                Ok((
                    server.try_wait()?.map(|_| ()),
                    "owned server still alive".into(),
                ))
            })
        });
    }
    if !errors.is_empty() {
        bail!("shutdown not fully verified: {}", errors.join("; "));
    }
    Ok("owned agents/panes stopped; isolated session stopped; project, checkouts, refs and evidence retained".into())
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(mut cmd: Command) -> Result<Process> {
    Ok(Process(
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    ))
}

fn owned_project(ctx: &Ctx, slug: &str) -> Result<Project> {
    let project = Project::load(&ctx.root, slug)?;
    let owned: Owned = project::read_json(&project.record_file("journey.json"))
        .context("journey ownership marker missing; no deletion authorized")?;
    if owned.slug != slug
        || !slug.starts_with("journey-")
        || project_identity(&project)? != (owned.device, owned.inode)
    {
        bail!("journey identity mismatch");
    }
    Ok(project)
}

fn seal(project: &Project, id: &str) -> Result<(Option<String>, String)> {
    let lane = crate::thread::load(project, id)?;
    let events = crate::events::checked_for_thread(project, id)?;
    let done = crate::events::latest_done_event(&events, id, lane.attempt.max(1));
    if let Some(event) = crate::events::latest_event(&events, id, lane.attempt.max(1)) {
        if let Some(waiting) = &event.payload.waiting {
            bail!("{id} sealed waiting: {}", waiting.text);
        }
        if let Some(failed) = &event.payload.failed {
            bail!("{id} sealed failed: {failed:?}");
        }
    }
    if matches!(lane.status, crate::thread::Status::Failed) {
        bail!("{id}: {:?}; {}", lane.status, lane.observation_error);
    }
    Ok((
        done.map(|event| event.created.clone()),
        format!(
            "{id}: {:?}, pane={}, observation={}",
            lane.status, lane.pane_id, lane.observation_error
        ),
    ))
}

fn sealed_sha(project: &Project, id: &str) -> Result<String> {
    let lane = crate::thread::load(project, id)?;
    let events = crate::events::checked_for_thread(project, id)?;
    crate::events::latest_done_event(&events, id, lane.attempt.max(1))
        .and_then(|event| event.payload.done.as_ref())
        .map(|done| done.sha.clone())
        .context("seal has no commit SHA")
}

pub(crate) fn independent_review(
    ready: Option<&crate::review::Review>,
    later_lane: &str,
    later_sealed: bool,
) -> Result<bool> {
    if later_sealed {
        bail!("D22 timing not established: box sealed before an independent review was observed");
    }
    let Some(review) = ready else {
        return Ok(false);
    };
    if review.members.iter().any(|m| m.thread == later_lane) {
        bail!("later box lane was folded into older seal's review");
    }
    Ok(review.reviewer.is_some())
}

fn start_lane(
    ctx: &Ctx,
    slug: &str,
    repo: &Path,
    recipe: &str,
    machine: &str,
    brief: &Path,
    request: &str,
) -> Result<String> {
    let output = ha(
        ctx,
        &[
            "thread",
            "start",
            slug,
            "--repo",
            &repo.to_string_lossy(),
            "--machine",
            machine,
            "--recipe",
            recipe,
            "--task-file",
            &brief.to_string_lossy(),
            "--title",
            &format!("journey {machine} {recipe}"),
            "--request",
            request,
            "--acceptance",
            "Write the named file, commit it, and seal with ha done",
        ],
    )?;
    let value: serde_json::Value = serde_json::from_str(&output)?;
    value["data"]["id"]
        .as_str()
        .map(str::to_string)
        .context("thread start has no id")
}

fn brief(path: &Path, file: &str, delay: bool) -> Result<()> {
    std::fs::write(
        path,
        format!(
            "Post-install plumbing probe, not judgment. Do not inspect any other project. {}Write only `{file}` containing `journey passed`, git add that file and commit. Write your required report at the path in the frozen brief, leave it untracked, then run `ha done`. No other work. If Claude, use --model claude-haiku-4-5-20251001; never use Agent/subagents.\n",
            if delay {
                "First run `sleep 20` (bounded timing probe for D22). "
            } else {
                ""
            }
        ),
    )?;
    Ok(())
}

fn prove_untrusted(ctx: &Ctx, folder: &Path) -> Result<()> {
    let config = match std::fs::read(ctx.env.home.join(".claude.json")) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
            .context("Claude trust configuration is unreadable")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::Value::Null,
        Err(error) => return Err(error.into()),
    };
    let canonical = std::fs::canonicalize(folder)?;
    for ancestor in canonical.ancestors().chain(folder.ancestors()) {
        if config
            .get("projects")
            .and_then(|p| p.get(ancestor.to_str()?))
            .and_then(|p| p.get("hasTrustDialogAccepted"))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            bail!(
                "N7 cannot be established: scratch folder has a previously trusted ancestor {}",
                ancestor.display()
            );
        }
    }
    Ok(())
}

fn prepare_repo(ctx: &Ctx, path: &Path) -> Result<()> {
    std::fs::create_dir(path)?;
    git(ctx, path, &["init", "-b", "main"])?;
    git(ctx, path, &["config", "user.name", "Journey"])?;
    git(ctx, path, &["config", "user.email", "journey@localhost"])?;
    git(ctx, path, &["config", "commit.gpgsign", "false"])?;
    git(
        ctx,
        path,
        &[
            "commit",
            "--allow-empty",
            "-m",
            &format!("Journey seed {}", path.display()),
        ],
    )?;
    Ok(())
}

fn prepare_transport_repo(
    ctx: &Ctx,
    scratch: &Path,
    url: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let repo = scratch.join("repo");
    let bare = scratch.join("remote.git");
    prepare_repo(ctx, &repo)?;
    git(
        ctx,
        scratch,
        &["init", "--bare", bare.to_str().context("bare path")?],
    )?;
    git(
        ctx,
        scratch,
        &[
            "--git-dir",
            bare.to_str().context("bare path")?,
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ],
    )?;
    git(
        ctx,
        &repo,
        &[
            "remote",
            "add",
            "journey",
            bare.to_str().context("bare path")?,
        ],
    )?;
    // Box preflight requires a URL-matched remote in the Mac clone. Landing
    // still uses `journey`, the local bare path, not this transport remote.
    git(ctx, &repo, &["remote", "add", "journey-transport", url])?;
    git(ctx, &repo, &["push", "journey", "main"])?;
    Ok((repo, bare))
}

fn run_steps(
    ctx: &Ctx,
    scratch: &Path,
    report: &mut Report,
    deadline: Instant,
    resources: &mut RunResources,
) -> Result<()> {
    let slug = report.project.clone();
    let repo = scratch.join("repo");
    let bare = scratch.join("remote.git");
    let trust_repo = scratch.join("trust-repo");
    let session = format!("scratch-{slug}");
    let catalog = crate::launch::recipe_catalog(&ctx.config_dir)?;
    let small = cheap_recipe(&catalog, false)?;
    let claude = cheap_recipe(&catalog, true)?;
    if !catalog.recipes[&claude]
        .args
        .iter()
        .any(|a| a == "claude-haiku-4-5-20251001")
    {
        bail!(
            "cheap Claude recipe {claude} must configure --model claude-haiku-4-5-20251001 for the throwaway probe"
        );
    }
    let label = crate::remote::declared_machine_labels(&ctx.config_dir)?
        .into_iter()
        .next()
        .context("no saved box configured")?;
    let profile =
        crate::remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, &label)?;
    let machine = crate::remote::machine_declaration(&ctx.config_dir, &label)?;
    if !machine.runs_kind(&catalog.recipes[&small].kind) {
        bail!("box {label} does not run recipe {small}");
    }
    let box_scratch = format!("{}/{}", machine.build.trim_end_matches('/'), slug);
    let box_repo = format!("{box_scratch}/repo");
    // A loopback-only Git daemon plus an SSH reverse forward lets BOTH machines
    // use the same publish URL without GitHub or a copied credential. Landing's
    // push_remote is the local bare path, not the daemon URL.
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let url = format!("git://127.0.0.1:{port}/remote.git");
    let mut daemon = None;
    let mut tunnel = None;
    let mut request = String::new();
    let mut created = false;
    let mut box_identity = String::new();
    let new_result = report.step("new", || {
        prepare_transport_repo(ctx, scratch, &url)?;
        let seed = git(ctx, &repo, &["rev-parse", "HEAD"])?;
        let mut cmd = Command::new("git");
        cmd.args(["daemon", "--listen=127.0.0.1", &format!("--port={port}"), &format!("--base-path={}", scratch.display()), "--export-all", "--enable=receive-pack", "--timeout=30", "--init-timeout=10", scratch.to_str().context("scratch path")?]);
        daemon = Some(spawn(cmd)?);
        bounded_poll(deadline, "local bare transport", Duration::from_secs(10), || {
            let out = ctx.runner.run(&Cmd::new("git", Duration::from_secs(2)).args(["ls-remote", &url, "main"]))?;
            let matches = out.success() && out.stdout.split_whitespace().next() == Some(seed.as_str());
            Ok((matches.then_some(()), format!("expected own seed {seed}; received {}; {}", out.stdout.trim(), out.error_text())))
        })?;
        let mut cmd = Command::new("ssh");
        cmd.args(["-NT", "-o", "BatchMode=yes", "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=2", "-R", &format!("127.0.0.1:{port}:127.0.0.1:{port}"), &profile.target]);
        tunnel = Some(spawn(cmd)?);
        // Refuse an existing folder rather than adopt somebody else's repo.
        let script = crate::remote::with_path(&machine.path, &format!("set -e; test ! -e {dir}; mkdir {dir}; git clone {url} {repo} >&2; git -C {repo} config user.name Journey; git -C {repo} config user.email journey@localhost; git -C {repo} config commit.gpgsign false; stat -c '%d:%i' {dir}", dir=crate::remote::quote(&box_scratch), url=crate::remote::quote(&url), repo=crate::remote::quote(&box_repo)));
        bounded_poll(deadline, "box local-bare transport", Duration::from_secs(15), || {
            let out = crate::remote::ssh(ctx.runner, &profile.target, &format!("git ls-remote {} main", crate::remote::quote(&url)), None, Duration::from_secs(3))?;
            let matches = out.success() && out.stdout.split_whitespace().next() == Some(seed.as_str());
            Ok((matches.then_some(()), format!("expected own seed {seed}; received {}; {}", out.stdout.trim(), out.error_text())))
        })?;
        let out = crate::remote::ssh(ctx.runner, &profile.target, &script, None, COMMAND)?;
        if !out.success() { bail!("box scratch clone: {}", out.error_text()); }
        ha(ctx, &["new", &slug, "--goal", "Automated journey owns all work. Coordinator: run ha context to acknowledge priming, then remain idle. Do not plan, start work, or prompt workers.", "--repo", repo.to_str().context("repo path")?])?;
        created = true;
        box_identity = out.stdout.trim().into();
        if box_identity.is_empty() { bail!("box scratch identity missing"); }
        let project = Project::load(&ctx.root, &slug)?;
        let (device, inode) = project_identity(&project)?;
        project::write_json(&project.record_file("journey.json"), &Owned { slug: slug.clone(), reviewer_recipe: small.clone(), device, inode })?;
        let (mut settings, _) = project.read_project_md()?;
        settings.repos[0].branch = Some("main".into());
        settings.repos[0].push_remote = Some("journey".into());
        settings.repos[0].box_path = Some(box_repo.clone());
        settings.repos[0].publish_url = Some(url.clone());
        settings.repos[0].gates = Some(Vec::new());
        project::write_atomic(&project.project_md(), format!("+++\n{}+++\n", toml::to_string(&settings)?).as_bytes())?;
        request = format!("request:{}", crate::prompt::record_pane_request(&project, "Automated journey requested after install: use the cheapest configured recipes for this throwaway project, write files, seal, review, land to its local bare remote, test Claude trust, delete only this project.")?);
        Ok(format!("{slug}; local bare {}; box {label}; recipes {small}/{claude}", bare.display()))
    });
    report.created = created;
    new_result?;
    let project = owned_project(ctx, &slug)?;
    report.step("open and prime coordinator", || {
        let mut server = Command::new(ctx.env.herdr_bin());
        server.args(["--session", &session, "server"]);
        use std::os::unix::process::CommandExt;
        if crate::herdr::session_list(&ctx.env.herdr_bin(), ctx.runner)?.iter().any(|s| s.name == session) { bail!("session name already exists; nothing adopted"); }
        resources.server = Some(server.process_group(0).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?);
        bounded_poll(deadline, "isolated session", Duration::from_secs(15), || {
            let exists = crate::herdr::session_list(&ctx.env.herdr_bin(), ctx.runner)?.iter().any(|s| s.name == session);
            Ok((exists.then_some(()), format!("session {session} absent")))
        })?;
        resources.session = Some(observe_session(ctx, &session)?);
        ha(ctx, &["open", &slug, "--session", &session, "--recipe", &small, "--basis", &request])?;
        bounded_poll(deadline, "coordinator bootstrap", Duration::from_secs(90), || {
            let c = project.coordinator().context("coordinator binding absent")?;
            let registered = crate::herdr::Herdr::new(ctx.env.herdr_bin(), &c.socket, ctx.runner).agent_list()?.iter().any(|a| crate::coordinator::agent_matches(&c, a));
            Ok(((registered && c.bootstrap == "acknowledged" && !c.prime_pending).then_some(()), format!("registered={registered}, bootstrap={}, prime_pending={}", c.bootstrap, c.prime_pending)))
        })?;
        let c = project.coordinator().context("coordinator absent")?;
        crate::herdr::Herdr::new(ctx.env.herdr_bin(), &c.socket, ctx.runner).tab_close(&c.tab_id)?;
        ha(ctx, &["close", &slug])?;
        // Automatic reviews need the project's session binding. Reopen an idle
        // session host after proving the first coordinator was primed and closed.
        ha(ctx, &["open", &slug, "--session", &session, "--recipe", &small, "--basis", &request])?;
        ha(ctx, &["review", &slug])?;
        Ok(format!("registered, context receipt acknowledged, closed {}; reopened idle session host; automatic reviews enabled", c.pane_id))
    })?;
    let mac_brief = scratch.join("mac.md");
    let box_brief = scratch.join("box.md");
    brief(&mac_brief, "mac.txt", false)?;
    brief(&box_brief, "box.txt", true)?;
    let mut mac = String::new();
    let mut mac_seal = String::new();
    let mut box_lane = String::new();
    // Keep automatic review from winning the race before the later lane exists.
    // Releasing the existing operation lock (not a manual review start) makes
    // the ticker exercise D22 with an older seal AND an unsealed later lane.
    let review_start_lock = crate::review::try_operation_lock(ctx, &repo.to_string_lossy())?
        .context("journey review start lock busy")?;
    report.step("Mac lane seals", || {
        mac = start_lane(ctx, &slug, &repo, &small, "local", &mac_brief, &request)?;
        mac_seal = bounded_poll(deadline, "Mac seal", OBSERVE, || seal(&project, &mac))?;
        let sha = sealed_sha(&project, &mac)?;
        git(ctx, &repo, &["cat-file", "-e", &format!("{sha}:mac.txt")])?;
        Ok(format!(
            "{mac} sealed at {mac_seal}, commit {sha} contains mac.txt"
        ))
    })?;
    report.step("later box lane and D22 automatic review", || {
        // Records round to seconds: make later-start evidence unambiguous.
        std::thread::sleep(Duration::from_secs(1));
        box_lane = start_lane(ctx, &slug, &repo, &small, &label, &box_brief, &request)?;
        let lane = crate::thread::load(&project, &box_lane)?;
        if lane.created.parse::<jiff::Timestamp>()? <= mac_seal.parse::<jiff::Timestamp>()? {
            bail!(
                "D22 sequence not established: box creation {} <= Mac seal {mac_seal}",
                lane.created
            );
        }
        if !lane.is_remote() {
            bail!(
                "requested box lane fell back to local: {}",
                lane.placement_reason
            );
        }
        drop(review_start_lock);
        let review = bounded_poll(
            deadline,
            "automatic review must not wait for later box lane",
            OBSERVE,
            || {
                let records = crate::review::list(&project)?;
                let ready = records
                    .iter()
                    .find(|r| r.members.iter().any(|m| m.thread == mac) && r.reviewer.is_some());
                let (box_seal, state) = seal(&project, &box_lane)?;
                let independent = independent_review(ready, &box_lane, box_seal.is_some())?;
                Ok((
                    ready.filter(|_| independent).cloned(),
                    format!("{} reviews; {state}", records.len()),
                ))
            },
        )?;
        Ok(format!(
            "{box_lane} on {label}; {} reviewer {} for older seal {mac}, excludes later lane",
            review.id,
            review.reviewer.unwrap_or_default()
        ))
    })?;
    report.step("box seals, reviewer merges, landing pushes", || {
        let at = bounded_poll(deadline, "box seal", OBSERVE, || seal(&project, &box_lane))?;
        let box_sha = sealed_sha(&project, &box_lane)?;
        git(
            ctx,
            scratch,
            &[
                "--git-dir",
                bare.to_str().context("bare path")?,
                "cat-file",
                "-e",
                &format!("{box_sha}:box.txt"),
            ],
        )?;
        bounded_poll(deadline, "both landings", Duration::from_secs(240), || {
            let reviews = crate::review::list(&project)?;
            let landed = [&mac, &box_lane].iter().all(|id| {
                reviews.iter().any(|r| {
                    r.members.iter().any(|m| &m.thread == *id)
                        && r.fast_forward
                        && r.push
                        && r.phase == crate::review::Phase::Complete
                })
            });
            Ok((landed.then_some(()), serde_json::to_string(&reviews)?))
        })?;
        let local = git(ctx, &repo, &["rev-parse", "main"])?;
        let pushed = git(
            ctx,
            scratch,
            &[
                "--git-dir",
                bare.to_str().context("bare path")?,
                "rev-parse",
                "refs/heads/main",
            ],
        )?;
        if local != pushed {
            bail!("landing {local} != local bare {pushed}");
        }
        for file in ["mac.txt", "box.txt"] {
            git(ctx, &repo, &["cat-file", "-e", &format!("main:{file}")])?;
        }
        Ok(format!(
            "{box_lane} sealed at {at}; both reviewed, merged, pushed; local bare main={pushed}"
        ))
    })?;
    report.step("Claude never-trusted folder", || {
        prepare_repo(ctx, &trust_repo)?;
        prove_untrusted(ctx, &trust_repo)?;
        let (mut settings, _) = project.read_project_md()?;
        settings.repos.push(crate::project::Repo {
            path: trust_repo.to_string_lossy().into_owned(),
            branch: Some("main".into()),
            gates: Some(Vec::new()),
            ..Default::default()
        });
        project::write_atomic(
            &project.project_md(),
            format!("+++\n{}+++\n", toml::to_string(&settings)?).as_bytes(),
        )?;
        let task = scratch.join("claude.md");
        brief(&task, "claude.txt", false)?;
        let id = start_lane(ctx, &slug, &trust_repo, &claude, "local", &task, &request)?;
        bounded_poll(
            deadline,
            "Claude trust answer and seal",
            Duration::from_secs(150),
            || {
                let lane = crate::thread::load(&project, &id)?;
                let (done, state) = seal(&project, &id)?;
                Ok((
                    (lane.trust_answered && done.is_some()).then_some(()),
                    format!("trust_answered={}; {state}", lane.trust_answered),
                ))
            },
        )?;
        let sha = sealed_sha(&project, &id)?;
        git(ctx, &trust_repo, &["cat-file", "-e", &format!("{sha}:claude.txt")])?;
        Ok(format!(
            "{id}: fresh repo {}; managed trust dialog answered; Claude ran and sealed commit {sha}",
            trust_repo.display()
        ))
    })?;
    report.step("identity-qualified delete", || {
        owned_project(ctx, &slug)?;
        ha(ctx, &["delete", &slug])?;
        if Project::load(&ctx.root, &slug).is_ok() {
            bail!("project still exists after delete");
        }
        // The generated box clone is not project-owned in lifecycle's plan.
        // It is removed only on success, by this run's exact reserved path.
        let out = crate::remote::ssh(
            ctx.runner,
            &profile.target,
            &format!(
                "set -e; test \"$(stat -c '%d:%i' {dir})\" = {identity}; rm -rf -- {dir}",
                dir = crate::remote::quote(&box_scratch),
                identity = crate::remote::quote(&box_identity)
            ),
            None,
            COMMAND,
        )?;
        if !out.success() {
            bail!("box scratch cleanup: {}", out.error_text());
        }
        stop_session(
            ctx,
            resources
                .session
                .as_ref()
                .context("owned session receipt missing")?,
        )?;
        if let Some(server) = resources.server.as_mut() {
            bounded_poll(deadline, "owned server exit", COMMAND, || {
                Ok((
                    server.try_wait()?.map(|_| ()),
                    "owned server still alive".into(),
                ))
            })?;
        }
        command(
            ctx,
            Cmd::new(ctx.env.herdr_bin(), COMMAND).args(["session", "delete", &session]),
        )?;
        Ok(format!(
            "ha delete {slug}; project absent, run-owned box clone and isolated session removed"
        ))
    })?;
    drop(tunnel);
    drop(daemon);
    Ok(())
}

/// The same entry point is used by hand and by the installed image's detached worker.
pub(crate) fn run(ctx: &Ctx, review: Option<(&str, &str)>) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("the real Mac + box journey must run on the Mac; oci cannot drive the Mac's herdr");
    }
    let delivery_ctx = ctx;
    let run_deadline = Instant::now() + WHOLE_RUN;
    let bounded = BoundedRunner {
        inner: ctx.runner,
        deadline: run_deadline - SHUTDOWN,
    };
    let bounded_ctx = Ctx {
        runner: &bounded,
        root: ctx.root.clone(),
        config_dir: ctx.config_dir.clone(),
        env: ctx.env,
        detached_ticker: ctx.detached_ticker,
    };
    let ctx = &bounded_ctx;
    let stamp = jiff::Timestamp::now()
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string()
        .to_lowercase();
    let slug = format!("journey-{stamp}-{}", std::process::id());
    let reports = ctx.root.join(".journeys");
    std::fs::create_dir_all(&reports)?;
    // Do not inherit a previously trusted home/project ancestor.
    let scratch = std::env::temp_dir().join(&slug);
    std::fs::create_dir(&scratch)?;
    let mut report = Report {
        project: slug,
        ..Default::default()
    };
    let request = match ctx.env.var("HERDR_ADE_JOURNEY_REQUEST") {
        Some(path) => {
            project::read_json::<InstallRequest>(Path::new(path)).context("install request missing")
        }
        None => installed_ticker(&ctx.root),
    };
    let observation_result = request
        .and_then(|request| {
            if !crate::build::same_commit(&request.build, crate::VERSION) { bail!("journey worker image {} does not match installed ticker {}", crate::VERSION, request.build); }
            poll("installed ticker's first full passes", OBSERVE, || {
                let record = project::read_json::<Passes>(&pass_path(&ctx.root, request.pid))
                    .unwrap_or_default();
                Ok((
                    (record.pid == request.pid && record.started == request.started)
                        .then(|| observation_result(&record, &request.build))
                        .flatten(),
                    format!(
                        "build={}, pid={}, complete passes={}, first failures={:?}; second pass unknown until completed",
                        record.build,
                        record.pid,
                        record.passes.len(), record.passes.first()
                    ),
                ))
            })
        })
        .unwrap_or_else(|e| format!("ticker first full pass: FAIL: {e:#}"));
    let observation = if let Some((slug, id)) = review {
        format!("REVIEW {slug}/{id}: {observation_result}")
    } else {
        format!("INSTALL: {observation_result}")
    };
    let journey_started = Instant::now();
    let mut resources = RunResources::default();
    if let Err(error) = run_steps(ctx, &scratch, &mut report, bounded.deadline, &mut resources)
        && !report.steps.iter().any(|s| !s.passed)
    {
        report.steps.push(Step {
            name: "setup".into(),
            seconds: journey_started.elapsed().as_secs_f64(),
            passed: false,
            evidence: format!("{error:#}"),
        });
    }
    report.deleted = report.created && !ctx.root.join(&report.project).exists();
    if report.steps.iter().any(|s| !s.passed) {
        let retained = Report {
            project: report.project.clone(),
            created: report.created,
            deleted: report.deleted,
            ..Default::default()
        };
        // Report teardown even if it fails; never claim pause killed processes.
        let _ = report.step("failure shutdown", || {
            shutdown_failed_run(
                delivery_ctx,
                &retained,
                &mut resources,
                run_deadline.min(Instant::now() + SHUTDOWN),
            )
        });
    }
    report.observation = observation.clone();
    project::write_json(&reports.join(format!("{}.json", report.project)), &report)?;
    project::write_json(&scratch.join("report.json"), &report)?;
    let mut notice = format!(
        "{observation}; JOURNEY {}: {}; scratch {}",
        report.project,
        report.notice(),
        scratch.display()
    );
    if let Some((slug, id)) = review
        && let Err(error) = crate::review::post_install_result(delivery_ctx, slug, id, &notice)
    {
        notice.push_str(&format!("; REVIEW record update failed: {error:#}"));
    }
    // One durable notice. The normal inbox delivery path primes/wakes adeherdr.
    let coordinator = Project::load(&ctx.root, "adeherdr")?;
    crate::inbox::write(&coordinator, "journey", &report.project, &notice, &notice)?;
    println!("{notice}");
    if report.steps.iter().all(|s| s.passed) {
        std::fs::remove_dir_all(scratch)?;
    }
    Ok(())
}

pub(crate) fn launch_failed(
    ctx: &Ctx,
    current: Option<(&str, &str)>,
    error: &anyhow::Error,
) -> Result<()> {
    let origin = current
        .map(|(slug, id)| format!("REVIEW {slug}/{id}"))
        .unwrap_or_else(|| "INSTALL".into());
    let line =
        format!("{origin}: post-install FAIL: {error:#}; journey not started, no project created");
    let coordinator = Project::load(&ctx.root, "adeherdr")?;
    crate::inbox::write(&coordinator, "journey", "startup", &line, &line)?;
    Ok(())
}

/// Never waits for the journey, and never changes install success/rollback facts.
pub(crate) fn after_install(ctx: &Ctx, current: Option<(&str, &str)>) -> Result<()> {
    let dir = ctx.root.join(".journeys");
    std::fs::create_dir_all(&dir)?;
    let key = current
        .map(|(slug, id)| format!("{slug}-{id}"))
        .unwrap_or_else(|| format!("manual-{}", jiff::Timestamp::now().as_millisecond()));
    let request_path = dir.join(format!("install-{key}.json"));
    if request_path.exists() {
        return Ok(());
    }
    let request = installed_ticker(&ctx.root)?;
    project::write_create_only(&request_path, serde_json::to_string(&request)?.as_bytes())?;
    let log = std::fs::File::create(dir.join(format!("install-{key}.log")))?;
    let bin = ctx.env.home.join(".local/bin/herdr-ade");
    let mut cmd = Command::new(bin);
    cmd.args(["--root", &ctx.root.to_string_lossy(), "harness", "journey"]);
    cmd.env("HERDR_ADE_JOURNEY_REQUEST", &request_path);
    if let Some((slug, id)) = current {
        cmd.args(["--review", &format!("{slug}/{id}")]);
    }
    for key in [
        "HERDR_ADE_LAUNCH",
        "HERDR_PANE_ID",
        "HERDR_TAB_ID",
        "HERDR_WORKSPACE_ID",
    ] {
        cmd.env_remove(key);
    }
    // Unlike the transport children, the worker must outlive the installing ticker.
    use std::os::unix::process::CommandExt;
    cmd.process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    if let Err(error) = cmd.spawn() {
        let _ = std::fs::remove_file(request_path);
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "journey/tests.rs"]
mod boundary_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, fail, ok};

    #[test]
    fn first_failure_cannot_be_hidden_by_a_clean_second_pass_or_an_old_image() {
        let mut record = Passes {
            build: crate::VERSION.into(),
            passes: vec![vec!["closed socket: EINVAL".into()]],
            ..Default::default()
        };
        assert!(observation_result(&record, crate::VERSION).is_none());
        record.passes.push(Vec::new());
        let text = observation_result(&record, crate::VERSION).unwrap();
        assert!(text.contains("FAIL") && text.contains("EINVAL") && text.contains("transient"));
        assert!(observation_result(&record, "0.0.0+other").is_none());
        record.passes[1].push("box unreachable".into());
        let persistent = observation_result(&record, crate::VERSION).unwrap();
        assert!(persistent.contains("second full pass still missing: box unreachable"));
        record.passes[0].clear();
        assert!(
            observation_result(&record, crate::VERSION)
                .unwrap()
                .contains("zero missing")
        );
    }

    #[test]
    fn only_first_two_completed_passes_are_retained() {
        let dir = tempfile::tempdir().unwrap();
        ticker_started(dir.path(), "this-run").unwrap();
        ticker_pass(dir.path(), &["missing socket".into()]).unwrap();
        ticker_pass(dir.path(), &[]).unwrap();
        ticker_pass(dir.path(), &["later unrelated failure".into()]).unwrap();
        let record: Passes =
            project::read_json(&pass_path(dir.path(), std::process::id())).unwrap();
        assert_eq!(record.passes.len(), 2);
        assert_eq!(record.passes[0], ["missing socket"]);
    }

    #[test]
    fn failure_stops_sequence_and_preserves_evidence_and_delete_command() {
        let world = crate::scenarios::World::new();
        let runner = FakeRunner::new();
        runner.on("new", ok("created"));
        runner.on("open", fail(1, "real herdr rejects timeout"));
        let ctx = Ctx {
            runner: &runner,
            ..world.ctx()
        };
        let mut report = Report {
            project: "journey-123".into(),
            created: true,
            ..Default::default()
        };
        let execute = (|| -> Result<()> {
            report.step("new", || ha(&ctx, &["new", "journey-123"]))?;
            report.step("open", || ha(&ctx, &["open", "journey-123"]))?;
            report.step("delete", || ha(&ctx, &["delete", "journey-123"]))
        })();
        assert!(execute.is_err());
        assert_eq!(runner.count("delete"), 0);
        let text = report.notice();
        assert!(
            text.contains("PASS new")
                && text.contains("FAIL open")
                && text.contains("rejects timeout")
                && text.contains("ha delete journey-123")
        );
        for cmd in runner.calls.borrow().iter() {
            assert!(cmd.own_group);
            assert_eq!(cmd.timeout, COMMAND);
        }
    }

    #[test]
    fn lane_start_uses_existing_cli_with_explicit_recipe_machine_and_task_authority() {
        let world = crate::scenarios::World::new();
        let runner = FakeRunner::new();
        runner.on("thread start", ok(r#"{"data":{"id":"t-123"}}"#));
        let ctx = Ctx {
            runner: &runner,
            ..world.ctx()
        };
        let id = start_lane(
            &ctx,
            "journey-123",
            Path::new("/scratch/repo"),
            "small",
            "box",
            Path::new("/scratch/task.md"),
            "request:q-123",
        )
        .unwrap();
        assert_eq!(id, "t-123");
        let command = runner.calls.borrow()[0].clone();
        let args = command.args.join(" ");
        assert!(args.contains("--machine box --recipe small --task-file /scratch/task.md"));
        assert!(args.contains("--request request:q-123 --acceptance"));
        assert!(command.env_remove.contains(&"HERDR_ADE_LAUNCH".into()));
    }

    #[test]
    fn a_ready_or_resolved_lane_is_not_a_seal() {
        let fx = crate::testkit::fixture();
        let (id, sha) = fx.lane(1);
        assert!(seal(&fx.project, &id).unwrap().0.is_none());
        crate::thread::update(&fx.project, &id, |lane| {
            lane.status = crate::thread::Status::Resolved
        })
        .unwrap();
        assert!(seal(&fx.project, &id).unwrap().0.is_none());
        fx.seal_done(&id, 1, 1, &sha, "journey report");
        assert!(seal(&fx.project, &id).unwrap().0.is_some());
    }

    #[test]
    fn cheap_selection_ignores_disabled_recipes_and_uses_small_configured_tiers() {
        let world = crate::scenarios::World::new();
        let mut catalog = crate::launch::recipe_catalog(&world.ctx().config_dir).unwrap();
        catalog.recipes.clear();
        for (id, model, enabled) in [
            ("expensive", "sol-high", true),
            ("small", "model-mini", true),
            ("disabled", "model-flash", false),
        ] {
            catalog.recipes.insert(
                id.into(),
                crate::contracts::Recipe {
                    kind: "pi".into(),
                    args: vec!["--model".into(), model.into()],
                    enabled,
                    ..Default::default()
                },
            );
        }
        assert_eq!(cheap_recipe(&catalog, false).unwrap(), "small");
        catalog.recipes.insert(
            "haiku".into(),
            crate::contracts::Recipe {
                kind: "claude".into(),
                args: vec!["--model".into(), "claude-haiku-4-5-20251001".into()],
                ..Default::default()
            },
        );
        assert_eq!(cheap_recipe(&catalog, true).unwrap(), "haiku");
    }

    #[test]
    fn no_deletion_authority_without_run_marker_and_matching_directory_identity() {
        let world = crate::scenarios::World::new();
        let project = project::create(&world.root, "journey-123", "", Vec::new()).unwrap();
        assert!(owned_project(&world.ctx(), &project.slug).is_err());
        let (device, inode) = project_identity(&project).unwrap();
        let marker = Owned {
            slug: project.slug.clone(),
            reviewer_recipe: "small".into(),
            device,
            inode,
        };
        project::write_json(&project.record_file("journey.json"), &marker).unwrap();
        assert!(owned_project(&world.ctx(), &project.slug).is_ok());
        assert_eq!(reviewer_recipe(&project).as_deref(), Some("small"));
        project::write_json(
            &project.record_file("journey.json"),
            &Owned {
                inode: inode + 1,
                ..marker
            },
        )
        .unwrap();
        assert!(owned_project(&world.ctx(), &project.slug).is_err());
        assert!(reviewer_recipe(&project).is_none());
        let report = Report {
            project: project.slug.clone(),
            steps: vec![Step {
                name: "new".into(),
                seconds: 0.0,
                passed: false,
                evidence: "collision".into(),
            }],
            ..Default::default()
        };
        assert!(!report.notice().contains("ha delete"));
    }

    #[test]
    fn n7_requires_a_fresh_folder_without_trusted_ancestors_or_unknown_config() {
        let world = crate::scenarios::World::new();
        let folder = world.home.path().join("scratch");
        std::fs::create_dir(&folder).unwrap();
        prove_untrusted(&world.ctx(), &folder).unwrap();
        let config = world.home.path().join(".claude.json");
        std::fs::write(&config, serde_json::json!({ "projects": { world.home.path().to_str().unwrap(): { "hasTrustDialogAccepted": true } } }).to_string()).unwrap();
        assert!(
            prove_untrusted(&world.ctx(), &folder)
                .unwrap_err()
                .to_string()
                .contains("trusted ancestor")
        );
        std::fs::write(config, "broken").unwrap();
        assert!(prove_untrusted(&world.ctx(), &folder).is_err());
    }

    #[test]
    fn budget_clamps_real_commands_and_does_not_run_after_deadline() {
        let runner = FakeRunner::new();
        runner.on("git", ok("ok"));
        let bounded = BoundedRunner {
            inner: &runner,
            deadline: Instant::now() + Duration::from_secs(2),
        };
        bounded.run(&Cmd::new("git", COMMAND)).unwrap();
        let command = runner.calls.borrow()[0].clone();
        assert!(command.timeout <= Duration::from_secs(2) && command.own_group);
        let exhausted = BoundedRunner {
            inner: &runner,
            deadline: Instant::now(),
        };
        assert!(exhausted.run(&Cmd::new("git", COMMAND)).is_err());
        assert_eq!(runner.count("git"), 1);
        assert!(
            bounded_poll::<()>(Instant::now(), "seal", OBSERVE, || panic!(
                "must not observe after whole-run deadline"
            ))
            .is_err()
        );
    }

    #[test]
    fn a_deadline_is_failure_not_inferred_success() {
        let error = poll::<()>("seal", Duration::ZERO, || {
            Ok((None, "no sealed event".into()))
        })
        .unwrap_err();
        assert!(error.to_string().contains("no sealed event"));
    }
}
