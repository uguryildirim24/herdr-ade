//! `ha ask`, `ha say` and the one typed publisher (SPEC-ADE D17 items 3 and
//! 4, item 35).
//!
//! `publish(HumanMessage)` is the only way text reaches Rolf's plane: the
//! board, the plugin's notifications and the journal. An `Ask` is
//! resolved to its stored record's exact question and ordered choices; a
//! `Notice` is one of the fixed texts below; an arbitrary string cannot be
//! published.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contracts::{Ask, HumanMessage};
use crate::paths::Ctx;
use crate::project::{self, Project, write_atomic};

/// The standing extra choice every ask carries (D17 item 4).
pub(crate) const NOT_UNDERSTOOD: &str = "I did not understand the question";

/// Fixed notices: the only text a `Notice` can publish.
const NOTICES: &[(&str, &str)] = &[
    (
        "journal_tail",
        "A half written line was found at the end of this record and was left out.",
    ),
    (
        "session_changed",
        "The coordinator started a new chat. Open its pane to read its replies.",
    ),
    (
        "hook_failed",
        "A check of the last reply did not run. Open the pane of the coordinator to read it.",
    ),
];

pub(crate) fn notice_text(id: &str) -> Option<&'static str> {
    NOTICES.iter().find(|(k, _)| *k == id).map(|(_, v)| *v)
}

/// An answer, stored next to the revision it answers; create-if-absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) id: String,
    pub(crate) revision: u32,
    pub(crate) choice: u32,
    pub(crate) text: String,
    pub(crate) not_understood: bool,
    pub(crate) answered: String,
    pub(crate) by: String,
}

fn asks_dir(project: &Project) -> PathBuf {
    project.record_dir("asks")
}

fn ask_dir(project: &Project, id: &str) -> PathBuf {
    asks_dir(project).join(id)
}

/// The shared ask-set lock, `<project>/.state/asks/.open.lock`. Creation, re-asking
/// and answering take it inside the project lock so concurrent writers cannot
/// allocate the same id.
struct AskSetLock {
    _file: std::fs::File,
}

fn ask_set_lock(project: &Project) -> Result<AskSetLock> {
    let dir = project.record_dir_for_write("asks")?;
    let file = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".open.lock"))?;
    file.lock()?;
    Ok(AskSetLock { _file: file })
}

fn rev_path(project: &Project, id: &str, revision: u32) -> PathBuf {
    ask_dir(project, id).join(format!("r{revision}.toml"))
}

fn answer_path(project: &Project, id: &str, revision: u32) -> PathBuf {
    ask_dir(project, id).join(format!("r{revision}.answer.toml"))
}

fn publications_dir(project: &Project) -> PathBuf {
    project.state_dir().join("publications")
}

fn publication_path(project: &Project, key: &str) -> PathBuf {
    publications_dir(project).join(format!("{:x}.json", Sha256::digest(key.as_bytes())))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Publication {
    key: String,
    message: HumanMessage,
    journal: bool,
    board: bool,
    /// `None` when this message has no notification sink.
    notified: Option<bool>,
}

struct PublicationLock {
    _file: std::fs::File,
}

fn publication_lock(project: &Project) -> Result<PublicationLock> {
    std::fs::create_dir_all(publications_dir(project))?;
    let file = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(publications_dir(project).join(".lock"))?;
    file.lock()?;
    Ok(PublicationLock { _file: file })
}

fn validate_ask_id(id: &str) -> Result<()> {
    let digits = id.strip_prefix("a-").unwrap_or("");
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        bail!("ask_unknown: `{id}` is not an ask id (expected the form a-1)");
    }
    Ok(())
}

pub(crate) fn load_revision(project: &Project, id: &str, revision: u32) -> Result<Option<Ask>> {
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

pub(crate) fn latest_revision(project: &Project, id: &str) -> u32 {
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

pub(crate) fn latest(project: &Project, id: &str) -> Result<Option<Ask>> {
    match latest_revision(project, id) {
        0 => Ok(None),
        r => load_revision(project, id, r),
    }
}

pub(crate) fn answer_of(project: &Project, id: &str, revision: u32) -> Option<Answer> {
    let text = std::fs::read_to_string(answer_path(project, id, revision)).ok()?;
    toml::from_str(&text).ok()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Withdrawal {
    pub(crate) id: String,
    pub(crate) revision: u32,
    pub(crate) reason: String,
    pub(crate) by: String,
    pub(crate) at: String,
}

fn withdrawal_path(project: &Project, id: &str, revision: u32) -> PathBuf {
    ask_dir(project, id).join(format!("r{revision}.withdrawn.toml"))
}

#[cfg(test)]
pub(crate) fn withdrawal_of(project: &Project, id: &str, revision: u32) -> Option<Withdrawal> {
    let text = std::fs::read_to_string(withdrawal_path(project, id, revision)).ok()?;
    toml::from_str(&text).ok()
}

fn is_withdrawn(project: &Project, id: &str, revision: u32) -> bool {
    withdrawal_path(project, id, revision).exists()
}

pub(crate) fn withdraw(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    reason: &str,
    by: &str,
) -> Result<Withdrawal> {
    validate_ask_id(id)?;
    if reason.trim().is_empty() || by.trim().is_empty() {
        bail!("ask_withdraw: a reason and actor are required");
    }
    let project = Project::load(&ctx.root, slug)?;
    let record = {
        let _lock = project.lock()?;
        let _set = ask_set_lock(&project)?;
        let ask = latest(&project, id)?
            .with_context(|| format!("ask_unknown: `{id}` has never been asked"))?;
        if answer_of(&project, id, ask.revision).is_some() {
            bail!("ask_closed: `{id}` is already answered; an answered ask cannot be withdrawn");
        }
        if is_withdrawn(&project, id, ask.revision) {
            bail!("ask_withdrawn: `{id}` is already withdrawn");
        }
        let record = Withdrawal {
            id: id.to_string(),
            revision: ask.revision,
            reason: reason.trim().to_string(),
            by: by.to_string(),
            at: project::now(),
        };
        write_atomic(
            &withdrawal_path(&project, id, ask.revision),
            toml::to_string(&record)?.as_bytes(),
        )?;
        record
    };
    let _ = crate::board::refresh(ctx, &project);
    Ok(record)
}

/// Latest revisions without an answer or withdrawal, oldest first.
pub(crate) fn open_asks(project: &Project) -> Vec<Ask> {
    let Ok(entries) = std::fs::read_dir(asks_dir(project)) else {
        return Vec::new();
    };
    let mut asks: Vec<Ask> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|id| validate_ask_id(id).is_ok())
        .filter_map(|id| latest(project, &id).ok().flatten())
        .filter(|a| answer_of(project, &a.id, a.revision).is_none())
        .filter(|a| !is_withdrawn(project, &a.id, a.revision))
        .collect();
    asks.sort_by(|a, b| (&a.asked, &a.id).cmp(&(&b.asked, &b.id)));
    asks
}

pub(crate) fn newest_open(project: &Project) -> Option<Ask> {
    open_asks(project).pop()
}

/// Question identity ignores presentation: case, spacing and punctuation.
fn normalized_question(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Every stored revision, including answered history, oldest id first.
fn revisions(project: &Project) -> Vec<(Ask, Option<Answer>)> {
    let Ok(entries) = std::fs::read_dir(asks_dir(project)) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|id| validate_ask_id(id).is_ok())
        .collect();
    ids.sort_by_key(|id| {
        id.strip_prefix("a-")
            .and_then(|digits| digits.parse::<u64>().ok())
            .unwrap_or(u64::MAX)
    });
    let mut out = Vec::new();
    for id in ids {
        for revision in 1..=latest_revision(project, &id) {
            if let Ok(Some(ask)) = load_revision(project, &id, revision) {
                out.push((ask, answer_of(project, &id, revision)));
            }
        }
    }
    out
}

fn refuse_repeated_question(project: &Project, new: &NewAsk) -> Result<()> {
    let wanted = normalized_question(&new.question);
    if let Some((ask, answer)) = revisions(project).into_iter().find(|(ask, answer)| {
        let still_open = answer.is_none()
            && ask.revision == latest_revision(project, &ask.id)
            && !is_withdrawn(project, &ask.id, ask.revision);
        (answer.is_some() || still_open) && normalized_question(&ask.question) == wanted
    }) {
        let answer = answer
            .map(|answer| answer.text)
            .unwrap_or_else(|| "still open".to_string());
        bail!(
            "ask_duplicate: `{}` already asks this question; answer: {}",
            ask.id,
            answer
        );
    }
    Ok(())
}

/// The board's compact line: the leading clause of the question at a word
/// boundary plus ` (<n> choices)`, at most 60 characters, never the choices.
pub(crate) fn compact_line(ask: &Ask) -> String {
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
pub(crate) fn numbered(ask: &Ask) -> String {
    let mut out = format!("{}\n", ask.question.trim());
    for (i, c) in ask.choices.iter().enumerate() {
        out.push_str(&format!("  {}. {}\n", i + 1, c));
    }
    out.push_str(&format!("  0. {NOT_UNDERSTOOD}\n"));
    out
}

pub(crate) struct NewAsk {
    pub(crate) question: String,
    pub(crate) choices: Vec<String>,
    pub(crate) what: Option<String>,
    pub(crate) means: Option<String>,
    pub(crate) round: Option<String>,
    /// Re-ask an existing id as revision `r+1`.
    pub(crate) reask: Option<String>,
}

/// `ha ask`: validate, write the immutable record, only then publish.
pub(crate) fn ask(ctx: &Ctx, slug: &str, new: NewAsk) -> Result<Ask> {
    let project = Project::load(&ctx.root, slug)?;
    if !(2..=4).contains(&new.choices.len()) {
        bail!("ask_choice_count: an ask takes two to four choices");
    }
    let binding = project.coordinator().map(|c| c.pane_id).unwrap_or_default();
    let record = {
        let _lock = project.lock()?;
        let _set = ask_set_lock(&project)?;
        refuse_repeated_question(&project, &new)?;
        let (id, revision) = match &new.reask {
            Some(id) => {
                validate_ask_id(id)?;
                let r = latest_revision(&project, id);
                if r == 0 {
                    bail!("ask_unknown: `{id}` has never been asked");
                }
                if is_withdrawn(&project, id, r) {
                    bail!("ask_withdrawn: `{id}` is withdrawn; it cannot be asked again");
                }
                if answer_of(&project, id, r).is_some() {
                    bail!("ask_closed: `{id}` revision {r} is already answered");
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
    let message = HumanMessage::Ask {
        id: record.id.clone(),
        revision: record.revision,
    };
    match publish(ctx, &project, &message) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("note: the ask is recorded; publication will be retried by the ticker: {e:#}")
        }
    }
    Ok(record)
}

/// Accept the exact displayed sentence as well as its number.
pub(crate) fn answer_text(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    revision: u32,
    choice: &str,
    by: &str,
) -> Result<Answer> {
    let n = if let Ok(n) = choice.parse::<u32>() {
        n
    } else if choice == NOT_UNDERSTOOD {
        0
    } else {
        let project = Project::load(&ctx.root, slug)?;
        let record = load_revision(&project, id, revision)?.context("ask_unknown")?;
        record
            .choices
            .iter()
            .position(|text| text == choice)
            .map(|index| index as u32 + 1)
            .with_context(|| {
                format!("ask_choice_unknown: `{choice}` is not an exact choice sentence")
            })?
    };
    answer(ctx, slug, id, revision, n, by)
}

/// `ha ask answer <id> --revision <r> <number-or-exact-sentence>`.
pub(crate) fn answer(
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
        let _set = ask_set_lock(&project)?;
        validate_ask_id(id)?;
        let latest = latest_revision(&project, id);
        if is_withdrawn(&project, id, latest) {
            bail!("ask_withdrawn: `{id}` is withdrawn; it cannot be answered");
        }
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
pub(crate) fn not_understood_count(project: &Project) -> u64 {
    project::read_json::<u64>(&project.state_dir().join("not-understood.json")).unwrap_or(0)
}

fn next_say_id(project: &Project) -> Result<String> {
    let _lock = project.lock()?;
    let path = project.state_dir().join("say-counter.json");
    let next = project::read_json::<u64>(&path).unwrap_or(0) + 1;
    project::write_json(&path, &next)?;
    Ok(format!("s-{next}"))
}

/// `ha say --what ... [--means ...]`.
pub(crate) fn say(ctx: &Ctx, slug: &str, what: &str, means: Option<&str>) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let id = next_say_id(&project)?;
    publish(
        ctx,
        &project,
        &HumanMessage::Say {
            id: id.clone(),
            what: what.trim().to_string(),
            means: means.map(|m| m.trim().to_string()),
            landed_round: None,
        },
    )?;
    Ok(id)
}

/// The landing line for a merged round (SPEC-talk §6.1). It publishes under
/// `landed:<round>` so a repeated merge publishes once, and it validates that
/// the round actually merged and checkpointed.
pub(crate) fn say_landed(
    ctx: &Ctx,
    slug: &str,
    what: &str,
    means: Option<&str>,
    round: &str,
) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let id = format!("landed-{round}");
    publish(
        ctx,
        &project,
        &HumanMessage::Say {
            id: id.clone(),
            what: what.trim().to_string(),
            means: means.map(|m| m.trim().to_string()),
            landed_round: Some(round.to_string()),
        },
    )?;
    Ok(id)
}

/// The last line of `what` that fits a board token: cut at a word boundary.
fn board_line(text: &str, max: usize) -> String {
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
pub(crate) struct Published {
    /// `None` when the journal already had this message (a duplicate).
    pub(crate) seq: Option<u64>,
    pub(crate) board: bool,
    pub(crate) notified: bool,
}

/// Resolves one stored ask revision for publication: known, latest,
/// unanswered, not withdrawn, and still passing the check.
pub(crate) fn open_revision(project: &Project, id: &str, revision: u32) -> Result<Ask> {
    let latest = latest_revision(project, id);
    let record = load_revision(project, id, revision)?
        .with_context(|| format!("ask_unknown: `{id}` revision {revision} does not exist"))?;
    if revision != latest {
        bail!("ask_revision_stale: `{id}` is at revision {latest}");
    }
    if is_withdrawn(project, id, revision) {
        bail!("ask_withdrawn: `{id}` revision {revision} is withdrawn");
    }
    if answer_of(project, id, revision).is_some() {
        bail!("ask_closed: `{id}` revision {revision} is answered");
    }
    Ok(record)
}

fn publication_key(message: &HumanMessage) -> Result<String> {
    match message {
        HumanMessage::Say { id, .. } if !id.trim().is_empty() => Ok(format!("say:{id}")),
        HumanMessage::Ask { id, revision } => Ok(format!("ask:{id}@{revision}")),
        HumanMessage::Say { .. } => bail!("say_id_missing: a say needs an id"),
        HumanMessage::Notice { .. } => bail!("notice_key_missing: notices use their own publisher"),
    }
}

fn save_publication(project: &Project, publication: &Publication) -> Result<()> {
    project::write_json(&publication_path(project, &publication.key), publication)
}

fn publication_complete(publication: &Publication) -> bool {
    publication.journal && publication.board && publication.notified.unwrap_or(true)
}

fn journal_key(message: &HumanMessage, publication_key: &str) -> String {
    match message {
        HumanMessage::Say {
            landed_round: Some(round),
            ..
        } => format!("landed:{round}"),
        _ => publication_key.to_string(),
    }
}

/// Publishes an authored say or ask under its canonical id. Each sink outcome
/// is durable, so a retry runs only the sinks that have not yet succeeded.
pub(crate) fn publish(ctx: &Ctx, project: &Project, msg: &HumanMessage) -> Result<Published> {
    if let HumanMessage::Notice { id } = msg {
        return publish_notice_keyed(ctx, project, id, None);
    }
    let ask_record = match msg {
        HumanMessage::Ask { id, revision } => Some(open_revision(project, id, *revision)?),
        HumanMessage::Say { landed_round, .. } => {
            if let Some(round) = landed_round {
                let merged = crate::round::read_merge(project, round)?
                    .is_some_and(|merge| merge.phase == crate::contracts::MergePhase::Checkpointed);
                if !merged {
                    bail!("landed_round_unmerged: `{round}` has not merged and checkpointed");
                }
            }
            None
        }
        HumanMessage::Notice { .. } => unreachable!(),
    };
    let board_value = match (msg, ask_record.as_ref()) {
        (HumanMessage::Say { what, .. }, _) => board_line(what, 80),
        (HumanMessage::Ask { .. }, Some(record)) => compact_line(record),
        _ => unreachable!(),
    };

    // Ask closure and all publication retries serialize with one another.
    let _ask_set = matches!(msg, HumanMessage::Ask { .. })
        .then(|| ask_set_lock(project))
        .transpose()?;
    if let HumanMessage::Ask { id, revision } = msg {
        // Recheck after taking the ask-set lock so answer or withdrawal cannot
        // race the board and notification sinks.
        open_revision(project, id, *revision)?;
    }
    let _publication = publication_lock(project)?;
    let key = publication_key(msg)?;
    let journal_key = journal_key(msg, &key);
    let path = publication_path(project, &key);
    // Check the last authored line, not the board's shortened token. Keep
    // retrying the same publication id possible after a partial sink failure.
    let journal = crate::talk::read(project);
    if let HumanMessage::Say { what, means, .. } = msg
        && !journal
            .lines
            .iter()
            .any(|line| line.key.as_deref() == Some(&journal_key))
        && let Some((previous_what, previous_means)) =
            journal
                .lines
                .iter()
                .rev()
                .find_map(|line| match &line.entry {
                    crate::talk::Entry::Say { what, means, .. } => Some((what, means)),
                    _ => None,
                })
        && previous_what == what
        && previous_means == means
    {
        return Err(crate::refusal::error(
            "say_repeated: your previous board line already says this; write a new line only when something changed",
        ));
    }
    let mut state = project::read_json::<Publication>(&path).unwrap_or(Publication {
        key: key.clone(),
        message: msg.clone(),
        journal: false,
        board: false,
        notified: matches!(msg, HumanMessage::Ask { .. }).then_some(false),
    });
    if state.key != key || state.message != *msg {
        bail!("publication_collision: `{key}` already names another message");
    }
    if !path.exists() {
        save_publication(project, &state)?;
    }
    if state.journal
        && !crate::talk::read(project)
            .lines
            .iter()
            .any(|line| line.key.as_deref() == Some(&journal_key))
    {
        state.journal = false;
        save_publication(project, &state)?;
    }

    let mut seq = None;
    if !state.journal {
        let entry = match msg {
            HumanMessage::Say {
                what,
                means,
                landed_round,
                ..
            } => crate::talk::Entry::Say {
                what: what.clone(),
                means: means.clone(),
                landed_round: landed_round.clone(),
            },
            HumanMessage::Ask { id, revision } => crate::talk::Entry::Ask {
                id: id.clone(),
                revision: *revision,
            },
            HumanMessage::Notice { .. } => unreachable!(),
        };
        seq = crate::talk::append(project, Some(&journal_key), entry)?;
        state.journal = true;
        save_publication(project, &state)?;
    }
    let board_key = if matches!(msg, HumanMessage::Ask { .. }) {
        "ade_needs_you"
    } else {
        "ade_last"
    };
    if !state.board && crate::board::publish_value(ctx, project, board_key, &board_value).is_ok() {
        state.board = true;
        if matches!(msg, HumanMessage::Say { .. }) {
            crate::board::remember_last(project, &board_value);
        }
        save_publication(project, &state)?;
    }
    if state.notified == Some(false) {
        let record = ask_record.as_ref().expect("ask publication has a record");
        let mut body = String::new();
        for (index, choice) in record.choices.iter().enumerate() {
            body.push_str(&format!("{}. {}\n", index + 1, choice));
        }
        if notify(ctx, project, &board_value, &body) {
            state.notified = Some(true);
            save_publication(project, &state)?;
        }
    }
    Ok(Published {
        seq,
        board: state.board,
        notified: state.notified.unwrap_or(false),
    })
}

pub(crate) fn publish_notice_keyed(
    _ctx: &Ctx,
    project: &Project,
    id: &str,
    key: Option<&str>,
) -> Result<Published> {
    notice_text(id).with_context(|| format!("notice_unknown: `{id}` is not a fixed notice"))?;
    let seq = crate::talk::append(
        project,
        key,
        crate::talk::Entry::Notice { id: id.to_string() },
    )?;
    Ok(Published {
        seq,
        board: false,
        notified: false,
    })
}

/// A plugin notification with the full question and its choices.
fn notify(ctx: &Ctx, project: &Project, title: &str, body: &str) -> bool {
    let body = format!("{body}0. {NOT_UNDERSTOOD}");
    let Some(coord) = project.coordinator().filter(|c| !c.socket.is_empty()) else {
        return false;
    };
    let herdr = crate::herdr::Herdr::new(ctx.env.herdr_bin(), &coord.socket, ctx.runner);
    herdr
        .call(
            &["notification", "show", title, "--body", &body],
            Duration::from_secs(10),
        )
        .is_ok()
}

/// Resumes each incomplete sink. Open asks are also visited so a crash after
/// writing the ask record but before writing its publication record is safe.
pub(crate) fn tick(ctx: &Ctx, project: &Project) -> Result<()> {
    for ask in open_asks(project) {
        let _ = publish(
            ctx,
            project,
            &HumanMessage::Ask {
                id: ask.id.clone(),
                revision: ask.revision,
            },
        );
    }
    let Ok(entries) = std::fs::read_dir(publications_dir(project)) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let Some(state) = project::read_json::<Publication>(&entry.path()) else {
            continue;
        };
        if !publication_complete(&state) {
            let _ = publish(ctx, project, &state.message);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::testkit::{Fx, fixture};
    use crate::runner::fake::ok;
    use crate::scenarios::World;

    fn keep_or_stop() -> NewAsk {
        NewAsk {
            question: "keep the experiment running another hour, or stop now?".into(),
            choices: vec!["keep it running another hour".into(), "stop it now".into()],
            what: None,
            means: None,
            round: None,
            reask: None,
        }
    }

    fn journal_kinds(project: &Project) -> Vec<String> {
        crate::talk::read(project)
            .lines
            .iter()
            .map(|l| match &l.entry {
                crate::talk::Entry::Say { .. } => "say".to_string(),
                crate::talk::Entry::Ask { id, revision } => format!("ask {id}@{revision}"),
                crate::talk::Entry::Notice { id } => format!("notice {id}"),
                crate::talk::Entry::Answer { id, choice, .. } => format!("answer {id} {choice}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn answer_accepts_exact_sentence_or_number() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let first = ask(&ctx, "demo", keep_or_stop()).unwrap();
        let answer = answer_text(
            &ctx,
            "demo",
            &first.id,
            first.revision,
            &first.choices[1],
            "test",
        )
        .unwrap();
        assert_eq!(
            (answer.choice, answer.text.as_str()),
            (2, first.choices[1].as_str())
        );
        assert!(
            answer_text(
                &ctx,
                "demo",
                &first.id,
                first.revision,
                "not a choice",
                "test"
            )
            .is_err()
        );
        let second = ask(
            &ctx,
            "demo",
            NewAsk {
                question: "should I leave the screen open or close it now?".into(),
                choices: vec![
                    "leave the screen open".into(),
                    "close the screen now".into(),
                ],
                ..keep_or_stop()
            },
        )
        .unwrap();
        assert_eq!(
            answer_text(&ctx, "demo", &second.id, second.revision, "1", "test")
                .unwrap()
                .choice,
            1
        );
    }

    #[test]
    fn one_ask_revision_is_published_once() {
        let fx = fixture();
        let a = ask(&fx.world.ctx(), "demo", keep_or_stop()).unwrap();
        assert_eq!((a.id.as_str(), a.revision), ("a-1", 1));
        let compact = compact_line(&a);
        assert_eq!(
            compact,
            "keep the experiment running another hour? (2 choices)"
        );
        assert!(compact.chars().count() <= 60);
        assert!(
            fx.world
                .runner
                .count(&format!("--token ade_needs_you={compact}"))
                == 1
        );
        assert!(
            fx.world.runner.count(&format!(
                "notification show {compact} --body 1. keep it running another hour"
            )) == 1
        );
        assert_eq!(journal_kinds(&fx.project), ["ask a-1@1"]);
        assert!(publication_complete(
            &project::read_json::<Publication>(&publication_path(&fx.project, "ask:a-1@1"))
                .unwrap()
        ));
        let overview = crate::overview::render(&fx.project, &[]);
        assert!(
            overview.contains(&format!(
                "  questions for you: 1 (newest a-1@1: {compact})\n"
            )),
            "{overview}"
        );
        // Every sink sees this ask revision once even when publication retries.
        publish(
            &fx.world.ctx(),
            &fx.project,
            &HumanMessage::Ask {
                id: "a-1".into(),
                revision: 1,
            },
        )
        .unwrap();
        assert_eq!(journal_kinds(&fx.project).len(), 1);
        assert_eq!(
            fx.world
                .runner
                .count(&format!("--token ade_needs_you={compact}")),
            1
        );
        assert_eq!(
            fx.world.runner.count(&format!(
                "notification show {compact} --body 1. keep it running another hour"
            )),
            1
        );
    }

    #[test]
    fn a_crash_after_the_record_is_resumed_by_the_ticker() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        // No notification rule: publication fails after the record exists.
        let a = ask(&world.ctx(), "demo", keep_or_stop()).unwrap();
        assert!(rev_path(&project, &a.id, 1).exists());
        let pending =
            project::read_json::<Publication>(&publication_path(&project, "ask:a-1@1")).unwrap();
        assert_eq!(pending.notified, Some(false));
        world.runner.on("notification show", ok(r#"{"result":{}}"#));
        world
            .runner
            .on("workspace report-metadata", ok(r#"{"result":{}}"#));
        tick(&world.ctx(), &project).unwrap();
        assert!(publication_complete(
            &project::read_json::<Publication>(&publication_path(&project, "ask:a-1@1")).unwrap()
        ));
        assert_eq!(world.runner.count("notification show"), 2);
        assert_eq!(
            journal_kinds(&project),
            ["ask a-1@1"],
            "one journal line across both tries"
        );
    }

    #[test]
    fn answers_bind_id_and_revision() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        ask(&ctx, "demo", keep_or_stop()).unwrap();
        let again = ask(
            &ctx,
            "demo",
            NewAsk {
                question: "Keep the experiment running two more hours or stop now?".into(),
                reask: Some("a-1".into()),
                ..keep_or_stop()
            },
        )
        .unwrap();
        assert_eq!(again.revision, 2);
        let e = format!(
            "{:#}",
            answer(&ctx, "demo", "a-1", 1, 2, "test").unwrap_err()
        );
        assert!(e.starts_with("ask_revision_stale"), "{e}");
        let e = format!(
            "{:#}",
            answer(&ctx, "demo", "a-1", 2, 3, "test").unwrap_err()
        );
        assert!(e.starts_with("ask_choice_out_of_range"), "{e}");
        let a = answer(&ctx, "demo", "a-1", 2, 0, "test").unwrap();
        assert!(a.not_understood);
        assert_eq!(a.text, NOT_UNDERSTOOD);
        assert_eq!(not_understood_count(&fx.project), 1);
        let e = format!(
            "{:#}",
            answer(&ctx, "demo", "a-1", 2, 1, "test").unwrap_err()
        );
        assert!(e.starts_with("ask_closed"), "{e}");
        assert!(open_asks(&fx.project).is_empty());
        let e = format!(
            "{:#}",
            publish(
                &ctx,
                &fx.project,
                &HumanMessage::Ask {
                    id: "a-1".into(),
                    revision: 2
                }
            )
            .unwrap_err()
        );
        assert!(e.starts_with("ask_closed"), "{e}");
        let e = format!(
            "{:#}",
            publish(
                &ctx,
                &fx.project,
                &HumanMessage::Ask {
                    id: "a-9".into(),
                    revision: 1
                }
            )
            .unwrap_err()
        );
        assert!(e.starts_with("ask_unknown"), "{e}");
    }

    #[test]
    fn say_refuses_the_previous_line_with_the_same_what_and_means() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        say(
            &ctx,
            "demo",
            "The first lane is done.",
            Some("You can read it now."),
        )
        .unwrap();
        let error = say(
            &ctx,
            "demo",
            "The first lane is done.",
            Some("You can read it now."),
        )
        .unwrap_err();
        assert!(crate::refusal::is(&error));
        assert!(format!("{error:#}").contains("previous board line"));
        assert_eq!(journal_kinds(&fx.project), ["say"]);
        say(
            &ctx,
            "demo",
            "The first lane is done.",
            Some("It is ready to merge."),
        )
        .unwrap();
        assert_eq!(journal_kinds(&fx.project), ["say", "say"]);
    }

    #[test]
    fn a_say_id_publishes_each_sink_once() {
        let fx = fixture();
        let msg = HumanMessage::Say {
            id: "s-7".into(),
            what: "The review is done.".into(),
            means: None,
            landed_round: None,
        };
        let first = publish(&fx.world.ctx(), &fx.project, &msg).unwrap();
        let second = publish(&fx.world.ctx(), &fx.project, &msg).unwrap();
        assert!(first.seq.is_some() && second.seq.is_none());
        assert_eq!(journal_kinds(&fx.project), ["say"]);
        assert_eq!(
            fx.world
                .runner
                .count("--token ade_last=The review is done."),
            1
        );
    }

    #[test]
    fn notices_are_fixed_ids_only() {
        let fx = fixture();
        let e = format!(
            "{:#}",
            publish(
                &fx.world.ctx(),
                &fx.project,
                &HumanMessage::Notice {
                    id: "anything".into()
                }
            )
            .unwrap_err()
        );
        assert!(e.starts_with("notice_unknown"), "{e}");
        let p = publish(
            &fx.world.ctx(),
            &fx.project,
            &HumanMessage::Notice {
                id: "journal_tail".into(),
            },
        )
        .unwrap();
        assert!(!p.board);
    }

    fn distinct_ask(n: usize) -> NewAsk {
        NewAsk {
            question: format!("May I spend {n} dollars on this check?"),
            ..keep_or_stop()
        }
    }

    fn ask_again(fx: &Fx) -> Result<Ask> {
        let n = project::read_json::<u64>(&fx.project.state_dir().join("ask-counter.json"))
            .unwrap_or(0)
            + 1;
        ask(&fx.world.ctx(), "demo", distinct_ask(n as usize))
    }

    #[test]
    fn withdraw_removes_an_older_ask_from_the_board_and_keeps_its_record() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let a = ask_again(&fx).unwrap();
        let b = ask_again(&fx).unwrap();
        let withdrawn = withdraw(&ctx, "demo", &a.id, "This is no longer needed.", "rolf").unwrap();
        assert_eq!(
            withdrawal_of(&fx.project, &a.id, 1),
            Some(withdrawn.clone())
        );
        assert_eq!(withdrawn.by, "rolf");
        assert!(!withdrawn.at.is_empty());
        assert_eq!(latest(&fx.project, &a.id).unwrap(), Some(a.clone()));
        assert_eq!(open_asks(&fx.project), vec![b.clone()]);
        assert!(
            crate::board::compute(&ctx, &fx.project)
                .contains(&("ade_needs_you".into(), compact_line(&b)))
        );
        withdraw(&ctx, "demo", &b.id, "No longer needed.", "rolf").unwrap();
        assert!(
            crate::board::compute(&ctx, &fx.project)
                .contains(&("ade_needs_you".into(), "nothing waits for you".into()))
        );
        assert!(
            answer(&ctx, "demo", &a.id, 1, 1, "test")
                .unwrap_err()
                .to_string()
                .starts_with("ask_withdrawn")
        );
        assert!(
            ask(
                &ctx,
                "demo",
                NewAsk {
                    reask: Some(a.id.clone()),
                    ..keep_or_stop()
                }
            )
            .unwrap_err()
            .to_string()
            .starts_with("ask_withdrawn")
        );
        assert!(open_revision(&fx.project, &a.id, 1).is_err());
        assert!(withdraw(&ctx, "demo", &a.id, "Again.", "rolf").is_err());
        // A withdrawn question does not prevent a genuinely new card.
        ask(&ctx, "demo", distinct_ask(1)).unwrap();
    }

    #[test]
    fn an_answered_or_unknown_ask_cannot_be_withdrawn() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let a = ask_again(&fx).unwrap();
        answer(&ctx, "demo", &a.id, 1, 1, "rolf").unwrap();
        let error = withdraw(&ctx, "demo", &a.id, "No longer needed.", "rolf")
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("ask_closed") && error.contains("answered"));
        assert!(withdrawal_of(&fx.project, &a.id, 1).is_none());
        assert!(
            withdraw(&ctx, "demo", "a-999", "No thanks.", "rolf")
                .unwrap_err()
                .to_string()
                .starts_with("ask_unknown")
        );
    }

    #[test]
    fn question_identity_ignores_case_spacing_and_punctuation() {
        assert_eq!(
            normalized_question("Can't stop -- now?"),
            normalized_question("CANTSTOPNOW")
        );
    }

    #[test]
    fn a_normalized_duplicate_names_the_existing_id_without_writing() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let a = ask(&ctx, "demo", keep_or_stop()).unwrap();
        let before = journal_kinds(&fx.project);
        let duplicate = NewAsk {
            question: "  Keep, the experiment running another hour; or stop now?  ".into(),
            ..keep_or_stop()
        };
        let error = ask(&ctx, "demo", duplicate).unwrap_err().to_string();
        assert_eq!(
            error,
            "ask_duplicate: `a-1` already asks this question; answer: still open"
        );
        assert_eq!(open_asks(&fx.project), vec![a.clone()]);
        assert_eq!(journal_kinds(&fx.project), before);
        assert!(!ask_dir(&fx.project, "a-2").exists());
        assert_eq!(
            project::read_json::<u64>(&fx.project.state_dir().join("ask-counter.json")).unwrap(),
            1
        );
        let same_id = ask(
            &ctx,
            "demo",
            NewAsk {
                reask: Some(a.id),
                ..keep_or_stop()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(same_id.starts_with("ask_duplicate"), "{same_id}");

        // A re-ask also cannot copy a different open question.
        let b = ask(&ctx, "demo", distinct_ask(2)).unwrap();
        assert!(
            ask(
                &ctx,
                "demo",
                NewAsk {
                    reask: Some(b.id),
                    ..keep_or_stop()
                }
            )
            .unwrap_err()
            .to_string()
            .starts_with("ask_duplicate")
        );
        answer(&ctx, "demo", "a-1", 1, 1, "rolf").unwrap();
        let error = ask(&ctx, "demo", keep_or_stop()).unwrap_err().to_string();
        assert!(error.contains("`a-1`") && error.contains("keep it running another hour"));
    }

    #[test]
    fn open_asks_have_no_cap() {
        let fx = fixture();
        for _ in 0..4 {
            ask_again(&fx).unwrap();
        }
        assert_eq!(open_asks(&fx.project).len(), 4);
        assert!(rev_path(&fx.project, "a-4", 1).exists());
    }

    #[test]
    fn a_pending_publication_does_not_block_a_new_question() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        // No board or notification rule: each ask is recorded but unpublished.
        for n in 1..=3 {
            ask(&world.ctx(), "demo", distinct_ask(n)).unwrap();
        }
        assert_eq!(open_asks(&project).len(), 3);
        assert!(!publication_complete(
            &project::read_json::<Publication>(&publication_path(&project, "ask:a-1@1")).unwrap()
        ));
        ask(&world.ctx(), "demo", keep_or_stop()).unwrap();
        assert_eq!(open_asks(&project).len(), 4);
    }

    #[test]
    fn reasking_an_open_question_keeps_its_identifier() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        for _ in 0..3 {
            ask_again(&fx).unwrap();
        }
        let first = ask(
            &ctx,
            "demo",
            NewAsk {
                reask: Some("a-1".into()),
                ..distinct_ask(5)
            },
        )
        .unwrap();
        assert_eq!((first.id.as_str(), first.revision), ("a-1", 2));

        let merged = ask(
            &ctx,
            "demo",
            NewAsk {
                reask: Some("a-3".into()),
                ..distinct_ask(6)
            },
        )
        .unwrap();
        assert_eq!((merged.id.as_str(), merged.revision), ("a-3", 2));
        assert_eq!(open_asks(&fx.project).len(), 3);
        assert!(journal_kinds(&fx.project).contains(&"ask a-3@2".to_string()));
        let ids: Vec<String> = open_asks(&fx.project).into_iter().map(|a| a.id).collect();
        assert!(ids.contains(&"a-1".to_string()) && ids.contains(&"a-2".to_string()));
    }
}
