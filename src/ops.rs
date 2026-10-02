//! Recoverable `done` and `waiting` operations (SPEC-ADE D5, item 32).

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::contracts::{
    DonePayload, Event, EventPayload, Op, OpKind, OpState, Recipient, Requested, WaitingPayload,
};
use crate::events;
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::runner::{Cmd, Runner};

fn ops_dir(project: &Project) -> PathBuf {
    project.record_dir("ops")
}

fn op_path(project: &Project, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(ops_dir(project).join(format!("{id}.toml")))
}

fn validate_id(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && !id.starts_with('.')
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if !valid {
        bail!("`{id}` is not an operation id");
    }
    Ok(())
}

pub(crate) fn load(project: &Project, id: &str) -> Result<Op> {
    let path = op_path(project, id)?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read operation {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("{} does not parse", path.display()))
}

pub(crate) fn list(project: &Project) -> Vec<Op> {
    let Ok(entries) = std::fs::read_dir(ops_dir(project)) else {
        return Vec::new();
    };
    let mut ops: Vec<Op> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".toml").map(str::to_owned))
        .filter_map(|id| load(project, &id).ok())
        .collect();
    ops.sort_by(|a, b| a.op.cmp(&b.op));
    ops
}

fn write_op(project: &Project, op: &Op) -> Result<()> {
    project.record_dir_for_write("ops")?;
    project::write_atomic(&op_path(project, &op.op)?, toml::to_string(op)?.as_bytes())
}

/// The complete payload of one `ha done` or `ha waiting`.
pub(crate) struct Reservation<'a> {
    pub(crate) thread: &'a str,
    pub(crate) attempt: u32,
    pub(crate) kind: OpKind,
    pub(crate) recipient: Recipient,
    pub(crate) requested: Requested,
    pub(crate) helper_pid: u32,
}

/// A done's report bytes and the review's correction barriers are part of its
/// dedup key. Read the report before locking; stage_done still validates the
/// report, git state and sha before any new event is sealed.
pub(crate) fn reserve_done(project: &Project, r: Reservation<'_>, worktree: &Path) -> Result<Op> {
    let Requested::Done { report_path, .. } = &r.requested else {
        bail!("op_payload_invalid: reserve_done needs a done payload");
    };
    let report = stable_read(&resolve_report(worktree, report_path)?)?;
    let hash = format!("{:x}", Sha256::digest(&report));
    reserve_inner(project, r, Some(&hash))
}

/// Reserve the complete payload under the project lock. Same-payload retries
/// resume one op. A changed payload abandons it and allocates the next id.
pub(crate) fn reserve(project: &Project, r: Reservation<'_>) -> Result<Op> {
    reserve_inner(project, r, None)
}

fn reserve_inner(project: &Project, r: Reservation<'_>, report_hash: Option<&str>) -> Result<Op> {
    let Reservation {
        thread,
        attempt,
        kind,
        recipient,
        requested,
        helper_pid,
    } = r;
    let _lock = project.lock()?;
    let mut existing: Vec<Op> = list(project)
        .into_iter()
        .filter(|op| op.thread == thread && op.attempt == attempt)
        .collect();
    existing.sort_by(|a, b| a.op.cmp(&b.op));
    // The lane record is authoritative for a requested correction. The
    // barrier can still be present after a fresh done, before review;
    // in that case retrying that fresh op must remain idempotent.
    let barriers = if report_hash.is_some() {
        let mut barriers = correction_barriers(project, thread)?;
        let box_path = project
            .record_dir("corrections")
            .join(format!("{thread}.toml"));
        if box_path.exists() {
            let box_record: BoxCorrections = toml::from_str(&std::fs::read_to_string(box_path)?)?;
            if box_record.thread != thread {
                bail!("correction_barrier_invalid: wrong thread");
            }
            if box_record.attempt == attempt {
                barriers.extend(box_record.events);
            }
        }
        barriers
    } else {
        Vec::new()
    };
    let matches = |op: &&Op| {
        op.state != OpState::Abandoned
            && op.kind == kind
            && op.requested == requested
            && op.recipient == recipient
            && report_hash.is_none_or(|hash| {
                if op.state == OpState::Reserved {
                    op.report_hash.as_deref() == Some(hash)
                } else {
                    op.artifact.as_deref() == Some(hash)
                }
            })
            && barriers.iter().all(|barrier| op_after_barrier(op, barrier))
    };
    // Only the latest done may be retried: reverting report bytes to an
    // earlier version is still a new submission. Other verbs keep their
    // existing same-payload retry semantics.
    let reusable = if report_hash.is_some() {
        existing
            .iter()
            .rev()
            .find(|op| op.state != OpState::Abandoned)
            .filter(matches)
    } else {
        existing.iter().rev().find(matches)
    };
    if let Some(op) = reusable {
        return Ok(op.clone());
    }
    for mut op in existing
        .iter()
        .filter(|op| op.state != OpState::Sealed && op.state != OpState::Abandoned)
        .cloned()
    {
        op.state = OpState::Abandoned;
        op.revision += 1;
        write_op(project, &op)?;
    }
    let n = existing
        .iter()
        .filter_map(|op| op.op.rsplit('-').next()?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let id = format!("{thread}-{attempt}-{n}");
    let op = Op {
        op: id.clone(),
        revision: 1,
        thread: thread.to_string(),
        attempt,
        kind,
        recipient,
        helper_pid,
        requested,
        event: id,
        state: OpState::Reserved,
        created: project::now(),
        artifact: None,
        report_hash: report_hash.map(str::to_owned),
        has_changes: None,
        published_ref: None,
    };
    write_op(project, &op)?;
    Ok(op)
}

/// The Mac pushes this narrow barrier before sending a correction to a box
/// pane. Round records themselves are Mac-only; the box never infers a missing
/// record means no correction was requested.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct BoxCorrections {
    pub(crate) thread: String,
    pub(crate) attempt: u32,
    pub(crate) events: Vec<String>,
}

pub(crate) fn correction_barriers(project: &Project, thread: &str) -> Result<Vec<String>> {
    // Box operations have a lane card, not a Mac thread record. Their
    // follow-up barrier is supplied by BoxCorrections below.
    if !crate::thread::threads_dir(project)
        .join(format!("{thread}.toml"))
        .exists()
    {
        return Ok(Vec::new());
    }
    let lane = crate::thread::load(project, thread)?;
    Ok((!lane.review_after.is_empty())
        .then_some(lane.review_after)
        .into_iter()
        .collect())
}

fn op_after_barrier(op: &Op, barrier: &str) -> bool {
    let prefix = format!("{}-{}-", op.thread, op.attempt);
    let Some(before) = barrier.strip_prefix(&prefix) else {
        // A barrier from an earlier attempt cannot block this attempt.
        return true;
    };
    match (
        before.parse::<u32>(),
        op.op
            .strip_prefix(&prefix)
            .and_then(|n| n.parse::<u32>().ok()),
    ) {
        (Ok(before), Some(now)) => now > before,
        _ => false,
    }
}

/// Verify the URL-matched remote after publishing a box lane's own ref.
pub(crate) fn check_published_ref(
    runner: &dyn Runner,
    worktree: &Path,
    branch: &str,
    publish_url: &str,
    sha: &str,
) -> Result<()> {
    let out = runner.run(
        &Cmd::new("git", std::time::Duration::from_secs(30))
            .args(["-C", &worktree.to_string_lossy()])
            .args(["ls-remote", publish_url, &format!("refs/heads/{branch}")]),
    )?;
    if !out.success() {
        return Err(crate::refusal::error(
            format!(
                "published_ref_check_failed: {}; retry `ha done`. If it keeps failing, ask the coordinator to check the remote",
                out.error_text()
            ),
            "ha done --report <report-path> --sha <HEAD-sha>",
        ));
    }
    let found = out.stdout.split_whitespace().next().unwrap_or("");
    if found != sha {
        return Err(crate::refusal::error(
            format!(
                "published_ref_mismatch: `{branch}` is {} on {publish_url}, not {sha}; retry `ha done`. If it still differs, ask the coordinator to check the remote",
                if found.is_empty() { "missing" } else { found }
            ),
            "ha done --report <report-path> --sha <HEAD-sha>",
        ));
    }
    Ok(())
}

/// Stage a Mac `done` without publishing. Git is invoked only here, then the
/// revision-1 marker is advanced under the lock.
pub(crate) fn stage_done(
    project: &Project,
    id: &str,
    worktree: &Path,
    runner: &dyn Runner,
) -> Result<Op> {
    stage_done_inner(project, id, worktree, runner, None)
}

/// Box `done`: publish only the branch and URL from the validated lane card.
/// Do this before staging, so recovery cannot seal an unpublished operation.
pub(crate) fn stage_box_done(
    project: &Project,
    id: &str,
    worktree: &Path,
    runner: &dyn Runner,
    card: &crate::contracts::LaneCard,
) -> Result<Op> {
    stage_done_inner(project, id, worktree, runner, Some(card))
}

fn stage_done_inner(
    project: &Project,
    id: &str,
    worktree: &Path,
    runner: &dyn Runner,
    card: Option<&crate::contracts::LaneCard>,
) -> Result<Op> {
    let op = load(project, id)?;
    if op.state == OpState::Staged || op.state == OpState::Sealed {
        return Ok(op);
    }
    if op.state != OpState::Reserved || op.revision != 1 || op.kind != OpKind::Done {
        bail!("op_state_changed: {id} is not reserved at revision 1");
    }
    let Requested::Done { sha, report_path } = &op.requested else {
        bail!("op_payload_invalid: {id} is not a done payload");
    };
    let report = resolve_report(worktree, report_path)?;
    let first = stable_read(&report)?;
    if op
        .report_hash
        .as_deref()
        .is_some_and(|hash| hash != format!("{:x}", Sha256::digest(&first)))
    {
        bail!("report_unstable: report changed after reservation");
    }
    let status = runner.run(
        &Cmd::new("git", std::time::Duration::from_secs(20))
            .args(["status", "--short"])
            .cwd(worktree),
    )?;
    if !status.success() {
        bail!("git_status_failed: {}", status.error_text());
    }
    if !status.stdout.trim().is_empty() {
        return Err(crate::refusal::error(
            "worktree_dirty: ha done requires an empty git status; commit or remove the listed changes first",
            format!("ha done --report {} --sha {sha}", report.display()),
        ));
    }
    let head = runner.run(
        &Cmd::new("git", std::time::Duration::from_secs(20))
            .args(["rev-parse", "HEAD"])
            .cwd(worktree),
    )?;
    if !head.success() {
        bail!("git_head_failed: {}", head.error_text());
    }
    if head.stdout.trim() != sha {
        return Err(crate::refusal::error(
            format!(
                "sha_mismatch: requested {sha}, HEAD is {}",
                head.stdout.trim()
            ),
            format!(
                "ha done --report {} --sha {}",
                report.display(),
                head.stdout.trim()
            ),
        ));
    }
    let tracked = runner.run(
        &Cmd::new("git", std::time::Duration::from_secs(20))
            .args([
                "ls-tree",
                "-r",
                "--name-only",
                "-z",
                sha,
                "--",
                ".herdr-project/",
            ])
            .cwd(worktree),
    )?;
    if !tracked.success() {
        bail!("git_tracked_paths_failed: {}", tracked.error_text());
    }
    let tracked: Vec<_> = tracked
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();
    if !tracked.is_empty() {
        let paths = tracked
            .iter()
            .map(|path| format!("- `{path}`"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(crate::refusal::error(
            format!(
                "worktree_dirty: commit {sha} tracks paths under .herdr-project/:\n{paths}\nuntrack these paths, commit, then run `ha done` again"
            ),
            "ha thread show <project> <thread> (commit or remove the listed changes, then retry)",
        ));
    }
    let second = stable_read(&report)?;
    if first != second {
        bail!("report_unstable: report bytes changed while staging");
    }
    if let Some(card) = card {
        if card.thread != op.thread || card.attempt != op.attempt || card.recipient != op.recipient
        {
            bail!("bootstrap_mismatch: done operation does not match the lane card");
        }
        publish_lane_ref(
            runner,
            worktree,
            &seal_ref(&card.branch, sha),
            &card.publish_url,
            sha,
        )?;
    }
    let base = match card {
        Some(card) => card.brief_commit.clone(),
        None => crate::thread::load(project, &op.thread)?.base,
    };
    if base.is_empty() {
        bail!("lane base is missing; cannot classify changes");
    }
    let git = crate::repo::Git::new(runner, worktree);
    let has_changes = git.trees_differ(&base, sha)?;
    let artifact = write_artifact(project, &first)?;
    advance_staged(
        project,
        id,
        Some(artifact),
        Some(has_changes),
        card.map(|card| seal_ref(&card.branch, sha)),
    )
}

/// Separate namespace avoids a file/directory conflict with the lane branch.
/// A full commit id makes retries idempotent without ever updating a ref.
pub(crate) fn seal_ref(branch: &str, sha: &str) -> String {
    format!("seals/{branch}/{sha}")
}

fn publish_lane_ref(
    runner: &dyn Runner,
    worktree: &Path,
    branch: &str,
    publish_url: &str,
    sha: &str,
) -> Result<()> {
    let out = runner.run(
        &Cmd::new("git", std::time::Duration::from_secs(60))
            .args(["-C", &worktree.to_string_lossy()])
            .args(["push", publish_url, &format!("{sha}:refs/heads/{branch}")]),
    ).map_err(|error| crate::refusal::error(format!(
        "lane_publish_failed: {error}; retry `ha done`. If it keeps failing, ask the coordinator to check the remote"
    ), "ha done --report <report-path> --sha <HEAD-sha>"))?;
    if !out.success() {
        return Err(crate::refusal::error(
            format!(
                "lane_publish_failed: {}; retry `ha done`. If it keeps failing, ask the coordinator to check the remote",
                out.error_text()
            ),
            "ha done --report <report-path> --sha <HEAD-sha>",
        ));
    }
    check_published_ref(runner, worktree, branch, publish_url, sha)
}

pub(crate) fn stage_waiting(project: &Project, id: &str) -> Result<Op> {
    let op = load(project, id)?;
    if op.state == OpState::Staged || op.state == OpState::Sealed {
        return Ok(op);
    }
    if op.state != OpState::Reserved
        || op.revision != 1
        || !matches!(op.kind, OpKind::Waiting | OpKind::Failed)
    {
        bail!("op_state_changed: {id} is not a reserved waiting/failure operation");
    }
    advance_staged(project, id, None, None, None)
}

fn advance_staged(
    project: &Project,
    id: &str,
    artifact: Option<String>,
    has_changes: Option<bool>,
    published_ref: Option<String>,
) -> Result<Op> {
    let _lock = project.lock()?;
    let mut current = load(project, id)?;
    if current.state == OpState::Staged || current.state == OpState::Sealed {
        if current.artifact == artifact || current.state == OpState::Sealed {
            return Ok(current);
        }
        bail!("op_state_changed: staged artifact differs for {id}");
    }
    if current.state != OpState::Reserved || current.revision != 1 {
        bail!("op_state_changed: {id} advanced while staging");
    }
    current.artifact = artifact;
    current.has_changes = has_changes;
    current.published_ref = published_ref;
    current.state = OpState::Staged;
    current.revision = 2;
    write_op(project, &current)?;
    Ok(current)
}

/// Seal under the project lock after the caller re-verifies the attempt and
/// recipient binding. A matching X2b event repairs the marker.
pub(crate) fn seal(
    project: &Project,
    id: &str,
    validate: impl FnOnce(&Op) -> Result<()>,
) -> Result<Event> {
    let _lock = project.lock()?;
    let mut op = load(project, id)?;
    if op.state == OpState::Sealed {
        return events::load(project, &op.event);
    }
    if op.state != OpState::Staged || op.revision != 2 {
        bail!("op_state_changed: {id} is not staged at revision 2");
    }
    validate(&op)?;
    let event = event_from_op(&op)?;
    events::seal_create_if_absent(project, &event)?;
    // The receipt records the bytes this sealer hashed (D5); the courier
    // carries it to the Mac event records.
    events::write_receipt(project, &event)?;
    op.state = OpState::Sealed;
    op.revision = 3;
    write_op(project, &op)?;
    Ok(event)
}

fn event_from_op(op: &Op) -> Result<Event> {
    let payload = match (&op.requested, op.kind) {
        (Requested::Done { sha, report_path }, OpKind::Done) => EventPayload {
            done: Some(DonePayload {
                has_changes: op.has_changes,
                sha: sha.clone(),
                report_path: report_path.clone(),
                artifact: op
                    .artifact
                    .clone()
                    .context("op_payload_invalid: staged done has no artifact")?,
                attestation: None,
                published_ref: op.published_ref.clone(),
            }),
            waiting: None,
            failed: None,
        },
        (
            Requested::Waiting {
                text,
                class,
                provider_kind,
            },
            OpKind::Waiting,
        ) => EventPayload {
            done: None,
            waiting: Some(WaitingPayload {
                text: text.clone(),
                class: *class,
                provider_kind: provider_kind.clone(),
            }),
            failed: None,
        },
        (
            Requested::Failed {
                failure,
                class,
                provider_kind,
            },
            OpKind::Failed,
        ) => EventPayload {
            failed: Some(WaitingPayload {
                text: failure.clone(),
                class: *class,
                provider_kind: provider_kind.clone(),
            }),
            ..Default::default()
        },
        _ => bail!("op_payload_invalid: kind and requested payload disagree"),
    };
    Ok(Event {
        id: op.event.clone(),
        op: op.op.clone(),
        thread: op.thread.clone(),
        attempt: op.attempt,
        recipient: op.recipient.clone(),
        created: op.created.clone(),
        payload,
    })
}

pub(crate) fn abandon(project: &Project, id: &str) -> Result<Op> {
    let _lock = project.lock()?;
    let mut op = load(project, id)?;
    if op.state != OpState::Sealed && op.state != OpState::Abandoned {
        op.state = OpState::Abandoned;
        op.revision += 1;
        write_op(project, &op)?;
    }
    Ok(op)
}

/// A2 ticker pass. A1 wires this from its ticker with the existing `Ctx` and
/// project. Staged operations are sealable from their own durable payload;
/// reserved operations are abandoned only when their exact helper is dead.
pub(crate) fn tick(ctx: &Ctx, project: &Project) -> Result<()> {
    let mut first: Option<anyhow::Error> = None;
    for op in list(project) {
        if let Err(error) = tick_op(ctx, project, &op) {
            first.get_or_insert(error.context(format!("op {}", op.op)));
        }
    }
    let delivered = crate::steps::deliver_events(ctx, project);
    match first {
        Some(error) => Err(error),
        None => delivered,
    }
}

/// X1 and X2 for one operation (D5).
fn tick_op(ctx: &Ctx, project: &Project, op: &Op) -> Result<()> {
    let current = crate::thread::load(project, &op.thread);
    let superseded = current
        .as_ref()
        .is_ok_and(|thread| thread.attempt.max(1) != op.attempt);
    match op.state {
        // X1: the helper is dead, or the lane's attempt was superseded.
        OpState::Reserved if superseded || !pid_alive(ctx.runner, op.helper_pid) => {
            abandon(project, &op.op)?;
        }
        // X2: seal from the op's own payload when the bindings still match.
        OpState::Staged => {
            let coordinator = project.coordinator();
            let valid = current.as_ref().is_ok_and(|thread| {
                thread.attempt.max(1) == op.attempt && thread.pane_id != op.recipient.pane
            }) && coordinator.as_ref().is_some_and(|record| {
                record.pane_id == op.recipient.pane
                    && record.attempt() == op.recipient.coordinator_attempt
            });
            if valid {
                let _ = seal(project, &op.op, |_| Ok(()))?;
            } else {
                abandon(project, &op.op)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Box-side D5 recovery (SPEC-remote §4.3, gate R15). The courier helper runs
/// this once per pass on the box, before it reads the sealed events. The lane
/// card, not a Mac-side thread record, is the box authority.
pub(crate) fn recover_box(ctx: &Ctx) -> Result<()> {
    let mut first: Option<anyhow::Error> = None;
    for slug in project::list_slugs(&ctx.root) {
        let Ok(project) = Project::load(&ctx.root, &slug) else {
            continue;
        };
        for op in list(&project) {
            if let Err(error) = recover_box_op(ctx, &project, &op) {
                first.get_or_insert(error.context(format!("op {}", op.op)));
            }
        }
    }
    match first {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// X1 and X2 for one box operation (D5). A reserved op whose helper is dead or
/// whose card moved on is abandoned; a staged op whose card still matches is
/// sealed from its own durable payload (X2b repairs the marker).
fn recover_box_op(ctx: &Ctx, project: &Project, op: &Op) -> Result<()> {
    match op.state {
        OpState::Reserved => {
            let card = load_box_card(project, &op.thread);
            let superseded = card.as_ref().is_some_and(|card| card.attempt != op.attempt);
            if superseded || !pid_alive(ctx.runner, op.helper_pid) {
                abandon(project, &op.op)?;
            }
        }
        OpState::Staged => {
            let valid = load_box_card(project, &op.thread)
                .is_some_and(|card| card.attempt == op.attempt && card.recipient == op.recipient);
            if valid {
                seal(project, &op.op, |_| Ok(()))?;
            } else {
                abandon(project, &op.op)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The lane card the box wrote at start (SPEC-remote §4.2 step 5).
fn load_box_card(project: &Project, thread: &str) -> Option<crate::contracts::LaneCard> {
    if crate::thread::validate_id(thread).is_err() {
        return None;
    }
    let path = project.record_dir("lanes").join(format!("{thread}.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

fn pid_alive(runner: &dyn Runner, pid: u32) -> bool {
    runner
        .run(
            &Cmd::new("/bin/kill", std::time::Duration::from_secs(2))
                .args(["-0", &pid.to_string()]),
        )
        .is_ok_and(|output| output.success())
}

fn resolve_report(worktree: &Path, requested: &str) -> Result<PathBuf> {
    let requested = Path::new(requested);
    if !requested.is_absolute()
        && requested
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(crate::refusal::error(
            "report_path_invalid: report must stay in the worktree",
            "ha done --report <path-inside-worktree> --sha <HEAD-sha>",
        ));
    }
    let path = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        worktree.join(requested)
    };
    if !path.is_file() {
        return Err(crate::refusal::error(
            format!("report_missing: {}", path.display()),
            format!("ha done --report {} --sha <HEAD-sha>", path.display()),
        ));
    }
    let worktree = std::fs::canonicalize(worktree)
        .with_context(|| format!("could not resolve worktree {}", worktree.display()))?;
    let path = std::fs::canonicalize(&path)
        .with_context(|| format!("could not resolve report {}", path.display()))?;
    if !path.starts_with(&worktree) {
        return Err(crate::refusal::error(
            "report_path_invalid: report must stay in the worktree",
            "ha done --report <path-inside-worktree> --sha <HEAD-sha>",
        ));
    }
    Ok(path)
}

fn stable_read(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let before = file.metadata()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        bail!("report_unstable: {} changed during a read", path.display());
    }
    Ok(bytes)
}

fn write_artifact(project: &Project, bytes: &[u8]) -> Result<String> {
    let hash = format!("{:x}", Sha256::digest(bytes));
    let dir = project.state_dir().join("artifacts");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(&hash);
    if path.exists() {
        if std::fs::read(&path)? == bytes {
            return Ok(hash);
        }
        bail!("artifact_conflict: {} has different bytes", path.display());
    }
    let tmp = dir.join(format!(".{hash}.{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        match std::fs::rename(&tmp, &path) {
            Ok(()) => {}
            Err(error) if path.exists() && std::fs::read(&path)? == bytes => {
                let _ = std::fs::remove_file(&tmp);
                let _ = error;
            }
            Err(error) => return Err(error.into()),
        }
        File::open(&dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RealRunner;
    use crate::runner::fake::{FakeRunner, fail, ok};

    fn fixture() -> (tempfile::TempDir, Project, FakeRunner, Recipient) {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let threads = crate::thread::threads_dir_for_write(&project).unwrap();
        for id in ["t-0001", "t-0088"] {
            let lane = crate::thread::Thread {
                id: id.into(),
                base: "base".into(),
                ..Default::default()
            };
            std::fs::write(
                threads.join(format!("{id}.toml")),
                toml::to_string(&lane).unwrap(),
            )
            .unwrap();
        }
        let runner = FakeRunner::new();
        runner
            .on("rev-parse base^{tree}", ok("base-tree\n"))
            .on("rev-parse abc^{tree}", ok("abc-tree\n"))
            .on("git status --short", ok(""))
            .on("git rev-parse HEAD", ok("abc\n"))
            .on("git ls-tree", ok(""));
        let recipient = Recipient {
            pane: "w1:p1".into(),
            coordinator_attempt: 1,
        };
        (root, project, runner, recipient)
    }

    fn reserved_box_done(project: &Project, worktree: &Path, recipient: &Recipient) -> Op {
        std::fs::write(worktree.join("report.md"), "ready\n").unwrap();
        reserve_done(
            project,
            Reservation {
                thread: "t-0088",
                attempt: 1,
                kind: OpKind::Done,
                recipient: recipient.clone(),
                requested: Requested::Done {
                    sha: "abc".into(),
                    report_path: "report.md".into(),
                },
                helper_pid: 1,
            },
            worktree,
        )
        .unwrap()
    }

    fn box_card(recipient: &Recipient) -> crate::contracts::LaneCard {
        crate::contracts::LaneCard {
            thread: "t-0088".into(),
            attempt: 1,
            branch: "hp/demo/t-0088".into(),
            brief_commit: "base".into(),
            publish_url: "/remotes/publish repo.git".into(),
            recipient: recipient.clone(),
            ..Default::default()
        }
    }

    #[test]
    fn box_done_pushes_only_its_own_ref_then_checks_before_staging() {
        let (root, project, runner, recipient) = fixture();
        let card = box_card(&recipient);
        let op = reserved_box_done(&project, root.path(), &recipient);
        runner
            .on(
                "ls-remote",
                ok("abc\trefs/heads/seals/hp/demo/t-0088/abc\n"),
            )
            .on("git -C", ok(""));
        stage_box_done(&project, &op.op, root.path(), &runner, &card).unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 7);
        assert_eq!(
            calls[3].args,
            [
                "-C",
                root.path().to_str().unwrap(),
                "push",
                "/remotes/publish repo.git",
                "abc:refs/heads/seals/hp/demo/t-0088/abc",
            ]
        );
        assert_eq!(
            calls[4].args,
            [
                "-C",
                root.path().to_str().unwrap(),
                "ls-remote",
                "/remotes/publish repo.git",
                "refs/heads/seals/hp/demo/t-0088/abc",
            ]
        );
        let staged = load(&project, &op.op).unwrap();
        assert_eq!(staged.state, OpState::Staged);
        assert_eq!(
            staged.published_ref.as_deref(),
            Some("seals/hp/demo/t-0088/abc")
        );
        let event = seal(&project, &op.op, |_| Ok(())).unwrap();
        assert_eq!(
            event.payload.done.unwrap().published_ref,
            staged.published_ref
        );
    }

    #[test]
    fn divergent_seals_publish_without_rewriting_a_ref() {
        use std::process::Command;
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let remote = temp.path().join("remote.git");
        std::fs::create_dir(&repo).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q", "--bare"])
                .arg(&remote)
                .status()
                .unwrap()
                .success()
        );
        let git = |args: &[&str]| -> String {
            let out = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "a@b.c"]);
        git(&["config", "user.name", "A"]);
        git(&["commit", "--allow-empty", "-qm", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        git(&["commit", "--allow-empty", "-qm", "first"]);
        let first = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "--detach", &base]);
        git(&["commit", "--allow-empty", "-qm", "second"]);
        let second = git(&["rev-parse", "HEAD"]);
        let runner = crate::runner::RealRunner;
        let url = remote.to_str().unwrap();
        let branch = "review/pile/t-1";
        for sha in [&first, &second] {
            publish_lane_ref(&runner, &repo, &seal_ref(branch, sha), url, sha).unwrap();
        }
        for sha in [&first, &second] {
            check_published_ref(&runner, &repo, &seal_ref(branch, sha), url, sha).unwrap();
        }
    }

    #[test]
    fn rejected_box_push_refuses_with_git_error_and_next_step() {
        let (root, project, runner, recipient) = fixture();
        let op = reserved_box_done(&project, root.path(), &recipient);
        runner.on("git -C", fail(1, "! [rejected] non-fast-forward"));
        let error = stage_box_done(
            &project,
            &op.op,
            root.path(),
            &runner,
            &box_card(&recipient),
        )
        .unwrap_err();
        assert!(crate::refusal::is(&error));
        assert!(error.to_string().contains("non-fast-forward"), "{error}");
        assert!(error.to_string().contains("retry `ha done`"), "{error}");
        assert_eq!(
            runner.calls.borrow().len(),
            4,
            "do not check or stage after rejection"
        );
        assert_eq!(load(&project, &op.op).unwrap().state, OpState::Reserved);
    }

    #[test]
    fn mac_done_never_pushes() {
        let (root, project, runner, recipient) = fixture();
        let op = reserved_box_done(&project, root.path(), &recipient);
        stage_done(&project, &op.op, root.path(), &runner).unwrap();
        assert_eq!(runner.calls.borrow().len(), 5);
        assert!(
            runner
                .calls
                .borrow()
                .iter()
                .all(|cmd| !cmd.args.contains(&"push".into()))
        );
    }

    fn done_again(
        project: &Project,
        worktree: &Path,
        runner: &dyn Runner,
        recipient: &Recipient,
        thread: &str,
    ) -> Event {
        let op = reserve_done(
            project,
            Reservation {
                thread,
                attempt: 1,
                kind: OpKind::Done,
                recipient: recipient.clone(),
                requested: Requested::Done {
                    sha: "abc".into(),
                    report_path: "report.md".into(),
                },
                helper_pid: 1,
            },
            worktree,
        )
        .unwrap();
        stage_done(project, &op.op, worktree, runner).unwrap();
        seal(project, &op.op, |_| Ok(())).unwrap()
    }

    #[test]
    fn report_changed_after_reservation_is_not_sealed_under_wrong_hash() {
        let (root, project, runner, recipient) = fixture();
        std::fs::write(root.path().join("report.md"), "first\n").unwrap();
        let reserve = || {
            reserve_done(
                &project,
                Reservation {
                    thread: "t-0001",
                    attempt: 1,
                    kind: OpKind::Done,
                    recipient: recipient.clone(),
                    requested: Requested::Done {
                        sha: "abc".into(),
                        report_path: "report.md".into(),
                    },
                    helper_pid: 1,
                },
                root.path(),
            )
            .unwrap()
        };
        let first = reserve();
        std::fs::write(root.path().join("report.md"), "second\n").unwrap();
        assert!(
            stage_done(&project, &first.op, root.path(), &runner)
                .unwrap_err()
                .to_string()
                .contains("report_unstable")
        );
        let second = reserve();
        assert_ne!(first.op, second.op);
        stage_done(&project, &second.op, root.path(), &runner).unwrap();
        seal(&project, &second.op, |_| Ok(())).unwrap();
    }

    #[test]
    fn edited_report_with_unchanged_sha_seals_new_artifact_without_correction() {
        let (root, project, runner, recipient) = fixture();
        let report = root.path().join("report.md");
        std::fs::write(&report, "first\n").unwrap();
        let first = done_again(&project, root.path(), &runner, &recipient, "t-0001");
        std::fs::write(&report, "corrected\n").unwrap();
        let second = done_again(&project, root.path(), &runner, &recipient, "t-0001");
        assert_eq!(second.id, "t-0001-1-2");
        assert_ne!(
            first.payload.done.unwrap().artifact,
            second.payload.done.as_ref().unwrap().artifact
        );
        assert_eq!(
            done_again(&project, root.path(), &runner, &recipient, "t-0001"),
            second
        );
    }

    #[test]
    fn item_32_staged_op_seals_without_helper_memory() {
        let (root, project, runner, recipient) = fixture();
        let report = root.path().join("report.md");
        std::fs::write(&report, b"result\n").unwrap();
        let op = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Done,
                recipient,
                requested: Requested::Done {
                    sha: "abc".into(),
                    report_path: "report.md".into(),
                },
                helper_pid: 999_999,
            },
        )
        .unwrap();
        stage_done(&project, &op.op, root.path(), &runner).unwrap();
        let event = seal(&project, &op.op, |_| Ok(())).unwrap();
        assert_eq!(event.id, op.op);
        assert_eq!(load(&project, &op.op).unwrap().revision, 3);
    }

    #[test]
    fn item_32_same_payload_resumes_and_changed_payload_supersedes() {
        let (_root, project, _runner, recipient) = fixture();
        let requested = Requested::Waiting {
            text: "wait".into(),
            class: crate::contracts::FailureClass::Unknown,
            provider_kind: None,
        };
        let first = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Waiting,
                recipient: recipient.clone(),
                requested: requested.clone(),
                helper_pid: 1,
            },
        )
        .unwrap();
        assert_eq!(
            reserve(
                &project,
                Reservation {
                    thread: "t-0001",
                    attempt: 1,
                    kind: OpKind::Waiting,
                    recipient: recipient.clone(),
                    requested,
                    helper_pid: 2
                }
            )
            .unwrap()
            .op,
            first.op
        );
        let second = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Waiting,
                recipient,
                requested: Requested::Waiting {
                    text: "different".into(),
                    class: crate::contracts::FailureClass::Unknown,
                    provider_kind: None,
                },
                helper_pid: 3,
            },
        )
        .unwrap();
        assert_ne!(first.op, second.op);
        assert_eq!(load(&project, &first.op).unwrap().state, OpState::Abandoned);
    }

    #[test]
    fn item_32_x2b_repairs_marker_and_conflict_stops() {
        let (_root, project, _runner, recipient) = fixture();
        let op = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Waiting,
                recipient,
                requested: Requested::Waiting {
                    text: "wait".into(),
                    class: crate::contracts::FailureClass::Unknown,
                    provider_kind: None,
                },
                helper_pid: 1,
            },
        )
        .unwrap();
        let staged = stage_waiting(&project, &op.op).unwrap();
        let event = event_from_op(&staged).unwrap();
        events::seal_create_if_absent(&project, &event).unwrap();
        assert_eq!(load(&project, &op.op).unwrap().state, OpState::Staged);
        seal(&project, &op.op, |_| Ok(())).unwrap();
        assert_eq!(load(&project, &op.op).unwrap().state, OpState::Sealed);
    }

    #[test]
    fn waiting_text_needs_no_clean_tree_or_sha() {
        let (_root, project, _runner, recipient) = fixture();
        let op = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Waiting,
                recipient,
                requested: Requested::Waiting {
                    text: "blocked".into(),
                    class: crate::contracts::FailureClass::Unknown,
                    provider_kind: None,
                },
                helper_pid: 1,
            },
        )
        .unwrap();
        assert_eq!(
            stage_waiting(&project, &op.op).unwrap().state,
            OpState::Staged
        );
    }

    #[test]
    fn done_accepts_an_absolute_report_inside_the_worktree_but_not_outside() {
        let worktree = tempfile::tempdir().unwrap();
        let report = worktree.path().join(".herdr-project/demo/report.md");
        std::fs::create_dir_all(report.parent().unwrap()).unwrap();
        std::fs::write(&report, b"result\n").unwrap();

        assert_eq!(
            resolve_report(worktree.path(), report.to_str().unwrap()).unwrap(),
            std::fs::canonicalize(&report).unwrap()
        );
        assert_eq!(
            resolve_report(worktree.path(), ".herdr-project/demo/report.md").unwrap(),
            std::fs::canonicalize(&report).unwrap()
        );

        let outside = tempfile::NamedTempFile::new().unwrap();
        let error = resolve_report(worktree.path(), outside.path().to_str().unwrap()).unwrap_err();
        assert!(crate::refusal::is(&error));
        assert!(error.to_string().contains("report_path_invalid"));
    }

    #[cfg(unix)]
    #[test]
    fn done_resolves_report_symlinks_without_allowing_escape() {
        use std::os::unix::fs::symlink;

        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let report = worktree.path().join("report.md");
        std::fs::write(&report, b"result\n").unwrap();
        std::fs::write(outside.path().join("report.md"), b"outside\n").unwrap();
        symlink(&report, worktree.path().join("inside.md")).unwrap();
        symlink(outside.path(), worktree.path().join("escape")).unwrap();
        assert_eq!(
            resolve_report(worktree.path(), "inside.md").unwrap(),
            std::fs::canonicalize(&report).unwrap()
        );
        for requested in [
            "escape/report.md".to_string(),
            worktree
                .path()
                .join("escape/report.md")
                .to_string_lossy()
                .into_owned(),
        ] {
            let error = resolve_report(worktree.path(), &requested).unwrap_err();
            assert!(crate::refusal::is(&error));
            assert!(error.to_string().starts_with("report_path_invalid:"));
        }
    }

    #[test]
    fn done_refuses_dirty_tree_and_wrong_sha() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let runner = FakeRunner::new();
        runner.on("git status --short", ok(" M src/lib.rs\n"));
        let recipient = Recipient {
            pane: "w1:p1".into(),
            coordinator_attempt: 1,
        };
        std::fs::write(root.path().join("report.md"), b"result\n").unwrap();
        let op = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Done,
                recipient: recipient.clone(),
                requested: Requested::Done {
                    sha: "abc".into(),
                    report_path: "report.md".into(),
                },
                helper_pid: 1,
            },
        )
        .unwrap();
        let error = stage_done(&project, &op.op, root.path(), &runner).unwrap_err();
        assert!(crate::refusal::is(&error));
        assert!(error.to_string().contains("worktree_dirty"));

        let root2 = tempfile::tempdir().unwrap();
        let project2 = project::create(root2.path(), "demo", "", vec![]).unwrap();
        std::fs::write(root2.path().join("report.md"), b"result\n").unwrap();
        let runner2 = FakeRunner::new();
        runner2
            .on("git status --short", ok(""))
            .on("git rev-parse HEAD", ok("different\n"));
        let op2 = reserve(
            &project2,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Done,
                recipient,
                requested: Requested::Done {
                    sha: "abc".into(),
                    report_path: "report.md".into(),
                },
                helper_pid: 1,
            },
        )
        .unwrap();
        let error = stage_done(&project2, &op2.op, root2.path(), &runner2).unwrap_err();
        assert!(crate::refusal::is(&error));
        assert!(error.to_string().contains("sha_mismatch"));
    }

    #[test]
    fn done_refuses_a_commit_that_tracks_project_runtime_files() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let repo_s = repo.to_string_lossy().into_owned();
        let git = |args: &[&str]| {
            let out = RealRunner
                .run(
                    &Cmd::new("git", std::time::Duration::from_secs(5))
                        .args(["-C", &repo_s])
                        .args(args.iter().copied()),
                )
                .unwrap();
            assert!(out.success(), "git {:?}: {}", args, out.error_text());
            out.stdout.trim().to_string()
        };
        let init = RealRunner
            .run(
                &Cmd::new("git", std::time::Duration::from_secs(5))
                    .args(["init", "-b", "main", &repo_s]),
            )
            .unwrap();
        assert!(init.success(), "{}", init.error_text());
        git(&["config", "user.email", "ade@test"]);
        git(&["config", "user.name", "ade"]);
        std::fs::write(repo.join(".git/info/exclude"), ".herdr-project/\n").unwrap();
        let report = repo.join(".herdr-project/x/report.md");
        std::fs::create_dir_all(report.parent().unwrap()).unwrap();
        std::fs::write(&report, "result\n").unwrap();
        git(&["add", "-f", ".herdr-project/x/report.md"]);
        git(&["commit", "-m", "track runtime report"]);
        let tracked_sha = git(&["rev-parse", "HEAD"]);

        let project = project::create(&root.path().join("state"), "demo", "", vec![]).unwrap();
        let recipient = Recipient {
            pane: "w1:p1".into(),
            coordinator_attempt: 1,
        };
        crate::thread::allocate(&project, |t| t.base = tracked_sha.clone()).unwrap();
        let tracked_op = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Done,
                recipient: recipient.clone(),
                requested: Requested::Done {
                    sha: tracked_sha,
                    report_path: ".herdr-project/x/report.md".into(),
                },
                helper_pid: 1,
            },
        )
        .unwrap();
        let error = stage_done(&project, &tracked_op.op, &repo, &RealRunner).unwrap_err();
        assert!(crate::refusal::is(&error));
        let error = error.to_string();
        assert!(error.starts_with("worktree_dirty:"), "{error}");
        assert!(error.contains(".herdr-project/x/report.md"), "{error}");
        assert!(
            error.contains("untrack these paths, commit, then run `ha done` again"),
            "{error}"
        );

        git(&["rm", "--cached", ".herdr-project/x/report.md"]);
        git(&["commit", "-m", "untrack runtime report"]);
        let clean_sha = git(&["rev-parse", "HEAD"]);
        let clean_op = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Done,
                recipient,
                requested: Requested::Done {
                    sha: clean_sha,
                    report_path: ".herdr-project/x/report.md".into(),
                },
                helper_pid: 1,
            },
        )
        .unwrap();
        stage_done(&project, &clean_op.op, &repo, &RealRunner).unwrap();
        seal(&project, &clean_op.op, |_| Ok(())).unwrap();
        assert_eq!(load(&project, &clean_op.op).unwrap().state, OpState::Sealed);
    }

    #[test]
    fn done_refuses_report_that_changes_between_reads() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let runner = FakeRunner::new();
        let recipient = Recipient {
            pane: "w1:p1".into(),
            coordinator_attempt: 1,
        };
        let report = root.path().join("report.md");
        std::fs::write(&report, b"first\n").unwrap();
        let changed = report.clone();
        runner.on_fn(
            |cmd| cmd.display().contains("git status --short"),
            move |_| {
                std::fs::write(&changed, b"second\n")?;
                Ok(ok(""))
            },
        );
        runner
            .on("git rev-parse HEAD", ok("abc\n"))
            .on("git ls-tree", ok(""));
        let op = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Done,
                recipient,
                requested: Requested::Done {
                    sha: "abc".into(),
                    report_path: "report.md".into(),
                },
                helper_pid: 1,
            },
        )
        .unwrap();
        assert!(
            stage_done(&project, &op.op, root.path(), &runner)
                .unwrap_err()
                .to_string()
                .contains("report_unstable")
        );
    }

    #[test]
    fn helper_and_ticker_race_produce_one_event() {
        let (_root, project, _runner, recipient) = fixture();
        let op = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Waiting,
                recipient,
                requested: Requested::Waiting {
                    text: "wait".into(),
                    class: crate::contracts::FailureClass::Unknown,
                    provider_kind: None,
                },
                helper_pid: 1,
            },
        )
        .unwrap();
        stage_waiting(&project, &op.op).unwrap();
        let a_project = project.clone();
        let b_project = project.clone();
        let a_id = op.op.clone();
        let b_id = op.op.clone();
        let a = std::thread::spawn(move || seal(&a_project, &a_id, |_| Ok(())).unwrap());
        let b = std::thread::spawn(move || seal(&b_project, &b_id, |_| Ok(())).unwrap());
        assert_eq!(a.join().unwrap(), b.join().unwrap());
        assert_eq!(events::list(&project).len(), 1);
    }

    #[test]
    fn box_recovery_repairs_an_interrupted_receipt_twice_and_abandons_a_dead_reserved_op() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let recipient = Recipient {
            pane: "w1:p1".into(),
            coordinator_attempt: 1,
        };
        std::fs::create_dir_all(project.state_dir().join("lanes")).unwrap();
        for (thread, attempt) in [("t-0001", 1), ("t-0002", 1)] {
            let card = crate::contracts::LaneCard {
                project: "demo".into(),
                thread: thread.into(),
                attempt,
                brief_hash: "abcd".into(),
                role: "lane".into(),
                kind: "pi".into(),
                pane_id: "w2:p1".into(),
                machine_label: "buildbox".into(),
                machine_id: "1".into(),
                box_repo: "/box/repo".into(),
                box_worktree: "/box/wt".into(),
                brief_commit: "b0".into(),
                branch: format!("hp/demo/{thread}"),
                publish_url: "https://github.com/uguryildirim24/herdr-ade.git".into(),
                recipient: recipient.clone(),
                start_line: "Run the box skill".into(),
                created: "2026-09-19T00:00:00Z".into(),
            };
            std::fs::write(
                project
                    .state_dir()
                    .join("lanes")
                    .join(format!("{thread}.toml")),
                toml::to_string(&card).unwrap(),
            )
            .unwrap();
        }
        let staged = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Waiting,
                recipient: recipient.clone(),
                requested: Requested::Waiting {
                    text: "wait".into(),
                    class: crate::contracts::FailureClass::Unknown,
                    provider_kind: None,
                },
                helper_pid: 1,
            },
        )
        .unwrap();
        let staged_op = stage_waiting(&project, &staged.op).unwrap();
        let event = event_from_op(&staged_op).unwrap();
        events::seal_create_if_absent(&project, &event).unwrap();
        // Old writer crashed after create_new of the final receipt, before
        // writing any bytes. The event exists but its op is still staged.
        project.record_dir_for_write("receipts").unwrap();
        std::fs::write(events::receipt_path(&project, &event.id).unwrap(), b"").unwrap();
        let reserved = reserve(
            &project,
            Reservation {
                thread: "t-0002",
                attempt: 1,
                kind: OpKind::Waiting,
                recipient,
                requested: Requested::Waiting {
                    text: "still here".into(),
                    class: crate::contracts::FailureClass::Unknown,
                    provider_kind: None,
                },
                helper_pid: 999_999,
            },
        )
        .unwrap();

        let runner = FakeRunner::new();
        runner.on("/bin/kill -0", fail(1, "gone"));
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let ctx = Ctx {
            env: &env,
            root: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        recover_box(&ctx).unwrap();
        recover_box(&ctx).unwrap();
        let receipt_path = events::receipt_path(&project, &event.id).unwrap();
        let receipt: events::Receipt =
            toml::from_str(&std::fs::read_to_string(&receipt_path).unwrap()).unwrap();
        assert_eq!(receipt.event, event.id);
        assert_eq!(
            receipt.event_hash,
            crate::thread::sha256_hex(&events::bytes(&event).unwrap())
        );
        assert_eq!(load(&project, &staged.op).unwrap().state, OpState::Sealed);
        assert_eq!(events::list(&project).len(), 1);
        assert!(events::receipt_path(&project, &staged.op).unwrap().exists());
        assert_eq!(
            load(&project, &reserved.op).unwrap().state,
            OpState::Abandoned
        );
    }

    #[test]
    fn dead_helper_after_stage_is_sealed_by_tick() {
        let (root, project, runner, recipient) = fixture();
        let lane = crate::thread::allocate(&project, |lane| {
            lane.launch_attempts = 1;
            lane.pane_id = "w1:p2".into();
            lane.cwd = root.path().display().to_string();
        })
        .unwrap();
        project
            .update_coordinator(|coordinator| {
                coordinator.socket = "/tmp/fake.sock".into();
                coordinator.pane_id = recipient.pane.clone();
                coordinator.agent_name = "hp-demo-coordinator".into();
                coordinator.launch_attempts = 1;
            })
            .unwrap();
        runner
            .on("/bin/kill -0", fail(1, "gone"))
            .on(
                "agent list",
                ok(r#"{"result":{"agents":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","name":"hp-demo-coordinator","agent":"claude","agent_status":"idle","cwd":"/tmp"}]}}"#),
            )
            .on("pane report-metadata", ok(r#"{"result":{}}"#))
            .on("agent prompt", ok(r#"{"result":{}}"#));
        let op = reserve(
            &project,
            Reservation {
                thread: &lane.id,
                attempt: 1,
                kind: OpKind::Waiting,
                recipient,
                requested: Requested::Waiting {
                    text: "wait".into(),
                    class: crate::contracts::FailureClass::Unknown,
                    provider_kind: None,
                },
                helper_pid: 999_999,
            },
        )
        .unwrap();
        stage_waiting(&project, &op.op).unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[("HERDR_BIN_PATH", "herdr")]);
        let ctx = Ctx {
            env: &env,
            root: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        tick(&ctx, &project).unwrap();
        assert_eq!(load(&project, &op.op).unwrap().state, OpState::Sealed);
        assert_eq!(events::list(&project).len(), 1);
    }
}
