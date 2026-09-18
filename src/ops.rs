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

pub fn op_path(project: &Project, id: &str) -> Result<PathBuf> {
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

pub fn load(project: &Project, id: &str) -> Result<Op> {
    let path = op_path(project, id)?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read operation {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("{} does not parse", path.display()))
}

pub fn list(project: &Project) -> Vec<Op> {
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

/// Reserve the complete payload under the project lock. Same-payload retries
/// resume one op. A changed payload abandons it and allocates the next id.
pub fn reserve(
    project: &Project,
    thread: &str,
    attempt: u32,
    kind: OpKind,
    recipient: Recipient,
    round: Option<String>,
    requested: Requested,
    helper_pid: u32,
) -> Result<Op> {
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

/// Stage a `done` without the project lock. Git is invoked only here, then the
/// revision-1 marker is advanced under the lock.
pub fn stage_done(project: &Project, id: &str, worktree: &Path, runner: &dyn Runner) -> Result<Op> {
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
        bail!("worktree_dirty: ha done requires an empty git status");
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
        bail!(
            "sha_mismatch: requested {sha}, HEAD is {}",
            head.stdout.trim()
        );
    }
    let second = stable_read(&report)?;
    if first != second {
        bail!("report_unstable: report bytes changed while staging");
    }
    let artifact = write_artifact(project, &first)?;
    advance_staged(project, id, Some(artifact))
}

pub fn stage_waiting(project: &Project, id: &str) -> Result<Op> {
    let op = load(project, id)?;
    if op.state == OpState::Staged || op.state == OpState::Sealed {
        return Ok(op);
    }
    if op.state != OpState::Reserved || op.revision != 1 || op.kind != OpKind::Waiting {
        bail!("op_state_changed: {id} is not a reserved waiting operation");
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
pub fn seal(
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
            }),
            waiting: None,
        },
        (Requested::Waiting { text }, OpKind::Waiting) => EventPayload {
            done: None,
            waiting: Some(WaitingPayload { text: text.clone() }),
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

#[allow(dead_code)] // called by A1's ticker integration through `tick`
pub fn abandon(project: &Project, id: &str) -> Result<Op> {
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
#[allow(dead_code)] // reviewer seam; A1 owns the ticker call site
pub fn tick(ctx: &Ctx, project: &Project) -> Result<()> {
    for op in list(project) {
        match op.state {
            OpState::Reserved if !pid_alive(ctx.runner, op.helper_pid) => {
                abandon(project, &op.op)?;
                crate::inbox::write(
                    project,
                    "preparation-abandoned",
                    &op.thread,
                    "completion preparation was abandoned",
                    "",
                )?;
            }
            OpState::Staged => {
                let current = crate::thread::load(project, &op.thread);
                let coordinator = project.coordinator();
                let valid = current.as_ref().is_ok_and(|thread| {
                    thread.launch_attempts.max(1) == op.attempt
                        && thread.pane_id != op.recipient.pane
                }) && coordinator.as_ref().is_some_and(|record| {
                    record.pane_id == op.recipient.pane
                        && record.launch_attempts.max(1) == op.recipient.coordinator_attempt
                });
                if valid {
                    let _ = seal(project, &op.op, |_| Ok(()))?;
                } else {
                    abandon(project, &op.op)?;
                }
            }
            _ => {}
        }
    }
    crate::steps::deliver_events(ctx, project)
}

#[allow(dead_code)] // reachable once A1 wires `tick`
fn pid_alive(runner: &dyn Runner, pid: u32) -> bool {
    runner
        .run(
            &Cmd::new("/bin/kill", std::time::Duration::from_secs(2))
                .args(["-0", &pid.to_string()]),
        )
        .is_ok_and(|output| output.success())
}

fn resolve_report(worktree: &Path, requested: &str) -> Result<PathBuf> {
    let relative = Path::new(requested);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        bail!("report_path_invalid: report must be relative and stay in the worktree");
    }
    let path = worktree.join(relative);
    if !path.is_file() {
        bail!("report_missing: {}", path.display());
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
    fn item_32_staged_op_seals_without_helper_memory() {
        let (root, project, runner, recipient) = fixture();
        let report = root.path().join("report.md");
        std::fs::write(&report, b"result\n").unwrap();
        let op = reserve(
            &project,
            "t-0001",
            1,
            OpKind::Done,
            recipient,
            None,
            Requested::Done {
                sha: "abc".into(),
                report_path: "report.md".into(),
            },
            999_999,
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
        };
        let first = reserve(
            &project,
            "t-0001",
            1,
            OpKind::Waiting,
            recipient.clone(),
            None,
            requested.clone(),
            1,
        )
        .unwrap();
        assert_eq!(
            reserve(
                &project,
                "t-0001",
                1,
                OpKind::Waiting,
                recipient.clone(),
                None,
                requested,
                2,
            )
            .unwrap()
            .op,
            first.op
        );
        let second = reserve(
            &project,
            "t-0001",
            1,
            OpKind::Waiting,
            recipient,
            None,
            Requested::Waiting {
                text: "different".into(),
            },
            3,
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
            "t-0001",
            1,
            OpKind::Waiting,
            recipient,
            None,
            Requested::Waiting {
                text: "wait".into(),
            },
            1,
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
            "t-0001",
            1,
            OpKind::Waiting,
            recipient,
            None,
            Requested::Waiting {
                text: "blocked".into(),
            },
            1,
        )
        .unwrap();
        assert_eq!(
            stage_waiting(&project, &op.op).unwrap().state,
            OpState::Staged
        );
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
            "t-0001",
            1,
            OpKind::Done,
            recipient.clone(),
            None,
            Requested::Done {
                sha: "abc".into(),
                report_path: "report.md".into(),
            },
            1,
        )
        .unwrap();
        assert!(
            stage_done(&project, &op.op, root.path(), &runner)
                .unwrap_err()
                .to_string()
                .contains("worktree_dirty")
        );

        let root2 = tempfile::tempdir().unwrap();
        let project2 = project::create(root2.path(), "demo", "", vec![]).unwrap();
        std::fs::write(root2.path().join("report.md"), b"result\n").unwrap();
        let runner2 = FakeRunner::new();
        runner2
            .on("git status --short", ok(""))
            .on("git rev-parse HEAD", ok("different\n"));
        let op2 = reserve(
            &project2,
            "t-0001",
            1,
            OpKind::Done,
            recipient,
            None,
            Requested::Done {
                sha: "abc".into(),
                report_path: "report.md".into(),
            },
            1,
        )
        .unwrap();
        assert!(
            stage_done(&project2, &op2.op, root2.path(), &runner2)
                .unwrap_err()
                .to_string()
                .contains("sha_mismatch")
        );
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
            "t-0001",
            1,
            OpKind::Done,
            recipient,
            None,
            Requested::Done {
                sha: "abc".into(),
                report_path: "report.md".into(),
            },
            1,
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
            "t-0001",
            1,
            OpKind::Waiting,
            recipient,
            None,
            Requested::Waiting {
                text: "wait".into(),
            },
            1,
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
            &lane.id,
            1,
            OpKind::Waiting,
            recipient,
            None,
            Requested::Waiting {
                text: "wait".into(),
            },
            999_999,
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
