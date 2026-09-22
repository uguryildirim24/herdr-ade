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
    project.dir().join("ops")
}

fn artifacts_dir(project: &Project) -> PathBuf {
    project.dir().join("artifacts")
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
    std::fs::create_dir_all(ops_dir(project))?;
    project::write_atomic(&op_path(project, &op.op)?, toml::to_string(op)?.as_bytes())
}

/// The complete payload of one `ha done` or `ha waiting`.
pub(crate) struct Reservation<'a> {
    pub(crate) thread: &'a str,
    pub(crate) attempt: u32,
    pub(crate) kind: OpKind,
    pub(crate) recipient: Recipient,
    pub(crate) round: Option<String>,
    pub(crate) requested: Requested,
    pub(crate) helper_pid: u32,
}

/// Reserve the complete payload under the project lock. Same-payload retries
/// resume one op. A changed payload abandons it and allocates the next id.
pub(crate) fn reserve(project: &Project, r: Reservation<'_>) -> Result<Op> {
    let Reservation {
        thread,
        attempt,
        kind,
        recipient,
        round,
        requested,
        helper_pid,
    } = r;
    let _lock = project.lock()?;
    let mut existing: Vec<Op> = list(project)
        .into_iter()
        .filter(|op| op.thread == thread && op.attempt == attempt)
        .collect();
    existing.sort_by(|a, b| a.op.cmp(&b.op));
    if let Some(op) = existing.iter().rev().find(|op| {
        op.state != OpState::Abandoned
            && op.kind == kind
            && op.requested == requested
            && op.recipient == recipient
            && op.round == round
    }) {
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
        round,
        helper_pid,
        requested,
        event: id,
        state: OpState::Reserved,
        created: project::now(),
        artifact: None,
    };
    write_op(project, &op)?;
    Ok(op)
}

/// The published lane ref must equal `sha` before a box `done` stages
/// (SPEC-remote §4.3): one `git ls-remote` against the URL-matched remote.
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
        bail!("published_ref_check_failed: {}", out.error_text());
    }
    let found = out.stdout.split_whitespace().next().unwrap_or("");
    let publish = format!(
        "git -C {} push {} {}",
        crate::remote::quote(&worktree.to_string_lossy()),
        crate::remote::quote(publish_url),
        crate::remote::quote(&format!("{sha}:refs/heads/{branch}")),
    );
    if found.is_empty() {
        return Err(crate::refusal::error(format!(
            "lane_ref_not_published: `{branch}` is not on {publish_url}; run `{publish}`, then retry `ha done`"
        )));
    }
    if found != sha {
        return Err(crate::refusal::error(format!(
            "published_ref_mismatch: `{branch}` is {found} on {publish_url}, not {sha}; run `{publish}`, then retry `ha done`"
        )));
    }
    Ok(())
}

/// Stage a `done` without the project lock. Git is invoked only here, then the
/// revision-1 marker is advanced under the lock.
pub(crate) fn stage_done(
    project: &Project,
    id: &str,
    worktree: &Path,
    runner: &dyn Runner,
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
            "worktree_dirty: ha done requires an empty git status",
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
        return Err(crate::refusal::error(format!(
            "sha_mismatch: requested {sha}, HEAD is {}",
            head.stdout.trim()
        )));
    }
    let second = stable_read(&report)?;
    if first != second {
        bail!("report_unstable: report bytes changed while staging");
    }
    let artifact = write_artifact(project, &first)?;
    advance_staged(project, id, Some(artifact))
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
    advance_staged(project, id, None)
}

fn advance_staged(project: &Project, id: &str, artifact: Option<String>) -> Result<Op> {
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
    // carries it to the Mac ledger.
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
                sha: sha.clone(),
                report_path: report_path.clone(),
                artifact: op
                    .artifact
                    .clone()
                    .context("op_payload_invalid: staged done has no artifact")?,
                attestation: None,
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
        round: op.round.clone(),
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
    let path = project.dir().join("lanes").join(format!("{thread}.toml"));
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
        ));
    }
    let path = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        worktree.join(requested)
    };
    if !path.is_file() {
        return Err(crate::refusal::error(format!(
            "report_missing: {}",
            path.display()
        )));
    }
    let worktree = std::fs::canonicalize(worktree)
        .with_context(|| format!("could not resolve worktree {}", worktree.display()))?;
    let path = std::fs::canonicalize(&path)
        .with_context(|| format!("could not resolve report {}", path.display()))?;
    if !path.starts_with(&worktree) {
        return Err(crate::refusal::error(
            "report_path_invalid: report must stay in the worktree",
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
    let dir = artifacts_dir(project);
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
    use crate::runner::fake::{FakeRunner, fail, ok};

    fn fixture() -> (tempfile::TempDir, Project, FakeRunner, Recipient) {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let runner = FakeRunner::new();
        runner
            .on("git status --short", ok(""))
            .on("git rev-parse HEAD", ok("abc\n"));
        let recipient = Recipient {
            pane: "w1:p1".into(),
            coordinator_attempt: 1,
        };
        (root, project, runner, recipient)
    }

    #[test]
    fn unpublished_ref_refusals_name_the_exact_safe_repair_command() {
        let worktree = Path::new("/box/reviewer's worktree");
        let branch = "hp/demo/reviewer";
        let url = "/remotes/publish repo.git";
        let repair = "git -C '/box/reviewer'\\''s worktree' push '/remotes/publish repo.git' new:refs/heads/hp/demo/reviewer";
        for (remote_output, reason) in [
            (
                "",
                "lane_ref_not_published: `hp/demo/reviewer` is not on /remotes/publish repo.git",
            ),
            (
                "old\trefs/heads/hp/demo/reviewer\n",
                "published_ref_mismatch: `hp/demo/reviewer` is old on /remotes/publish repo.git, not new",
            ),
        ] {
            let runner = FakeRunner::new();
            runner.on("ls-remote", ok(remote_output));
            let error = check_published_ref(&runner, worktree, branch, url, "new").unwrap_err();
            assert!(crate::refusal::is(&error));
            assert_eq!(
                error.to_string(),
                format!("{reason}; run `{repair}`, then retry `ha done`")
            );
            let calls = runner.calls.borrow();
            assert_eq!(calls.len(), 1, "a refusal must never push implicitly");
            assert_eq!(
                calls[0].args,
                [
                    "-C",
                    "/box/reviewer's worktree",
                    "ls-remote",
                    url,
                    "refs/heads/hp/demo/reviewer"
                ]
            );
        }
        let runner = FakeRunner::new();
        runner.on("ls-remote", ok("new\trefs/heads/hp/demo/reviewer\n"));
        check_published_ref(&runner, worktree, branch, url, "new").unwrap();
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
                round: None,
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
                round: None,
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
                    round: None,
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
                round: None,
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
                round: None,
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
                round: None,
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
                round: None,
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
                round: None,
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
        runner.on("git rev-parse HEAD", ok("abc\n"));
        let op = reserve(
            &project,
            Reservation {
                thread: "t-0001",
                attempt: 1,
                kind: OpKind::Done,
                recipient,
                round: None,
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
                round: None,
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
    fn box_recovery_seals_a_staged_op_from_its_card_and_abandons_a_dead_reserved_one() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let recipient = Recipient {
            pane: "w1:p1".into(),
            coordinator_attempt: 1,
        };
        std::fs::create_dir_all(project.dir().join("lanes")).unwrap();
        for (thread, attempt) in [("t-0001", 1), ("t-0002", 1)] {
            let card = crate::contracts::LaneCard {
                project: "demo".into(),
                thread: thread.into(),
                attempt,
                brief_hash: "abcd".into(),
                role: "lane".into(),
                kind: "pi".into(),
                pane_id: "w2:p1".into(),
                machine_label: "oci".into(),
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
                project.dir().join("lanes").join(format!("{thread}.toml")),
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
                round: None,
                requested: Requested::Waiting {
                    text: "wait".into(),
                    class: crate::contracts::FailureClass::Unknown,
                    provider_kind: None,
                },
                helper_pid: 1,
            },
        )
        .unwrap();
        stage_waiting(&project, &staged.op).unwrap();
        let reserved = reserve(
            &project,
            Reservation {
                thread: "t-0002",
                attempt: 1,
                kind: OpKind::Waiting,
                recipient,
                round: None,
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
                round: None,
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
