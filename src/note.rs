//! Provenanced project memory and standing instructions.
//!
//! New notes are append-only records. Historical task notes still load from
//! task records; current facts and instructions live only in this log.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::project::{self, Project};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub(crate) enum Kind {
    Memory,
    Instruction,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Instruction => "standing instruction",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Note {
    pub(crate) schema: u32,
    pub(crate) id: String,
    pub(crate) kind: Kind,
    pub(crate) at: String,
    pub(crate) request: String,
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) replaces: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tasks: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) at: Option<String>,
    pub(crate) request: Option<String>,
    pub(crate) text: String,
    pub(crate) replaces: Option<String>,
    pub(crate) tasks: Vec<String>,
}

fn path(project: &Project) -> PathBuf {
    project.record_file("notes.jsonl")
}

pub(crate) struct ReplacementLock {
    _file: File,
}

/// Serializes the check-and-append boundary shared by notes, decisions and
/// tasks. Their own storage locks cannot prevent two different record kinds
/// from replacing the same current row at once.
pub(crate) fn replacement_lock(project: &Project) -> Result<ReplacementLock> {
    let path = project.state_dir().join("replacements.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    file.lock()?;
    Ok(ReplacementLock { _file: file })
}

pub(crate) fn read(project: &Project) -> Vec<Note> {
    std::fs::read_to_string(path(project))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

pub(crate) fn rows(project: &Project) -> Vec<Row> {
    let mut rows: Vec<Row> = read(project)
        .into_iter()
        .map(|note| Row {
            id: note.id,
            kind: note.kind.word().into(),
            at: Some(note.at),
            request: Some(note.request),
            text: note.text,
            replaces: note.replaces,
            tasks: note.tasks,
        })
        .collect();
    for decision in crate::decide::read(project).records {
        let request = decision.request.clone().or_else(|| {
            decision
                .basis
                .as_deref()
                .and_then(|basis| basis.strip_prefix("request:"))
                .map(str::to_string)
        });
        rows.push(Row {
            id: decision.id,
            kind: "decision".into(),
            at: Some(decision.at),
            request,
            text: decision.line,
            replaces: decision.replaces,
            tasks: Vec::new(),
        });
    }
    let tasks = crate::task::list_with_errors(project).0;
    let replaced_tasks: BTreeSet<String> = tasks
        .iter()
        .filter_map(|task| task.replaces.clone())
        .collect();
    for task in tasks {
        if task.replaces.is_some() || replaced_tasks.contains(&task.id) {
            let request = task
                .authority
                .iter()
                .find_map(|authority| authority.strip_prefix("request:").map(str::to_string));
            rows.push(Row {
                id: task.id.clone(),
                kind: "task".into(),
                at: Some(task.created.clone()),
                request,
                text: task.title.clone(),
                replaces: task.replaces.clone(),
                tasks: vec![task.id.clone()],
            });
        }
        for (index, note) in task.notes.into_iter().enumerate() {
            let id = if note.id.is_empty() {
                format!("undated:{}:note-{:04}", task.id, index + 1)
            } else {
                note.id
            };
            let dated = !note.request.is_empty();
            rows.push(Row {
                id,
                kind: "task note".into(),
                at: dated.then_some(note.at),
                request: dated.then_some(note.request),
                text: note.text,
                replaces: note.replaces,
                tasks: vec![task.id.clone()],
            });
        }
    }
    rows
}

pub(crate) fn replacement_map(rows: &[Row]) -> BTreeMap<String, String> {
    rows.iter()
        .filter_map(|row| {
            row.replaces
                .as_ref()
                .map(|old| (old.clone(), row.id.clone()))
        })
        .collect()
}

pub(crate) fn target_exists(project: &Project, id: &str) -> bool {
    rows(project).iter().any(|row| row.id == id)
        || (id.starts_with("job-") && crate::task::load(project, id).is_ok())
}

pub(crate) fn add(
    project: &Project,
    kind: Kind,
    text: &str,
    request: &str,
    replaces: Option<&str>,
    tasks: Vec<String>,
) -> Result<Note> {
    if text.trim().is_empty() {
        return Err(crate::refusal::error("note_text: a note is required"));
    }
    let request = request.strip_prefix("request:").unwrap_or(request);
    crate::decide::validate_basis(project, &format!("request:{request}"))?;
    for task in &tasks {
        crate::task::load(project, task)?;
    }
    let _replacement_lock = replaces.map(|_| replacement_lock(project)).transpose()?;
    if let Some(old) = replaces {
        if !target_exists(project, old) {
            bail!("note_replacement: no note `{old}` exists");
        }
        let rows = rows(project);
        if replacement_map(&rows).contains_key(old) {
            bail!("note_replacement: `{old}` already has a replacement");
        }
    }
    let _lock = project.lock()?;
    let records = read(project);
    let next = records
        .iter()
        .filter_map(|record| record.id.strip_prefix("n-")?.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let note = Note {
        schema: 1,
        id: format!("n-{next:04}"),
        kind,
        at: project::now(),
        request: request.into(),
        text: text.trim().into(),
        replaces: replaces.map(str::to_string),
        tasks,
    };
    let path = project.record_file_for_write("notes.jsonl")?;
    let mut file = File::options().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&note)?)?;
    file.sync_all()?;
    drop(file);
    drop(_lock);
    crate::project::refresh_page(project)?;
    Ok(note)
}

pub(crate) fn sort_newest_first(rows: &mut Vec<Row>) {
    rows.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
    // Rounded timestamps can tie. Explicit replacement is stronger evidence
    // of order than an id or a guessed prose subject.
    let replacements = replacement_map(rows);
    for _ in 0..rows.len() {
        let positions: BTreeMap<String, usize> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| (row.id.clone(), index))
            .collect();
        let Some((old_index, new_index)) = rows.iter().enumerate().find_map(|(old_index, row)| {
            let new_index = *positions.get(replacements.get(&row.id)?)?;
            (new_index > old_index).then_some((old_index, new_index))
        }) else {
            break;
        };
        let newer = rows.remove(new_index);
        rows.insert(old_index, newer);
    }
}

pub(crate) fn active_rows(project: &Project) -> Vec<Row> {
    let rows = rows(project);
    let replaced: BTreeSet<String> = replacement_map(&rows).into_keys().collect();
    rows.into_iter()
        .filter(|row| !replaced.contains(&row.id))
        .collect()
}

pub(crate) fn active_for(project: &Project, task: Option<&str>) -> Vec<Row> {
    active_rows(project)
        .into_iter()
        .filter(|row| {
            row.tasks.is_empty() || task.is_some_and(|id| row.tasks.iter().any(|t| t == id))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::testkit::fixture;

    #[test]
    fn a_note_replacement_removes_a_stale_decision_from_current_views() {
        let fx = fixture();
        crate::talk::append(
            &fx.project,
            None,
            crate::talk::Entry::Rolf {
                request: "q-1".into(),
                text: "Use the newer instruction.".into(),
                answer: None,
            },
        )
        .unwrap();
        let decision = crate::decide::decide(
            &fx.world.ctx(),
            "demo",
            crate::decide::NewDecision {
                line: "I kept the old instruction.",
                class: "routine",
                key: None,
                basis: None,
                replaces: None,
                request: None,
            },
        )
        .unwrap();
        add(
            &fx.project,
            Kind::Instruction,
            "Use the newer instruction.",
            "q-1",
            Some(&decision.id),
            vec![],
        )
        .unwrap();

        assert!(crate::decide::current(&fx.project).is_empty());
        let overview = crate::talk::overview::Overview::load(
            &fx.project,
            &crate::talk::Journal::default(),
            &crate::talk::view::Conversation::default(),
            &crate::talk::overview::Live::default(),
        );
        assert!(
            overview
                .sections
                .iter()
                .flatten()
                .all(|row| !row.full_text().contains("I kept the old instruction."))
        );
        let error = crate::decide::decide(
            &fx.world.ctx(),
            "demo",
            crate::decide::NewDecision {
                line: "I chose another replacement.",
                class: "routine",
                key: None,
                basis: None,
                replaces: Some(&decision.id),
                request: Some("q-1"),
            },
        )
        .unwrap_err();
        assert!(error.to_string().starts_with("decision_replaced"));
    }

    #[test]
    fn explicit_replacement_hides_old_note_from_active_rows() {
        let fx = fixture();
        crate::talk::append(
            &fx.project,
            None,
            crate::talk::Entry::Rolf {
                request: "q-1".into(),
                text: "Use the newer instruction.".into(),
                answer: None,
            },
        )
        .unwrap();
        let old = add(&fx.project, Kind::Memory, "Keep this.", "q-1", None, vec![]).unwrap();
        let new = add(
            &fx.project,
            Kind::Memory,
            "Keep that instead.",
            "q-1",
            Some(&old.id),
            vec![],
        )
        .unwrap();
        let active = active_for(&fx.project, None);
        assert!(!active.iter().any(|row| row.id == old.id));
        assert!(active.iter().any(|row| row.id == new.id));
        let page = std::fs::read_to_string(fx.project.project_md()).unwrap();
        assert!(page.contains("Keep that instead."));
        assert!(!page.contains("Keep this."));

        let task = crate::task::add(
            &fx.project,
            "Ship the checked change.",
            vec!["request:q-1".into()],
            vec!["The command reports the new result.".into()],
            None,
            Some(new.id.clone()),
        )
        .unwrap();
        assert!(
            !active_for(&fx.project, None)
                .iter()
                .any(|row| row.id == new.id)
        );
        let page = std::fs::read_to_string(fx.project.project_md()).unwrap();
        assert!(!page.contains("Keep that instead."));
        assert!(page.contains(&task.title));
    }
}
