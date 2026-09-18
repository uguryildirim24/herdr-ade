//! `ha ask`, `ha say` and the one typed publisher (SPEC-ADE D17 items 3 and
//! 4, item 35).
//!
//! `publish(HumanMessage)` is the only way text reaches Rolf's plane: the
//! board, the plugin's notifications and the talk surface. An `Ask` is
//! resolved to its stored record's exact question and ordered choices; a
//! `Notice` is one of the fixed texts below; an arbitrary string cannot be
//! published.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::{Ask, HumanMessage};
use crate::glossary::{self, format_check};
use crate::paths::Ctx;
use crate::plain;
use crate::project::{self, Project, write_atomic};

/// The standing extra choice every ask carries (D17 item 4).
pub const NOT_UNDERSTOOD: &str = "I did not understand the question";

/// Fixed notices: the only text a `Notice` can publish. Every text passes the
/// checker with an empty registry (tested).
pub const NOTICES: &[(&str, &str)] = &[
    (
        "plain_exhausted",
        "The coordinator could not say this plainly. Open its pane to read it.",
    ),
    (
        "talk_uncertain",
        "Your last message may not have arrived. The own pane of the coordinator shows whether it did.",
    ),
    (
        "needs_you_in_pane",
        "The coordinator needs you in its own pane.",
    ),
    (
        "ask_redrawn",
        "A new question came in, so your number was not used. Read the question again and answer it.",
    ),
    (
        "journal_tail",
        "An unfinished line was found at the end of this record and was skipped.",
    ),
    (
        "session_changed",
        "The coordinator started a new chat. Its replies show here again once it is opened again.",
    ),
    (
        "hook_failed",
        "A check of the last reply did not run. Open the pane of the coordinator to read it.",
    ),
    (
        "native_on",
        "You are typing in the pane of the coordinator now. Nothing there is checked.",
    ),
    ("native_off", "Your messages go through this tab again."),
    (
        "native_not_ready",
        "The coordinator is not ready yet, so your messages still wait.",
    ),
    (
        "request_waiting",
        "Your message waits until the coordinator is ready.",
    ),
    (
        "recipient_changed",
        "The coordinator changed, so your waiting message was not sent. Type it again if you still want it.",
    ),
    (
        "ask_not_found",
        "That question is closed or was asked again, so your number was not used.",
    ),
];

pub fn notice_text(id: &str) -> Option<&'static str> {
    NOTICES.iter().find(|(k, _)| *k == id).map(|(_, v)| *v)
}

/// An answer, stored next to the revision it answers; create-if-absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Answer {
    pub id: String,
    pub revision: u32,
    pub choice: u32,
    pub text: String,
    pub not_understood: bool,
    pub answered: String,
    pub by: String,
}

fn asks_dir(project: &Project) -> PathBuf {
    project.dir().join("asks")
}

fn ask_dir(project: &Project, id: &str) -> PathBuf {
    asks_dir(project).join(id)
}

fn rev_path(project: &Project, id: &str, revision: u32) -> PathBuf {
    ask_dir(project, id).join(format!("r{revision}.toml"))
}

fn answer_path(project: &Project, id: &str, revision: u32) -> PathBuf {
    ask_dir(project, id).join(format!("r{revision}.answer.toml"))
}

fn published_marker(project: &Project, id: &str, revision: u32) -> PathBuf {
    ask_dir(project, id).join(format!("r{revision}.published"))
}

fn validate_ask_id(id: &str) -> Result<()> {
    let digits = id.strip_prefix("a-").unwrap_or("");
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        bail!("ask_unknown: `{id}` is not an ask id (expected the form a-1)");
    }
    Ok(())
}

pub fn load_revision(project: &Project, id: &str, revision: u32) -> Result<Option<Ask>> {
    validate_ask_id(id)?;
    let path = rev_path(project, id, revision);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(toml::from_str(&text).with_context(|| {
            format!("ask record {} does not parse", path.display())
        })?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn latest_revision(project: &Project, id: &str) -> u32 {
    let Ok(entries) = std::fs::read_dir(ask_dir(project, id)) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| {
            n.strip_prefix('r')?
                .strip_suffix(".toml")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0)
}

pub fn latest(project: &Project, id: &str) -> Result<Option<Ask>> {
    match latest_revision(project, id) {
        0 => Ok(None),
        r => load_revision(project, id, r),
    }
}

pub fn answer_of(project: &Project, id: &str, revision: u32) -> Option<Answer> {
    let text = std::fs::read_to_string(answer_path(project, id, revision)).ok()?;
    toml::from_str(&text).ok()
}

/// Latest revisions without an answer, oldest first.
pub fn open_asks(project: &Project) -> Vec<Ask> {
    let Ok(entries) = std::fs::read_dir(asks_dir(project)) else {
        return Vec::new();
    };
    let mut asks: Vec<Ask> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|id| validate_ask_id(id).is_ok())
        .filter_map(|id| latest(project, &id).ok().flatten())
        .filter(|a| answer_of(project, &a.id, a.revision).is_none())
        .collect();
    asks.sort_by(|a, b| (&a.asked, &a.id).cmp(&(&b.asked, &b.id)));
    asks
}

pub fn newest_open(project: &Project) -> Option<Ask> {
    open_asks(project).pop()
}

/// The board's compact line: the leading clause of the question at a word
/// boundary plus ` (<n> choices)`, at most 60 characters, never the choices.
pub fn compact_line(ask: &Ask) -> String {
    let suffix = format!(" ({} choices)", ask.choices.len());
    let budget = 60usize.saturating_sub(suffix.len() + 1);
    let question = ask.question.trim().trim_end_matches('?');
    let clause = question
        .split([',', ';', ':'])
        .next()
        .unwrap_or(question)
        .trim();
    let mut out = String::new();
    for word in clause.split_whitespace() {
        let next = if out.is_empty() {
            word.to_string()
        } else {
            format!("{out} {word}")
        };
        if next.chars().count() > budget {
            break;
        }
        out = next;
    }
    if out.is_empty() {
        out = clause.chars().take(budget).collect();
    }
    format!("{out}?{suffix}")
}

/// The full question with numbered choices and the standing `0`.
pub fn numbered(ask: &Ask) -> String {
    let mut out = format!("{}\n", ask.question.trim());
    for (i, c) in ask.choices.iter().enumerate() {
        out.push_str(&format!("  {}. {}\n", i + 1, c));
    }
    out.push_str(&format!("  0. {NOT_UNDERSTOOD}\n"));
    out
}

pub struct NewAsk {
    pub question: String,
    pub choices: Vec<String>,
    pub what: Option<String>,
    pub means: Option<String>,
    pub round: Option<String>,
    /// Re-ask an existing id as revision `r+1`.
    pub reask: Option<String>,
}

fn check_structured(project: &Project, new: &NewAsk) -> Result<()> {
    if !(2..=4).contains(&new.choices.len()) {
        bail!(
            "ask_choice_count: an ask takes two to four choices, got {}",
            new.choices.len()
        );
    }
    let g = glossary::registry(project);
    let mut problems = Vec::new();
    let r = plain::check(&new.question, &g);
    if !r.passed() {
        problems.push(format!("question: {}", format_check(&new.question, &r)));
    }
    for (i, choice) in new.choices.iter().enumerate() {
        let r = plain::check(choice, &g);
        if !r.passed() {
            problems.push(format!("choice {}: {}", i + 1, format_check(choice, &r)));
        }
    }
    for v in plain::check_ask(&new.question, &new.choices, &g)
        .violations
        .iter()
        .filter(|v| v.rule == plain::Rule::QuestionForm)
    {
        let span = [&new.question]
            .into_iter()
            .chain(new.choices.iter())
            .find(|t| t.len() == v.span.end)
            .map(String::as_str)
            .unwrap_or("");
        problems.push(format!("{}: \"{span}\": {}", v.rule.code(), v.fix));
    }
    for (field, value) in [("what", &new.what), ("means", &new.means)] {
        if let Some(value) = value {
            problems.extend(say_problems(field, value, &g));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        bail!("plain_refused:\n{}", problems.join("\n"))
    }
}

/// R7 on an envelope field, then R1 to R5 on its text.
fn say_problems(field: &str, value: &str, g: &plain::Glossary) -> Vec<String> {
    if value.trim().is_empty() {
        return vec![format!(
            "plain_envelope: required envelope field {field} is missing or empty"
        )];
    }
    let r = plain::check(value, g);
    if r.passed() {
        Vec::new()
    } else {
        vec![format!("{field}: {}", format_check(value, &r))]
    }
}

/// `ha ask`: validate, write the immutable record, only then publish.
pub fn ask(ctx: &Ctx, slug: &str, new: NewAsk) -> Result<Ask> {
    let project = Project::load(&ctx.root, slug)?;
    check_structured(&project, &new)?;
    let binding = project.coordinator().map(|c| c.pane_id).unwrap_or_default();
    let record = {
        let _lock = project.lock()?;
        let (id, revision) = match &new.reask {
            Some(id) => {
                let r = latest_revision(&project, id);
                if r == 0 {
                    bail!("ask_unknown: `{id}` has never been asked");
                }
                (id.clone(), r + 1)
            }
            None => {
                let counter = project.state_dir().join("ask-counter.json");
                let n: u64 = project::read_json::<u64>(&counter).unwrap_or(0) + 1;
                project::write_json(&counter, &n)?;
                (format!("a-{n}"), 1)
            }
        };
        let record = Ask {
            id: id.clone(),
            revision,
            project: project.slug.clone(),
            round: new.round.clone(),
            question: new.question.trim().to_string(),
            choices: new.choices.iter().map(|c| c.trim().to_string()).collect(),
            what: new.what.clone(),
            means: new.means.clone(),
            asked: project::now(),
            coordinator_binding: binding,
        };
        std::fs::create_dir_all(ask_dir(&project, &id))?;
        let path = rev_path(&project, &id, revision);
        if path.exists() {
            bail!("ask_exists: {} already exists", path.display());
        }
        write_atomic(&path, toml::to_string(&record)?.as_bytes())?;
        record
    };
    let compact = compact_line(&record);
    glossary::gate(&project, &compact).context("the compact board line failed the check")?;
    if let Err(e) = publish(
        ctx,
        &project,
        &HumanMessage::Ask {
            id: record.id.clone(),
            revision: record.revision,
        },
    ) {
        eprintln!("note: the ask is recorded; publication will be retried by the ticker: {e:#}");
    }
    Ok(record)
}

/// `ha ask answer <id> --revision <r> <n>`.
pub fn answer(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    revision: u32,
    choice: u32,
    by: &str,
) -> Result<Answer> {
    let project = Project::load(&ctx.root, slug)?;
    let answer = {
        let _lock = project.lock()?;
        let latest = latest_revision(&project, id);
        if latest == 0 {
            bail!("ask_unknown: `{id}` has never been asked");
        }
        if revision != latest {
            bail!(
                "ask_revision_stale: `{id}` is at revision {latest}; revision {revision} is void"
            );
        }
        let record = load_revision(&project, id, revision)?.context("ask_unknown")?;
        if answer_of(&project, id, revision).is_some() {
            bail!("ask_closed: `{id}` revision {revision} is already answered");
        }
        let text = match choice {
            0 => NOT_UNDERSTOOD.to_string(),
            n => record
                .choices
                .get(n as usize - 1)
                .cloned()
                .with_context(|| {
                    format!(
                        "ask_choice_out_of_range: choose 0 to {}",
                        record.choices.len()
                    )
                })?,
        };
        let answer = Answer {
            id: id.to_string(),
            revision,
            choice,
            text,
            not_understood: choice == 0,
            answered: project::now(),
            by: by.to_string(),
        };
        write_atomic(
            &answer_path(&project, id, revision),
            toml::to_string(&answer)?.as_bytes(),
        )?;
        if choice == 0 {
            let path = project.state_dir().join("not-understood.json");
            let n: u64 = project::read_json::<u64>(&path).unwrap_or(0) + 1;
            project::write_json(&path, &n)?;
        }
        answer
    };
    let _ = crate::talk::append(
        &project,
        Some(&format!("answer:{id}@{revision}")),
        crate::talk::Entry::Answer {
            id: id.to_string(),
            revision,
            choice,
        },
    );
    let _ = crate::board::refresh(ctx, &project);
    Ok(answer)
}

/// How many `0 = I did not understand` answers the project has seen; a
/// signal for Rolf, never a gate.
pub fn not_understood_count(project: &Project) -> u64 {
    project::read_json::<u64>(&project.state_dir().join("not-understood.json")).unwrap_or(0)
}

/// `ha say --what ... [--means ...]`.
pub fn say(ctx: &Ctx, slug: &str, what: &str, means: Option<&str>) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    publish(
        ctx,
        &project,
        &HumanMessage::Say {
            what: what.trim().to_string(),
            means: means.map(|m| m.trim().to_string()),
        },
    )
    .map(|_| ())
}

/// The last line of `what` that fits a board token: cut at a word boundary.
pub fn board_line(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for word in text.split_whitespace() {
        let next = if out.is_empty() {
            word.to_string()
        } else {
            format!("{out} {word}")
        };
        if next.chars().count() + 3 > max {
            break;
        }
        out = next;
    }
    format!("{out}...")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// `None` when the journal already had this message (a duplicate).
    pub seq: Option<u64>,
    pub board: bool,
    pub notified: bool,
}

pub fn publish(ctx: &Ctx, project: &Project, msg: &HumanMessage) -> Result<Published> {
    publish_keyed(ctx, project, msg, None)
}

/// The one publisher. `key` makes a publication idempotent (a duplicate hook
/// run of the same reply appends once).
pub fn publish_keyed(
    ctx: &Ctx,
    project: &Project,
    msg: &HumanMessage,
    key: Option<&str>,
) -> Result<Published> {
    let g = glossary::registry(project);
    let mut problems = Vec::new();
    match msg {
        HumanMessage::Say { what, means } => {
            problems.extend(say_problems("what", what, &g));
            if let Some(means) = means {
                problems.extend(say_problems("means", means, &g));
            }
        }
        other => {
            for v in plain::check_message(other, &g).violations {
                problems.push(format!("{}: {}", v.rule.code(), v.fix));
            }
        }
    }
    if !problems.is_empty() {
        bail!("plain_refused:\n{}", problems.join("\n"));
    }
    match msg {
        HumanMessage::Say { what, means } => {
            let line = board_line(what, 80);
            glossary::gate(project, &line).context("the board line failed the check")?;
            let seq = crate::talk::append(
                project,
                key,
                crate::talk::Entry::Say {
                    what: what.clone(),
                    means: means.clone(),
                },
            )?;
            if seq.is_none() {
                return Ok(Published {
                    seq,
                    board: false,
                    notified: false,
                });
            }
            let board = crate::board::publish_value(ctx, project, "ade_last", &line).is_ok();
            crate::board::remember_last(project, &line);
            Ok(Published {
                seq,
                board,
                notified: false,
            })
        }
        HumanMessage::Ask { id, revision } => {
            let latest = latest_revision(project, id);
            let record = load_revision(project, id, *revision)?.with_context(|| {
                format!("ask_unknown: `{id}` revision {revision} does not exist")
            })?;
            if *revision != latest {
                bail!("ask_revision_stale: `{id}` is at revision {latest}");
            }
            if answer_of(project, id, *revision).is_some() {
                bail!("ask_closed: `{id}` revision {revision} is answered");
            }
            let checked = plain::check_ask(&record.question, &record.choices, &g);
            if !checked.passed() {
                bail!("plain_refused: the stored question no longer passes the check");
            }
            let compact = compact_line(&record);
            glossary::gate(project, &compact)?;
            let seq = crate::talk::append(
                project,
                Some(
                    &key.map(str::to_string)
                        .unwrap_or(format!("ask:{id}@{revision}")),
                ),
                crate::talk::Entry::Ask {
                    id: id.clone(),
                    revision: *revision,
                },
            )?;
            let board =
                crate::board::publish_value(ctx, project, "ade_needs_you", &compact).is_ok();
            let mut body = String::new();
            for (i, c) in record.choices.iter().enumerate() {
                body.push_str(&format!("{}. {}\n", i + 1, c));
            }
            body.push_str(&format!("0. {NOT_UNDERSTOOD}"));
            let notified = notify(ctx, project, &compact, &body);
            if board && notified {
                let _ = std::fs::write(published_marker(project, id, *revision), "");
            }
            Ok(Published {
                seq,
                board,
                notified,
            })
        }
        HumanMessage::Notice { id } => {
            let text = notice_text(id)
                .with_context(|| format!("notice_unknown: `{id}` is not a fixed notice"))?;
            glossary::gate(project, text)?;
            let seq =
                crate::talk::append(project, key, crate::talk::Entry::Notice { id: id.clone() })?;
            let mut board = false;
            if id == "plain_exhausted" {
                let line = board_line(text, 80);
                board = crate::board::publish_value(ctx, project, "ade_last", &line).is_ok();
                crate::board::remember_last(project, &line);
            }
            Ok(Published {
                seq,
                board,
                notified: false,
            })
        }
    }
}

/// A plugin notification whose title and body are checked texts.
fn notify(ctx: &Ctx, project: &Project, title: &str, body: &str) -> bool {
    if glossary::gate(project, title).is_err() || glossary::gate(project, body).is_err() {
        return false;
    }
    let Some(coord) = project.coordinator().filter(|c| !c.socket.is_empty()) else {
        return false;
    };
    let herdr = crate::herdr::Herdr::new(ctx.env.herdr_bin(), &coord.socket, ctx.runner);
    herdr
        .call(
            &["notification", "show", title, "--body", body],
            Duration::from_secs(10),
        )
        .is_ok()
}

/// Resumes publication of every open latest revision that has no
/// `published` marker (a crash between record and publication).
pub fn tick(ctx: &Ctx, project: &Project) -> Result<()> {
    for a in open_asks(project) {
        if published_marker(project, &a.id, a.revision).exists() {
            continue;
        }
        let _ = publish(
            ctx,
            project,
            &HumanMessage::Ask {
                id: a.id.clone(),
                revision: a.revision,
            },
        );
    }
    Ok(())
}
