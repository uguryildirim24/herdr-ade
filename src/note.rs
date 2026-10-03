//! Provenanced project memory and standing instructions.
//!
//! New notes are immutable atomic records. Historical task notes and JSONL
//! logs still load; current facts and instructions use one file per record.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
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

/// Serializes the check-and-append boundary shared by notes and
/// tasks. Their own storage locks cannot prevent two different record kinds
/// from replacing the same current row at once.
pub(crate) fn replacement_lock(project: &Project) -> Result<File> {
    project::lock_file(&project.state_dir().join("replacements.lock"))
}

fn records<T: serde::de::DeserializeOwned>(project: &Project, kind: &str) -> Result<Vec<T>> {
    let (mut rows, _) = project::read_jsonl(&project.record_file(&format!("{kind}.jsonl")))?;
    let dir = project.record_dir(kind);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(rows),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", dir.display()));
        }
    };
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    for path in paths {
        if path.extension().is_some_and(|ext| ext == "json") {
            rows.push(
                serde_json::from_slice(&std::fs::read(&path)?)
                    .with_context(|| format!("corrupt record: {}", path.display()))?,
            );
        }
    }
    Ok(rows)
}

pub(crate) fn read(project: &Project) -> Result<Vec<Note>> {
    records(project, "notes")
}

/// Reserve even the id of a historical interrupted append. If the write died
/// before its id, reserve one slot above the last complete historical record.
fn interrupted_id(project: &Project) -> Result<u64> {
    let (rows, tail) = project::read_jsonl::<Note>(&path(project))?;
    let Some(tail) = tail else { return Ok(0) };
    let last = rows
        .iter()
        .filter_map(|row| note_number(&row.id))
        .max()
        .unwrap_or(0);
    let text = String::from_utf8_lossy(&tail);
    let recorded = text
        .split_once("\"id\"")
        .and_then(|(_, rest)| rest.split_once(':'))
        .and_then(|(_, rest)| rest.trim_start().strip_prefix("\"n-"))
        .and_then(|rest| rest.split('"').next())
        .and_then(|number| number.parse::<u64>().ok())
        .unwrap_or(0);
    Ok(recorded.max(last + 1))
}

fn note_number(id: &str) -> Option<u64> {
    id.strip_prefix("n-")?.parse().ok()
}

pub(crate) fn rows(project: &Project) -> Vec<Row> {
    rows_checked(project).unwrap_or_else(|error| {
        // Unknown retirement history cannot make any old instruction active.
        eprintln!("{error:#}");
        Vec::new()
    })
}

fn rows_checked(project: &Project) -> Result<Vec<Row>> {
    let mut rows: Vec<Row> = read(project)?
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
    for retirement in retirements(project)? {
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
    Ok(rows)
}

fn retirements(project: &Project) -> Result<Vec<Retirement>> {
    records(project, "retirements")
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
            let answer = crate::ask::answered_revision(project, id, *revision)?;
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
    let notes = records::<Note>(project, "notes")?;
    records::<Retirement>(project, "retirements")?;
    if !notes.iter().any(|note| note.id == id) {
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
    let path = project
        .record_dir_for_write("retirements")?
        .join(format!("{id}.json"));
    project::write_create_only(&path, &serde_json::to_vec(&record)?)?;
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
    let _lock = project.lock()?;
    let notes = records::<Note>(project, "notes")?;
    records::<Retirement>(project, "retirements")?;
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
    let next = notes
        .iter()
        .filter_map(|record| note_number(&record.id))
        .chain([interrupted_id(project)?])
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
    let path = project
        .record_dir_for_write("notes")?
        .join(format!("{}.json", note.id));
    project::write_create_only(&path, &serde_json::to_vec(&note)?)?;
    drop(_lock);
    crate::project::refresh_page(project)?;
    Ok(note)
}

pub(crate) fn sort_newest_first(rows: &mut [Row]) {
    rows.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
}

pub(crate) fn active_rows(project: &Project) -> Vec<Row> {
    let rows = rows(project);
    let replaced: BTreeSet<String> = replacement_map(&rows).into_keys().collect();
    rows.into_iter()
        .filter(|row| row.kind != "retirement" && !replaced.contains(&row.id))
        .collect()
}

impl Row {
    /// Scope is explicit authority, not a relevance judgment about the text.
    pub(crate) fn applies_to(&self, task: Option<&str>) -> bool {
        self.tasks.is_empty() || task.is_some_and(|id| self.tasks.iter().any(|t| t == id))
    }
}

pub(crate) fn active_for(project: &Project, task: Option<&str>) -> Vec<Row> {
    active_rows(project)
        .into_iter()
        .filter(|row| row.applies_to(task))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::fixture;

    #[test]
    fn explicit_scope_preserves_global_instructions_and_historical_records() {
        let fx = fixture();
        crate::prompt::record_test_request(&fx.project, "q-scope", "Keep task scopes explicit.")
            .unwrap();
        let tasks: Vec<_> = ["Research", "Build"]
            .into_iter()
            .map(|title| {
                crate::task::add(
                    &fx.project,
                    title,
                    vec!["request:q-scope".into()],
                    vec!["Retain required evidence.".into()],
                    None,
                    None,
                )
                .unwrap()
            })
            .collect();
        // A real pre-scope JSONL shape still loads; no new required field.
        std::fs::write(path(&fx.project),
            "{\"schema\":1,\"id\":\"n-0001\",\"kind\":\"instruction\",\"at\":\"2026-09-22\",\"request\":\"q-scope\",\"text\":\"Never copy credentials.\"}\n").unwrap();
        let shared = add(
            &fx.project,
            Kind::Instruction,
            "Do not call the Agent tool.",
            "q-scope",
            None,
            tasks.iter().map(|t| t.id.clone()).collect(),
        )
        .unwrap();
        let local = add(
            &fx.project,
            Kind::Memory,
            "Research-only source evidence.",
            "q-scope",
            None,
            vec![tasks[0].id.clone()],
        )
        .unwrap();
        let retired = add(
            &fx.project,
            Kind::Instruction,
            "Obsolete rule.",
            "q-scope",
            None,
            vec![],
        )
        .unwrap();
        retire(&fx.project, &retired.id, "q-scope", "No longer controls.").unwrap();
        for task in [None, Some(tasks[0].id.as_str()), Some(tasks[1].id.as_str())] {
            let rows = active_for(&fx.project, task);
            assert!(rows.iter().any(|row| row.id == "n-0001"));
            assert_eq!(rows.iter().any(|row| row.id == shared.id), task.is_some());
            assert_eq!(
                rows.iter().any(|row| row.id == local.id),
                task == Some(tasks[0].id.as_str())
            );
            assert!(!rows.iter().any(|row| row.id == retired.id));
        }
    }

    #[test]
    fn corrupt_retirement_never_reactivates_an_instruction() {
        for historical in [false, true] {
            let fx = fixture();
            crate::prompt::record_test_request(&fx.project, "q-1", "Retire the instruction.")
                .unwrap();
            let note = add(
                &fx.project,
                Kind::Instruction,
                "Obsolete instruction.",
                "q-1",
                None,
                vec![],
            )
            .unwrap();
            assert!(active_rows(&fx.project).iter().any(|row| row.id == note.id));
            let retirement = retire(&fx.project, &note.id, "q-1", "Obsolete.").unwrap();
            let mut path = fx
                .project
                .record_dir("retirements")
                .join(format!("{}.json", note.id));
            if historical {
                std::fs::remove_file(&path).unwrap();
                path = fx.project.record_file("retirements.jsonl");
                std::fs::write(
                    &path,
                    format!("{}\n", serde_json::to_string(&retirement).unwrap()),
                )
                .unwrap();
            }
            assert!(!active_rows(&fx.project).iter().any(|row| row.id == note.id));
            std::fs::write(&path, "{broken\n").unwrap();
            assert!(
                active_rows(&fx.project).is_empty(),
                "corruption is not absent retirement history"
            );
        }
    }

    #[test]
    fn ask_authority_validates_paths_and_requires_the_matching_revision() {
        let fx = fixture();
        let ask = crate::contracts::Ask {
            id: "a-1".into(),
            revision: 1,
            ..Default::default()
        };
        let answer = crate::ask::Answer {
            id: "a-1".into(),
            revision: 1,
            choice: 1,
            text: "yes".into(),
            not_understood: false,
            answered: project::now(),
            by: "Rolf".into(),
        };
        let dir = fx.project.record_dir_for_write("asks").unwrap().join("a-1");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("r1.toml"), toml::to_string(&ask).unwrap()).unwrap();
        std::fs::write(
            dir.join("r1.answer.toml"),
            toml::to_string(&answer).unwrap(),
        )
        .unwrap();
        assert_eq!(
            validate_basis(&fx.project, "ask:a-1@1").unwrap(),
            "ask:a-1@1"
        );
        // Even plausible records outside asks must not authorize a path escape.
        let escaped = fx.project.record_dir("escaped");
        std::fs::create_dir(&escaped).unwrap();
        std::fs::write(escaped.join("r1.toml"), toml::to_string(&ask).unwrap()).unwrap();
        std::fs::write(
            escaped.join("r1.answer.toml"),
            toml::to_string(&answer).unwrap(),
        )
        .unwrap();
        assert!(validate_basis(&fx.project, "ask:../escaped@1").is_err());
        for bad in ["", "a/b", "/tmp/a-1", "a-1..", "a-x"] {
            assert!(validate_basis(&fx.project, &format!("ask:{bad}@1")).is_err());
        }
        std::fs::write(dir.join("r1.toml"), "").unwrap();
        assert!(validate_basis(&fx.project, "ask:a-1@1").is_err());
        std::fs::write(dir.join("r1.toml"), toml::to_string(&ask).unwrap()).unwrap();
        std::fs::write(
            dir.join("r1.answer.toml"),
            toml::to_string(&crate::ask::Answer {
                revision: 2,
                ..answer
            })
            .unwrap(),
        )
        .unwrap();
        assert!(validate_basis(&fx.project, "ask:a-1@1").is_err());
    }

    #[test]
    fn interrupted_append_then_add_replace_and_retire_preserves_records_and_ids() {
        for (tail, expected) in [
            (r#"{"schema":1,"id":"n-0010","text":"torn"#, "n-0011"),
            (r#"{"schema":1"#, "n-0003"),
        ] {
            let fx = fixture();
            crate::prompt::record_test_request(&fx.project, "q-1", "Keep the corrected notes.")
                .unwrap();
            let historical = Note {
                schema: 1,
                id: "n-0001".into(),
                kind: Kind::Instruction,
                at: project::now(),
                request: "q-1".into(),
                text: "Historical instruction.".into(),
                replaces: None,
                tasks: vec![],
            };
            std::fs::write(
                path(&fx.project),
                format!("{}\n{tail}", serde_json::to_string(&historical).unwrap()),
            )
            .unwrap();
            let historical_retirement = Retirement {
                id: "n-0000".into(),
                at: project::now(),
                request: "q-1".into(),
                reason: "Historical retirement.".into(),
            };
            std::fs::write(
                fx.project.record_file("retirements.jsonl"),
                format!(
                    "{}\n{{\"id\":",
                    serde_json::to_string(&historical_retirement).unwrap()
                ),
            )
            .unwrap();
            let added = add(
                &fx.project,
                Kind::Instruction,
                "Added after crash.",
                "q-1",
                None,
                vec![],
            )
            .unwrap();
            assert_eq!(added.id, expected);
            let replaced = add(
                &fx.project,
                Kind::Instruction,
                "Replacement after crash.",
                "q-1",
                Some(&historical.id),
                vec![],
            )
            .unwrap();
            let retired = retire(&fx.project, &added.id, "q-1", "Retired after crash.").unwrap();
            let again = add(
                &fx.project,
                Kind::Memory,
                "Another note.",
                "q-1",
                None,
                vec![],
            )
            .unwrap();
            let notes = read(&fx.project).unwrap();
            assert_eq!(notes.len(), 4);
            assert_eq!(
                notes
                    .iter()
                    .map(|note| &note.id)
                    .collect::<BTreeSet<_>>()
                    .len(),
                4
            );
            for note in [&historical, &added, &replaced, &again] {
                assert!(notes.contains(note));
            }
            let retirements = retirements(&fx.project).unwrap();
            assert_eq!(retirements.len(), 2);
            assert!(retirements.iter().any(|row| row.id == retired.id));
            assert!(
                retirements
                    .iter()
                    .any(|row| row.reason == historical_retirement.reason)
            );
            let rows = rows(&fx.project);
            assert!(rows.iter().any(|row| row.text == "Retired after crash."));
            assert!(
                active_for(&fx.project, None)
                    .iter()
                    .any(|row| row.id == replaced.id)
            );
            assert!(
                !active_for(&fx.project, None)
                    .iter()
                    .any(|row| row.id == added.id)
            );
            // Current evidence is not silently skipped or overwritten.
            let bad = fx
                .project
                .record_dir("notes")
                .join(format!("{}.json", again.id));
            std::fs::write(&bad, b"{").unwrap();
            let error = add(
                &fx.project,
                Kind::Memory,
                "Must not hide corruption.",
                "q-1",
                None,
                vec![],
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(&bad.display().to_string()));
        }
    }
}
