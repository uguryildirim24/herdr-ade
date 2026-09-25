//! Inbox items hold messages, not projections of thread or round records.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::project::{self, Project};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub(crate) struct Item {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) subject: String,
    pub(crate) created: String,
    pub(crate) summary: String,
    /// Sealed event id when this item is a delivery projection.
    pub(crate) event: String,
    /// Optional message body.
    #[serde(skip)]
    pub(crate) body: String,
}

pub(crate) fn inbox_dir(project: &Project) -> PathBuf {
    project.record_dir("inbox")
}

fn parse(text: &str) -> Option<Item> {
    let rest = text.strip_prefix("+++\n")?;
    let (front, body) = rest
        .split_once("\n+++\n")
        .or_else(|| Some((rest.strip_suffix("\n+++")?, "")))?;
    let mut item: Item = toml::from_str(front).ok()?;
    item.body = body.trim_matches('\n').to_string();
    (!removed_kind(&item.kind)).then_some(item)
}

/// Old projections are ignored on read; no migration or replacement files.
fn removed_kind(kind: &str) -> bool {
    matches!(
        kind,
        "thread-state"
            | "report-available"
            | "preparation-abandoned"
            | "round-advance"
            | "merge-diverged"
            | "merge-pending"
            | "pr"
            | "lineage-mismatch"
            | "done"
            | "waiting"
    )
}

/// File-name-safe form of a subject (a thread id or machine label).
fn safe_subject(subject: &str) -> String {
    let cleaned: String = subject
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    let cleaned = cleaned.trim_matches('-').to_string();
    if cleaned.is_empty() {
        "item".to_string()
    } else {
        cleaned
    }
}

/// Writes one item. The id is `<UTC timestamp>-<kind>-<subject>-<n>`, where
/// `<n>` is a counter allocated under the project lock, so two events in one
/// tick never share a name. `body` is optional.
pub(crate) fn write(
    project: &Project,
    kind: &str,
    subject: &str,
    summary: &str,
    body: &str,
) -> Result<String> {
    if removed_kind(kind) {
        bail!("inbox_record_kind: `{kind}` belongs in its owning record");
    }
    let _lock = project.lock()?;
    project.record_dir_for_write("inbox")?;
    let counter_path = project.state_dir().join("inbox-counter.json");
    let n: u64 = project::read_json::<u64>(&counter_path).unwrap_or(0) + 1;
    project::write_json(&counter_path, &n)?;
    let stamp = jiff::Timestamp::now()
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string();
    let id = format!("{stamp}-{kind}-{}-{n}", safe_subject(subject));
    let item = Item {
        id: id.clone(),
        kind: kind.to_string(),
        subject: subject.to_string(),
        created: project::now(),
        // One line, no control characters: summaries are printed in the digest.
        summary: summary
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect(),
        event: String::new(),
        body: String::new(),
    };
    let mut text = format!("+++\n{}+++\n", toml::to_string(&item)?);
    if !body.is_empty() {
        text.push('\n');
        text.push_str(body.trim_end());
        text.push('\n');
    }
    project::write_atomic(
        &inbox_dir(project).join(format!("{id}.md")),
        text.as_bytes(),
    )?;
    Ok(id)
}

/// Writes the one stable inbox projection for a sealed event. A retry observes
/// the existing item instead of allocating a second counter id.
pub(crate) fn write_event(
    project: &Project,
    event: &crate::contracts::Event,
    kind: &str,
    summary: &str,
) -> Result<String> {
    if removed_kind(kind) {
        bail!("inbox_record_kind: `{kind}` belongs in its owning record");
    }
    let _lock = project.lock()?;
    project.record_dir_for_write("inbox")?;
    let id = format!("event-{}", event.id);
    validate_id(&id)?;
    let path = inbox_dir(project).join(format!("{id}.md"));
    if path.exists()
        || inbox_dir(project)
            .join("done")
            .join(format!("{id}.md"))
            .exists()
    {
        return Ok(id);
    }
    let item = Item {
        id: id.clone(),
        kind: kind.to_string(),
        subject: event.thread.clone(),
        created: project::now(),
        summary: summary
            .chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect(),
        event: event.id.clone(),
        body: String::new(),
    };
    let text = format!("+++\n{}+++\n", toml::to_string(&item)?);
    project::write_atomic(&path, text.as_bytes())?;
    Ok(id)
}

/// Deletes handled items older than `days`.
pub(crate) fn prune_done(project: &Project, days: u64) {
    let Ok(entries) = std::fs::read_dir(inbox_dir(project).join("done")) else {
        return;
    };
    let limit = std::time::Duration::from_secs(days * 24 * 3600);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > limit);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Unhandled items, oldest first. Event ids do not start with a timestamp.
pub(crate) fn unhandled(project: &Project) -> Vec<Item> {
    let Ok(entries) = std::fs::read_dir(inbox_dir(project)) else {
        return Vec::new();
    };
    let mut items: Vec<Item> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".md"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|text| parse(&text))
        .collect();
    items.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    items
}

pub(crate) fn seen(project: &Project) -> BTreeSet<String> {
    project::read_json(&project.state_dir().join("inbox-seen.json")).unwrap_or_default()
}

/// Records that `context` showed these items, so they are announced once only.
pub(crate) fn mark_seen(project: &Project, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let _lock = project.lock()?;
    let mut all = seen(project);
    all.extend(ids.iter().cloned());
    // Ids of items that no longer exist are dropped so the file stays small.
    let live: BTreeSet<String> = unhandled(project).into_iter().map(|i| i.id).collect();
    all.retain(|id| live.contains(id));
    project::write_json(&project.state_dir().join("inbox-seen.json"), &all)
}

/// A context read acknowledges only events shown to their exact coordinator
/// pane and attempt. `--peek` never calls this function.
pub(crate) fn acknowledge_events(
    project: &Project,
    ids: &[String],
    pane: &str,
    coordinator_attempt: u32,
) -> Result<()> {
    for item in unhandled(project)
        .into_iter()
        .filter(|item| ids.contains(&item.id) && !item.event.is_empty())
    {
        let event = crate::events::load(project, &item.event)?;
        let binding_matches = event.recipient.pane == pane
            && event.recipient.coordinator_attempt == coordinator_attempt;
        if binding_matches {
            crate::events::append_delivery(
                project,
                &event.id,
                crate::contracts::DeliveryState::Acknowledged,
            )?;
        }
    }
    Ok(())
}

/// An item id is also a file name, so it is checked before any path is built.
fn validate_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok || id.contains("..") {
        bail!("`{id}` is not an inbox item id");
    }
    Ok(())
}

/// Handles event-linked items only when the caller is their bound coordinator.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct DoneOutcome {
    pub(crate) moved: Vec<String>,
    pub(crate) missing: Vec<String>,
}

impl DoneOutcome {
    pub(crate) fn message(&self) -> String {
        format!("{} item(s) moved to inbox/done\n", self.moved.len())
    }

    pub(crate) fn warnings(&self) -> String {
        self.missing
            .iter()
            .map(|id| format!("no unhandled item `{id}`\n"))
            .collect()
    }
}

/// Handle all unhandled items of a kind using the same binding checks as id-based handling.
pub(crate) fn done_kind_bound(
    project: &Project,
    kind: &str,
    binding: Option<(&str, u32)>,
) -> Result<DoneOutcome> {
    let ids = unhandled(project)
        .into_iter()
        .filter(|item| item.kind == kind)
        .map(|item| item.id)
        .collect::<Vec<_>>();
    done_bound(project, &ids, false, binding)
}

pub(crate) fn done_bound(
    project: &Project,
    ids: &[String],
    all: bool,
    binding: Option<(&str, u32)>,
) -> Result<DoneOutcome> {
    let ids: Vec<String> = if all {
        unhandled(project).into_iter().map(|i| i.id).collect()
    } else {
        ids.to_vec()
    };
    for id in &ids {
        validate_id(id)?;
    }
    let _lock = project.lock()?;
    let dir = inbox_dir(project);
    let current = project
        .coordinator()
        .map(|record| (record.pane_id.clone(), record.attempt()));
    // Every item is checked before any moves: a refusal moves nothing.
    let mut checked = Vec::new();
    let mut missing = Vec::new();
    for id in &ids {
        let from = dir.join(format!("{id}.md"));
        if !from.is_file() {
            missing.push(id.clone());
            continue;
        }
        let item = std::fs::read_to_string(&from)
            .ok()
            .and_then(|text| parse(&text));
        if let Some(item) = &item
            && !item.event.is_empty()
        {
            let event = crate::events::load(project, &item.event)?;
            let Some((pane, attempt)) = binding else {
                bail!("coordinator_binding_required: event item `{id}` needs its coordinator");
            };
            let binding_matches = event.recipient.pane == pane
                && event.recipient.coordinator_attempt == attempt
                || current
                    .as_ref()
                    .is_some_and(|(p, a)| p == pane && *a == attempt);
            if !binding_matches {
                bail!("coordinator_binding_mismatch: event item `{id}` belongs to another binding");
            }
        }
        if item.is_some() {
            checked.push((from, id, item));
        }
    }
    let mut moved = Vec::new();
    if !checked.is_empty() && !dir.join("done").is_dir() {
        std::fs::create_dir(dir.join("done"))?;
    }
    for (from, id, item) in checked {
        std::fs::rename(&from, dir.join("done").join(format!("{id}.md")))?;
        if let Some(item) = item
            && !item.event.is_empty()
        {
            crate::events::append_delivery_locked(
                project,
                &item.event,
                crate::contracts::DeliveryState::Handled,
            )?;
        }
        moved.push(id.clone());
    }
    Ok(DoneOutcome { moved, missing })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_item(project: &Project, id: &str, body: &str) {
        std::fs::create_dir_all(inbox_dir(project)).unwrap();
        let text = format!(
            "+++\nid = \"{id}\"\nkind = \"note\"\nsubject = \"r\"\ncreated = \"2026-09-17T00:00:00Z\"\nsummary = \"s\"\n+++\n{body}"
        );
        std::fs::write(inbox_dir(project).join(format!("{id}.md")), text).unwrap();
    }

    #[test]
    fn lists_marks_seen_and_moves_to_done() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        write_item(&project, "20260917T000002Z-note-r-2", "\nbody text\n");
        write_item(&project, "20260917T000001Z-note-r-1", "");
        let items = unhandled(&project);
        assert_eq!(items.len(), 2);
        assert!(items[0].id.ends_with("-1"));
        assert_eq!(items[1].body, "body text");

        mark_seen(&project, &[items[0].id.clone()]).unwrap();
        assert_eq!(seen(&project).len(), 1);

        assert_eq!(
            done_bound(&project, &[items[0].id.clone()], false, None)
                .unwrap()
                .moved,
            [items[0].id.clone()]
        );
        assert_eq!(unhandled(&project).len(), 1);
        assert!(
            inbox_dir(&project)
                .join("done")
                .join(format!("{}.md", items[0].id))
                .is_file()
        );
        assert_eq!(
            done_bound(&project, &[], true, None).unwrap().moved.len(),
            1
        );
        assert!(unhandled(&project).is_empty());
    }

    #[test]
    fn lists_event_items_by_creation_time_not_id_prefix() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        std::fs::create_dir_all(inbox_dir(&project)).unwrap();
        for (id, created) in [
            ("event-t-0001-1-1", "2026-09-17T00:00:00Z"),
            ("20260917T000100Z-note-r-1", "2026-09-17T00:01:00Z"),
        ] {
            let text = format!(
                "+++\nid = \"{id}\"\nkind = \"note\"\ncreated = \"{created}\"\nsummary = \"due\"\n+++\n"
            );
            std::fs::write(inbox_dir(&project).join(format!("{id}.md")), text).unwrap();
        }
        assert_eq!(unhandled(&project)[0].id, "event-t-0001-1-1");
    }

    #[test]
    fn done_kind_only_moves_matching_items() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let first = write(&project, "note", "first", "due", "").unwrap();
        write(&project, "outage", "box", "offline", "").unwrap();
        let second = write(&project, "note", "second", "due", "").unwrap();
        assert_eq!(
            done_kind_bound(&project, "note", None).unwrap().moved,
            [first, second]
        );
        assert_eq!(unhandled(&project).len(), 1);
        assert_eq!(unhandled(&project)[0].kind, "outage");
    }

    #[test]
    fn two_events_in_one_tick_get_two_items() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let a = write(&project, "note", "nightly", "first", "").unwrap();
        let b = write(&project, "note", "nightly", "second\nline", "").unwrap();
        assert_ne!(a, b);
        assert!(a.ends_with("-note-nightly-1"), "{a}");
        assert!(b.ends_with("-note-nightly-2"), "{b}");
        let items = unhandled(&project);
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].summary, "second line");
        assert!(items.iter().all(|i| i.body.is_empty()));
        // A written item can be marked done by its id.
        assert_eq!(
            done_bound(&project, &[a], false, None).unwrap().moved.len(),
            1
        );
    }

    #[test]
    fn removed_kinds_are_dropped_on_read_not_migrated() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        std::fs::create_dir(inbox_dir(&project)).unwrap();
        for kind in [
            "thread-state",
            "report-available",
            "preparation-abandoned",
            "round-advance",
            "merge-diverged",
            "merge-pending",
            "pr",
            "lineage-mismatch",
            "done",
            "waiting",
        ] {
            let path = inbox_dir(&project).join(format!("{kind}.md"));
            let text = format!("+++\nkind = \"{kind}\"\nid = \"{kind}\"\n+++\n");
            std::fs::write(&path, &text).unwrap();
            assert!(write(&project, kind, "x", "stale", "").is_err());
            assert!(unhandled(&project).is_empty());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
        write(&project, "note", "nightly", "due", "work").unwrap();
        assert_eq!(unhandled(&project).len(), 1);
        assert_eq!(
            done_bound(&project, &[], true, None).unwrap().moved.len(),
            1
        );
        assert!(unhandled(&project).is_empty());
    }

    #[test]
    fn hostile_ids_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        for bad in ["../PROJECT", "a/b", "", ".hidden", "x..y"] {
            assert!(
                done_bound(&project, &[bad.to_string()], false, None).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn event_receipts_require_the_bound_coordinator() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let event = crate::contracts::Event {
            id: "t-0001-1-1".into(),
            op: "t-0001-1-1".into(),
            thread: "t-0001".into(),
            attempt: 1,
            round: None,
            recipient: crate::contracts::Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 2,
            },
            created: project::now(),
            payload: crate::contracts::EventPayload {
                done: None,
                waiting: Some(crate::contracts::WaitingPayload {
                    text: "wait".into(),
                    ..Default::default()
                }),
                failed: None,
            },
        };
        crate::events::seal_create_if_absent(&project, &event).unwrap();
        let item = write_event(&project, &event, "courier-delivery", "lane waits").unwrap();

        // This is the `--peek` behavior: merely listing/showing writes no fact.
        assert!(
            crate::events::states(&project, &event.id)
                .unwrap()
                .is_empty()
        );
        acknowledge_events(&project, std::slice::from_ref(&item), "w1:p2", 2).unwrap();
        acknowledge_events(&project, std::slice::from_ref(&item), "w1:p1", 1).unwrap();
        assert!(
            crate::events::states(&project, &event.id)
                .unwrap()
                .is_empty()
        );
        acknowledge_events(&project, std::slice::from_ref(&item), "w1:p1", 2).unwrap();
        assert_eq!(
            crate::events::states(&project, &event.id).unwrap(),
            vec![crate::contracts::DeliveryState::Acknowledged]
        );
        assert!(done_bound(&project, std::slice::from_ref(&item), false, None).is_err());
        assert_eq!(
            done_bound(&project, &[item], false, Some(("w1:p1", 2)))
                .unwrap()
                .moved
                .len(),
            1
        );
        assert_eq!(
            crate::events::states(&project, &event.id).unwrap(),
            vec![
                crate::contracts::DeliveryState::Acknowledged,
                crate::contracts::DeliveryState::Handled
            ]
        );

        // A2 review M2: after a replacement the new coordinator handles the
        // old binding's item; a refusal moves nothing.
        let old = crate::contracts::Event {
            id: "t-0001-1-2".into(),
            op: "t-0001-1-2".into(),
            ..event.clone()
        };
        crate::events::seal_create_if_absent(&project, &old).unwrap();
        let old_item = write_event(&project, &old, "courier-delivery", "lane waits").unwrap();
        write_item(&project, "note-1", "body");
        project
            .update_coordinator(|c| {
                c.pane_id = "w1:p3".into();
                c.generation = 3;
            })
            .unwrap();
        let both = ["note-1".to_string(), old_item.clone()];
        assert!(done_bound(&project, &both, false, Some(("w1:p9", 1))).is_err());
        assert_eq!(unhandled(&project).len(), 2);
        assert_eq!(
            done_bound(&project, &both, false, Some(("w1:p3", 3)))
                .unwrap()
                .moved
                .len(),
            2
        );
    }
}
