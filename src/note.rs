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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Retirement {
    pub(crate) id: String,
    pub(crate) at: String,
    pub(crate) request: String,
    pub(crate) reason: String,
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

/// Serializes the check-and-append boundary shared by notes and
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
    for retirement in retirements(project) {
        rows.push(Row {
            id: format!("retired:{}", retirement.id),
            kind: "retirement".into(),
            at: Some(retirement.at),
            request: Some(retirement.request),
            text: retirement.reason,
            replaces: Some(retirement.id),
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

fn retirements(project: &Project) -> Vec<Retirement> {
    std::fs::read_to_string(project.record_file("retirements.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

pub(crate) fn validate_basis(project: &Project, text: &str) -> Result<String> {
    use crate::contracts::AuthorityRef;
    use anyhow::Context;
    let reference = AuthorityRef::parse(text).context("invalid authority reference")?;
    match &reference {
        AuthorityRef::Request(_) => {
            return Ok(crate::prompt::resolve_request(project, text)?.basis());
        }
        AuthorityRef::Ask { id, revision } => {
            if crate::ask::latest_revision(project, id) != *revision {
                bail!("ask revision is not current");
            }
            let answer =
                crate::ask::answer_of(project, id, *revision).context("ask is not answered")?;
            if answer.not_understood || answer.choice == 0 {
                bail!("a no answer authorizes nothing");
            }
        }
    }
    Ok(reference.as_str())
}

/// Retire a current memory or instruction without inventing a replacement fact.
pub(crate) fn retire(
    project: &Project,
    id: &str,
    request: &str,
    reason: &str,
) -> Result<Retirement> {
    if reason.trim().is_empty() {
        return Err(crate::refusal::error(
            "note_retirement: a reason is required",
            format!(
                "ha note retire {} {id} --request {request} --reason \"<reason>\"",
                project.slug
            ),
        ));
    }
    let reference = if request.starts_with("request:") {
        request.to_string()
    } else {
        format!("request:{request}")
    };
    let request = validate_basis(project, &reference)?;
    let _replacement_lock = replacement_lock(project)?;
    let _lock = project.lock()?;
    if !read(project).iter().any(|note| note.id == id) {
        return Err(crate::refusal::error(
            format!("note_retirement: no note `{id}` exists"),
            format!(
                "ha note add {} \"<note>\" --kind memory --request {request}",
                project.slug
            ),
        ));
    }
    if replacement_map(&rows(project)).contains_key(id) {
        return Err(crate::refusal::error(
            format!("note_retirement: `{id}` is already replaced or retired"),
            format!("ha context {}", project.slug),
        ));
    }
    let record = Retirement {
        id: id.to_string(),
        at: project::now(),
        request: request.trim_start_matches("request:").to_string(),
        reason: reason.trim().to_string(),
    };
    let path = project.record_file_for_write("retirements.jsonl")?;
    let mut file = File::options().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&record)?)?;
    file.sync_all()?;
    drop(file);
    drop(_lock);
    crate::project::refresh_page(project)?;
    Ok(record)
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
        return Err(crate::refusal::error(
            "note_text: a note is required",
            format!(
                "ha note add {} \"<note>\" --kind {} --request {request}",
                project.slug,
                match kind {
                    Kind::Memory => "memory",
                    Kind::Instruction => "instruction",
                }
            ),
        ));
    }
    let reference = if request.starts_with("request:") {
        request.to_string()
    } else {
        format!("request:{request}")
    };
    let request = validate_basis(project, &reference)?;
    let request = request
        .strip_prefix("request:")
        .expect("validated request basis");
    for task in &tasks {
        crate::task::load(project, task)?;
    }
    let _replacement_lock = replaces.map(|_| replacement_lock(project)).transpose()?;
    if let Some(old) = replaces {
        if !target_exists(project, old) {
            return Err(crate::refusal::error(
                format!("note_replacement: no note `{old}` exists"),
                format!("ha context {}", project.slug),
            ));
        }
        let rows = rows(project);
        if replacement_map(&rows).contains_key(old) {
            return Err(crate::refusal::error(
                format!("note_replacement: `{old}` already has a replacement"),
                format!("ha context {}", project.slug),
            ));
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
        .filter(|row| row.kind != "retirement" && !replaced.contains(&row.id))
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
    use crate::testkit::fixture;

    #[test]
    fn retirement_keeps_history_but_removes_current_fact_and_instruction() {
        let fx = fixture();
        crate::prompt::record_test_request(&fx.project, "q-1", "These are no longer true.")
            .unwrap();
        for kind in [Kind::Memory, Kind::Instruction] {
            let note = add(
                &fx.project,
                kind,
                "The old fact is true.",
                "q-1",
                None,
                vec![],
            )
            .unwrap();
            let retired = retire(&fx.project, &note.id, "q-1", "It is now false.").unwrap();
            assert_eq!(retired.id, note.id);
            assert!(read(&fx.project).iter().any(|old| old.id == note.id));
            assert!(
                !active_for(&fx.project, None)
                    .iter()
                    .any(|row| row.id == note.id)
            );
            assert!(retire(&fx.project, &note.id, "q-1", "Again.").is_err());
        }
        let page = std::fs::read_to_string(fx.project.project_md()).unwrap();
        assert!(!page.contains("The old fact is true."));
        assert_eq!(retirements(&fx.project).len(), 2);
    }

    #[test]
    fn explicit_replacement_hides_old_note_from_active_rows() {
        let fx = fixture();
        crate::prompt::record_test_request(&fx.project, "q-1", "Use the newer instruction.")
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
