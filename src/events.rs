//! Immutable completion events and their append-only delivery journals.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::contracts::{DeliveryLine, DeliveryState, Event, EventPayload};
use crate::project::Project;

fn events_dir(project: &Project) -> PathBuf {
    project.dir().join("events")
}

fn deliveries_dir(project: &Project) -> PathBuf {
    project.dir().join("deliveries")
}

pub fn event_path(project: &Project, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(events_dir(project).join(format!("{id}.toml")))
}

fn journal_path(project: &Project, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(deliveries_dir(project).join(format!("{id}.jsonl")))
}

fn validate_id(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && !id.starts_with('.')
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if !valid {
        bail!("`{id}` is not an event id");
    }
    Ok(())
}

/// Canonical bytes used both for the create-if-absent write and the X2b
/// equality check. The event's field order is fixed by the contract type.
pub fn bytes(event: &Event) -> Result<Vec<u8>> {
    let mut text = toml::to_string(event)?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text.into_bytes())
}

/// Creates an immutable event. If another helper already created it, only
/// exact byte equality is accepted.
pub fn seal_create_if_absent(project: &Project, event: &Event) -> Result<()> {
    std::fs::create_dir_all(events_dir(project))?;
    let path = event_path(project, &event.id)?;
    let expected = bytes(event)?;
    // Written whole beside the target, then linked into place: a reader never
    // sees a half-written event (the dot name is skipped by every lister).
    let tmp = events_dir(project).join(format!(".{}.{}.tmp", event.id, std::process::id()));
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&expected)?;
        file.sync_all()?;
    }
    let linked = std::fs::hard_link(&tmp, &path);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => {
            sync_parent(&path)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let actual = std::fs::read(&path)
                .with_context(|| format!("could not read sealed event {}", path.display()))?;
            if actual == expected {
                Ok(())
            } else {
                bail!(
                    "event_conflict: sealed event {} has different bytes",
                    event.id
                )
            }
        }
        Err(error) => Err(error).with_context(|| format!("could not seal {}", path.display())),
    }
}

pub fn load(project: &Project, id: &str) -> Result<Event> {
    let path = event_path(project, id)?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read event {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("{} does not parse", path.display()))
}

pub fn list(project: &Project) -> Vec<Event> {
    let Ok(entries) = std::fs::read_dir(events_dir(project)) else {
        return Vec::new();
    };
    let mut events: Vec<Event> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".toml").map(str::to_owned))
        .filter_map(|id| load(project, &id).ok())
        .collect();
    events.sort_by(|a, b| a.id.cmp(&b.id));
    events
}

/// Appends a fact once. Re-running acknowledgement or handling is idempotent;
/// `submitted` may still be duplicated when the transport succeeded before a
/// crash, which is the intentional X4 at-least-once boundary.
pub fn append_delivery(project: &Project, event: &str, state: DeliveryState) -> Result<()> {
    let _lock = project.lock()?;
    append_delivery_locked(project, event, state)
}

pub(crate) fn append_delivery_locked(
    project: &Project,
    event: &str,
    state: DeliveryState,
) -> Result<()> {
    std::fs::create_dir_all(deliveries_dir(project))?;
    if state != DeliveryState::Submitted && states(project, event)?.contains(&state) {
        return Ok(());
    }
    let path = journal_path(project, event)?;
    let line = DeliveryLine {
        event: event.to_string(),
        state,
    };
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    serde_json::to_writer(&mut file, &line)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    sync_parent(&path)
}

pub fn delivery_lines(project: &Project, event: &str) -> Result<Vec<DeliveryLine>> {
    let path = journal_path(project, event)?;
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).context("delivery journal line does not parse"))
        .collect()
}

pub fn states(project: &Project, event: &str) -> Result<Vec<DeliveryState>> {
    Ok(delivery_lines(project, event)?
        .into_iter()
        .map(|line| line.state)
        .collect())
}

pub fn typed_line(event: &Event) -> Result<String> {
    match &event.payload {
        EventPayload {
            done: Some(done),
            waiting: None,
        } => Ok(format!(
            "DONE {} {} {}",
            event.thread, done.report_path, done.sha
        )),
        EventPayload {
            done: None,
            waiting: Some(waiting),
        } => Ok(format!("WAITING {} {}", event.thread, waiting.text)),
        _ => bail!(
            "event_payload_invalid: {} has no single tagged payload",
            event.id
        ),
    }
}

fn sync_parent(path: &std::path::Path) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{DonePayload, Recipient};
    use crate::project;

    fn fixture() -> (tempfile::TempDir, Project, Event) {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let event = Event {
            id: "t-0001-1-1".into(),
            op: "t-0001-1-1".into(),
            thread: "t-0001".into(),
            attempt: 1,
            round: None,
            recipient: Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 1,
            },
            created: "2026-09-18T00:00:00Z".into(),
            payload: EventPayload {
                done: Some(DonePayload {
                    sha: "abc".into(),
                    report_path: ".reports/lane.md".into(),
                    artifact: "def".into(),
                }),
                waiting: None,
            },
        };
        (root, project, event)
    }

    #[test]
    fn seal_is_create_only_and_byte_equal() {
        let (_root, project, event) = fixture();
        seal_create_if_absent(&project, &event).unwrap();
        seal_create_if_absent(&project, &event).unwrap();
        let mut changed = event.clone();
        changed.created.push('x');
        assert!(
            seal_create_if_absent(&project, &changed)
                .unwrap_err()
                .to_string()
                .contains("event_conflict")
        );
    }

    #[test]
    fn delivery_journal_is_append_only_and_ack_is_idempotent() {
        let (_root, project, event) = fixture();
        append_delivery(&project, &event.id, DeliveryState::Submitted).unwrap();
        append_delivery(&project, &event.id, DeliveryState::Submitted).unwrap();
        append_delivery(&project, &event.id, DeliveryState::Acknowledged).unwrap();
        append_delivery(&project, &event.id, DeliveryState::Acknowledged).unwrap();
        let states = states(&project, &event.id).unwrap();
        assert_eq!(
            states,
            vec![
                DeliveryState::Submitted,
                DeliveryState::Submitted,
                DeliveryState::Acknowledged
            ]
        );
    }
}
