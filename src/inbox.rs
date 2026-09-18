//! Inbox items: events the ticker leaves for the coordinator.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::project::{self, Project};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub subject: String,
    pub created: String,
    pub summary: String,
    /// Sealed event id when this item is a delivery projection.
    pub event: String,
    /// Empty except for `routine` items.
    #[serde(skip)]
    pub body: String,
}

fn inbox_dir(project: &Project) -> PathBuf {
    project.dir().join("inbox")
}

fn parse(text: &str) -> Option<Item> {
    let rest = text.strip_prefix("+++\n")?;
    let (front, body) = rest
        .split_once("\n+++\n")
        .or_else(|| Some((rest.strip_suffix("\n+++")?, "")))?;
    let mut item: Item = toml::from_str(front).ok()?;
    item.body = body.trim_matches('\n').to_string();
    Some(item)
}

/// File-name-safe form of a subject (a thread id, routine name, machine label).
pub fn safe_subject(subject: &str) -> String {
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
/// tick never share a name. `body` is empty except for `routine` items.
pub fn write(
    project: &Project,
    kind: &str,
    subject: &str,
    summary: &str,
    body: &str,
) -> Result<String> {
    let _lock = project.lock()?;
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
pub fn write_event(
    project: &Project,
    event: &crate::contracts::Event,
    kind: &str,
    summary: &str,
) -> Result<String> {
    let _lock = project.lock()?;
    let id = if kind == "recipient-changed" {
        format!("recipient-changed-{}", event.id)
    } else {
        format!("event-{}", event.id)
    };
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
pub fn prune_done(project: &Project, days: u64) {
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

/// Unhandled items, oldest first (ids start with a UTC timestamp).
pub fn unhandled(project: &Project) -> Vec<Item> {
    let Ok(entries) = std::fs::read_dir(inbox_dir(project)) else {
        return Vec::new();
    };
    let mut items: Vec<Item> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".md"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|text| parse(&text))
        .collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    items
}

pub fn seen(project: &Project) -> BTreeSet<String> {
    project::read_json(&project.state_dir().join("inbox-seen.json")).unwrap_or_default()
}

/// Records that `context` showed these items, so they are nudged once only.
pub fn mark_seen(project: &Project, ids: &[String]) -> Result<()> {
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
pub fn acknowledge_events(
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
        let binding_matches = if item.kind == "recipient-changed" {
            project.coordinator().is_some_and(|record| {
                record.pane_id == pane && record.attempt() == coordinator_attempt
            })
        } else {
            event.recipient.pane == pane
                && event.recipient.coordinator_attempt == coordinator_attempt
        };
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
pub fn done_bound(
    project: &Project,
    ids: &[String],
    all: bool,
    binding: Option<(&str, u32)>,
) -> Result<usize> {
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
    for id in &ids {
        let from = dir.join(format!("{id}.md"));
        if !from.is_file() {
            eprintln!("no unhandled item `{id}`");
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
            let is_current = current
                .as_ref()
                .is_some_and(|(p, a)| p == pane && *a == attempt);
            let own =
                event.recipient.pane == pane && event.recipient.coordinator_attempt == attempt;
            // The current coordinator also handles items of a binding it
            // replaced; nobody else can.
            let binding_matches = if item.kind == "recipient-changed" {
                is_current
            } else {
                own || is_current
            };
            if !binding_matches {
                bail!("coordinator_binding_mismatch: event item `{id}` belongs to another binding");
            }
        }
        checked.push((from, id, item));
    }
    let mut moved = 0;
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
        moved += 1;
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_item(project: &Project, id: &str, body: &str) {
        let text = format!(
            "+++\nid = \"{id}\"\nkind = \"routine\"\nsubject = \"r\"\ncreated = \"2026-09-17T00:00:00Z\"\nsummary = \"s\"\n+++\n{body}"
        );
        std::fs::write(inbox_dir(project).join(format!("{id}.md")), text).unwrap();
    }

    #[test]
    fn lists_marks_seen_and_moves_to_done() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        write_item(&project, "20260917T000002Z-routine-r-2", "\nbody text\n");
        write_item(&project, "20260917T000001Z-routine-r-1", "");
        let items = unhandled(&project);
        assert_eq!(items.len(), 2);
        assert!(items[0].id.ends_with("-1"));
        assert_eq!(items[1].body, "body text");

        mark_seen(&project, &[items[0].id.clone()]).unwrap();
        assert_eq!(seen(&project).len(), 1);

        assert_eq!(
            done_bound(&project, &[items[0].id.clone()], false, None).unwrap(),
            1
        );
        assert_eq!(unhandled(&project).len(), 1);
        assert!(
            inbox_dir(&project)
                .join("done")
                .join(format!("{}.md", items[0].id))
                .is_file()
        );
        assert_eq!(done_bound(&project, &[], true, None).unwrap(), 1);
        assert!(unhandled(&project).is_empty());
    }

    #[test]
    fn two_events_in_one_tick_get_two_items() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let a = write(&project, "thread-state", "t-0001", "first", "").unwrap();
        let b = write(&project, "thread-state", "t-0001", "second\nline", "").unwrap();
        assert_ne!(a, b);
        assert!(a.ends_with("-thread-state-t-0001-1"), "{a}");
        assert!(b.ends_with("-thread-state-t-0001-2"), "{b}");
        let items = unhandled(&project);
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].summary, "second line");
        assert!(items.iter().all(|i| i.body.is_empty()));
        // A written item can be marked done by its id.
        assert_eq!(done_bound(&project, &[a], false, None).unwrap(), 1);
    }

    #[test]
    fn routine_items_carry_a_body_and_subjects_are_made_file_safe() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let id = write(&project, "outage", "Elias MacBook/../x", "down", "").unwrap();
        assert!(id.contains("-outage-elias-macbook----x-"), "{id}");
        write(
            &project,
            "routine",
            "nightly",
            "due",
            "Check the build.\n\n```\nout\n```",
        )
        .unwrap();
        let routine = unhandled(&project)
            .into_iter()
            .find(|i| i.kind == "routine")
            .unwrap();
        assert!(routine.body.starts_with("Check the build."));
        assert!(routine.body.ends_with("```"));
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
                }),
            },
        };
        crate::events::seal_create_if_absent(&project, &event).unwrap();
        let item = write_event(&project, &event, "waiting", "lane waits").unwrap();

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
            done_bound(&project, &[item], false, Some(("w1:p1", 2))).unwrap(),
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
        let old_item = write_event(&project, &old, "waiting", "lane waits").unwrap();
        write_item(&project, "routine-1", "body");
        project
            .update_coordinator(|c| {
                c.pane_id = "w1:p3".into();
                c.generation = 3;
            })
            .unwrap();
        let both = ["routine-1".to_string(), old_item.clone()];
        assert!(done_bound(&project, &both, false, Some(("w1:p9", 1))).is_err());
        assert_eq!(unhandled(&project).len(), 2);
        assert_eq!(
            done_bound(&project, &both, false, Some(("w1:p3", 3))).unwrap(),
            2
        );
    }

    #[test]
    fn replacement_coordinator_acknowledges_recipient_changed_item() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        project
            .update_coordinator(|record| {
                record.pane_id = "w2:p1".into();
                record.generation = 3;
            })
            .unwrap();
        let event = crate::contracts::Event {
            id: "t-0001-1-1".into(),
            op: "t-0001-1-1".into(),
            thread: "t-0001".into(),
            attempt: 1,
            round: None,
            recipient: crate::contracts::Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 1,
            },
            created: project::now(),
            payload: crate::contracts::EventPayload {
                done: None,
                waiting: Some(crate::contracts::WaitingPayload {
                    text: "wait".into(),
                }),
            },
        };
        crate::events::seal_create_if_absent(&project, &event).unwrap();
        let item = write_event(
            &project,
            &event,
            "recipient-changed",
            "an earlier event needs review",
        )
        .unwrap();
        acknowledge_events(&project, &[item], "w2:p1", 3).unwrap();
        assert_eq!(
            crate::events::states(&project, &event.id).unwrap(),
            vec![crate::contracts::DeliveryState::Acknowledged]
        );
    }
}
