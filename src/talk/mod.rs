//! The plugin-owned conversation surface (SPEC-ADE D18): `ha talk <slug>`.
//!
//! An alternate-screen project view in its own tab. The conversation renders
//! only `talk/journal.jsonl`: checked coordinator messages, Rolf's own lines,
//! asks with their numbered choices, `say` lines and fixed notices. Rolf's
//! input is a recoverable request (`queued`, `submitted`, `uncertain`,
//! `accepted`, item 35); replaying the journal renders and never re-sends.
//!
//! One append owner: every writer appends under `talk/journal.lock`,
//! fsyncs and releases. A trailing incomplete line is terminated, skipped by
//! readers, and reported once as a `journal_tail` notice.
//!
//! Decision on item 24: ships this round, on by default for a `claude`
//! coordinator and off for other kinds until their hooks are verified;
//! `talk = true | false` in `PROJECT.md` front matter overrides.

use std::borrow::Cow;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contracts::{HumanMessage, Recipient, TalkInbound, TalkRequestState};
use crate::herdr::Herdr;
use crate::paths::Ctx;
use crate::project::{self, Project};

mod cost;
pub(crate) mod overview;
pub(crate) mod screen;
mod stale;
mod theme;
pub(crate) mod view;

/// Entries are bounded (D18 item 6).
pub(crate) const MAX_ENTRY_BYTES: usize = 64 * 1024;

/// One journal entry. Serialized flattened next to `seq`, so an inbound line
/// is `{"seq":n,...,"inbound":{...}}` and also parses as A0's
/// `TalkJournalRecord`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Entry {
    Inbound(TalkInbound),
    /// Rolf's own line, shown as typed.
    Rolf {
        request: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<AnswerRef>,
    },
    Say {
        what: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        means: Option<String>,
        /// The round whose successful merge this line is landing evidence for
        /// (SPEC-talk §6.1). Omitted on ordinary say entries.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        landed_round: Option<String>,
    },
    Ask {
        id: String,
        revision: u32,
    },
    Answer {
        id: String,
        revision: u32,
        choice: u32,
    },
    Notice {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AnswerRef {
    pub(crate) id: String,
    pub(crate) revision: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) key: Option<String>,
    #[serde(default)]
    pub(crate) at: String,
    #[serde(flatten)]
    pub(crate) entry: Entry,
}

pub(crate) fn talk_dir(project: &Project) -> PathBuf {
    project.dir().join("talk")
}

pub(crate) fn journal_path(project: &Project) -> PathBuf {
    talk_dir(project).join("journal.jsonl")
}

#[derive(Debug, Default)]
pub(crate) struct Journal {
    pub(crate) lines: Vec<Line>,
    /// Complete lines that did not parse (a terminated partial tail).
    pub(crate) skipped: usize,
    /// The file ends without a newline: a write was cut.
    pub(crate) tail_incomplete: bool,
}

pub(crate) fn parse(bytes: &[u8]) -> Journal {
    let mut journal = Journal::default();
    let text = String::from_utf8_lossy(bytes);
    journal.tail_incomplete = !text.is_empty() && !text.ends_with('\n');
    let mut parts: Vec<&str> = text.split('\n').collect();
    // The last piece is "" after a final newline, or the incomplete tail.
    parts.pop();
    for part in parts {
        if part.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Line>(part) {
            Ok(line) => journal.lines.push(line),
            Err(_) => journal.skipped += 1,
        }
    }
    journal
}

pub(crate) fn read(project: &Project) -> Journal {
    parse(&std::fs::read(journal_path(project)).unwrap_or_default())
}

struct Locked {
    _file: File,
}

fn lock_file(project: &Project, name: &str) -> Result<Locked> {
    std::fs::create_dir_all(talk_dir(project))?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(talk_dir(project).join(name))?;
    file.lock()?;
    Ok(Locked { _file: file })
}

/// Appends one entry under the journal lock and fsyncs. With `key`, an entry
/// already carrying that key is not appended again (`Ok(None)`).
pub(crate) fn append(project: &Project, key: Option<&str>, entry: Entry) -> Result<Option<u64>> {
    let _lock = lock_file(project, "journal.lock")?;
    let path = journal_path(project);
    let bytes = std::fs::read(&path).unwrap_or_default();
    let journal = parse(&bytes);
    if let Some(key) = key
        && journal.lines.iter().any(|l| l.key.as_deref() == Some(key))
    {
        return Ok(None);
    }
    let mut file = File::options().create(true).append(true).open(&path)?;
    let mut seq = journal.lines.last().map_or(0, |l| l.seq);
    let mut out = String::new();
    if journal.tail_incomplete {
        // Terminate the cut line so it stays a skipped line of its own, then
        // report it once. It is never an acknowledgement or a result.
        out.push('\n');
        seq += 1;
        out.push_str(&serde_json::to_string(&Line {
            seq,
            key: None,
            at: project::now(),
            entry: Entry::Notice {
                id: "journal_tail".into(),
            },
        })?);
        out.push('\n');
    }
    seq += 1;
    let text = serde_json::to_string(&Line {
        seq,
        key: key.map(str::to_string),
        at: project::now(),
        entry,
    })?;
    if text.len() > MAX_ENTRY_BYTES {
        bail!(
            "talk_entry_too_large: {} bytes, at most {MAX_ENTRY_BYTES}",
            text.len()
        );
    }
    out.push_str(&text);
    out.push('\n');
    file.write_all(out.as_bytes())?;
    file.sync_all()?;
    Ok(Some(seq))
}

// -------------------------------------------------- coordinator-pane prompts

/// What a prompt typed into the coordinator pane is: the delivery of a
/// talk-tab message already recorded under a request id, or a harness line
/// (priming, nudge, event) that is not Rolf's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingPrompt {
    Delivery(String),
    Automated,
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingPromptRecord {
    delivery: Option<String>,
    at: i64,
}

/// A marker older than this belongs to a hook that never fired; it must not
/// claim a later prompt with the same text.
const PENDING_PROMPT_SECS: i64 = 120;
const PASTE_OPEN: &str = "<pasted_content id=\"";
const TASK_OPEN: &str = "<task-notification>";
const TASK_CLOSE: &str = "</task-notification>";

/// Removes Claude Code's wrapper only when the whole prompt is made of paste
/// blocks. Native words before or after a block make this `None`, so a mixed
/// prompt remains Rolf's request rather than consuming a harness marker.
fn pasted_contents(text: &str) -> Option<String> {
    let mut rest = text.trim();
    let mut contents = Vec::new();
    while let Some(after_open) = rest.strip_prefix(PASTE_OPEN) {
        let (id, after_id) = after_open.split_once("\">")?;
        if id.is_empty() || id.contains(['<', '>', '"']) {
            return None;
        }
        let close = format!("</pasted_content id=\"{id}\">");
        let (content, after_close) = after_id.split_once(&close)?;
        contents.push(content.trim());
        rest = after_close.trim_start();
    }
    if contents.is_empty() || !rest.is_empty() {
        return None;
    }
    Some(contents.join("\n"))
}

fn marker_text(text: &str) -> Cow<'_, str> {
    pasted_contents(text).map_or_else(|| Cow::Borrowed(text), Cow::Owned)
}

/// Claude Code submits background-task notices through the prompt hook even
/// though nobody typed them. Mixed native text is deliberately not included.
pub(crate) fn is_task_notification_prompt(text: &str) -> bool {
    let mut rest = text.trim();
    let mut found = false;
    while let Some(after_open) = rest.strip_prefix(TASK_OPEN) {
        let Some((_, after_close)) = after_open.split_once(TASK_CLOSE) else {
            return false;
        };
        found = true;
        rest = after_close.trim_start();
    }
    found && rest.is_empty()
}

/// Historical hook mistakes stay in the append-only journal but are omitted
/// from the conversation. A pasted human message is retained; only known
/// harness lines inside a pure paste wrapper are hidden.
fn is_parent_status_line(text: &str) -> bool {
    let marker = marker_text(text);
    let mut words = marker.split_whitespace();
    let Some(status) = words.next() else {
        return false;
    };
    if !matches!(status, "BLOCKED" | "GONE") {
        return false;
    }
    let Some(name) = words.next() else {
        return false;
    };
    if words.next().is_some() {
        return false;
    }
    let Some(rest) = name.strip_prefix("hp-") else {
        return false;
    };
    let Some((slug, thread)) = rest.rsplit_once("-t-") else {
        return false;
    };
    !slug.is_empty() && thread.len() >= 4 && thread.chars().all(|c| c.is_ascii_digit())
}

/// Herdr's parent notifier keys BLOCKED/GONE by the full agent name, unlike
/// the plugin courier's `t-NNNN` lines. They are machine input, not a request.
pub(crate) fn mark_parent_status_prompt(project: &Project, pane: &str, text: &str) -> Result<()> {
    if is_parent_status_line(text) {
        mark_automated_prompt(project, pane, text)?;
    }
    Ok(())
}

pub(crate) fn is_historical_system_prompt(text: &str) -> bool {
    if is_task_notification_prompt(text) || is_parent_status_line(text) {
        return true;
    }
    let Some(text) = pasted_contents(text) else {
        return false;
    };
    let text = text.trim();
    text.starts_with("DONE t-")
        || text.starts_with("WAITING t-")
        || text.starts_with("BLOCKED t-")
        || text.starts_with("GONE t-")
        || text == crate::steps::NUDGE_TEXT
        || (text.starts_with("You are the coordinator of the herdr project `")
            && text.contains(" skill coordinator`")
            && text.contains(" context "))
}

fn pending_prompt_path(project: &Project, pane: &str, text: &str) -> PathBuf {
    let mut hash = Sha256::new();
    hash.update(pane.as_bytes());
    hash.update(b"\n");
    hash.update(marker_text(text).trim_end().as_bytes());
    talk_dir(project)
        .join("prompts")
        .join(format!("{:x}.json", hash.finalize()))
}

/// Records the next prompt this coordinator pane will receive so the
/// prompt-submit hook can classify it. Only written when a hook is bound, so
/// a kind without one leaves no files behind.
fn mark_pending_prompt(
    project: &Project,
    pane: &str,
    text: &str,
    delivery: Option<&str>,
) -> Result<()> {
    if !crate::hook::captures(project, pane) {
        return Ok(());
    }
    let path = pending_prompt_path(project, pane, text);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    project::write_json(
        &path,
        &PendingPromptRecord {
            delivery: delivery.map(str::to_string),
            at: jiff::Timestamp::now().as_second(),
        },
    )
}

/// Marks the prompt a talk delivery is about to type, so the hook cites the
/// request `submit` already recorded instead of writing a second one.
pub(crate) fn mark_talk_delivery(
    project: &Project,
    pane: &str,
    request: &str,
    text: &str,
) -> Result<()> {
    mark_pending_prompt(project, pane, text, Some(request))
}

/// Marks a harness line so the hook never records it as a request from Rolf.
pub(crate) fn mark_automated_prompt(project: &Project, pane: &str, text: &str) -> Result<()> {
    mark_pending_prompt(project, pane, text, None)
}

/// Reads and removes the marker for this pane and exact prompt.
pub(crate) fn take_pending_prompt(
    project: &Project,
    pane: &str,
    text: &str,
) -> Option<PendingPrompt> {
    let path = pending_prompt_path(project, pane, text);
    let record: PendingPromptRecord = project::read_json(&path)?;
    let _ = std::fs::remove_file(&path);
    if jiff::Timestamp::now().as_second() - record.at > PENDING_PROMPT_SECS {
        return None;
    }
    Some(match record.delivery {
        Some(request) => PendingPrompt::Delivery(request),
        None => PendingPrompt::Automated,
    })
}

// ------------------------------------------------------------- settings

/// `talk` in `PROJECT.md` front matter, otherwise on for every coordinator
/// adapter. Coordinator admission checks the adapter's declared support.
pub(crate) fn enabled(project: &Project) -> bool {
    if let Ok((settings, _)) = project.read_project_md()
        && let Some(on) = settings.talk
    {
        return on;
    }
    true
}

/// The kind the coordinator was launched with (the `coordinator` workflow).
pub(crate) fn coordinator_kind(project: &Project) -> String {
    project
        .coordinator()
        .map(|c| c.launch.kind)
        .filter(|kind| !kind.is_empty())
        .unwrap_or_default()
}

/// Per-kind labels (D17 item 2, D18 item 5). The shipped capability table is
/// A2's `adapters.rs`; these are the values this spec fixes today.
fn recipient(project: &Project) -> Recipient {
    let coord = project.coordinator().unwrap_or_default();
    Recipient {
        coordinator_attempt: coord.attempt(),
        pane: coord.pane_id,
    }
}

// ---------------------------------------------------------------- writer

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Native {
    suspended: bool,
    since: String,
}

fn native_path(project: &Project) -> PathBuf {
    talk_dir(project).join("native.toml")
}

/// True while Rolf works in the native pane (`!native` until `!back`). The
/// outbox's writer (A2) must not type into the coordinator while this holds.
pub(crate) fn writer_suspended(project: &Project) -> bool {
    std::fs::read_to_string(native_path(project))
        .ok()
        .and_then(|t| toml::from_str::<Native>(&t).ok())
        .is_some_and(|n| n.suspended)
}

fn set_suspended(project: &Project, on: bool) -> Result<()> {
    std::fs::create_dir_all(talk_dir(project))?;
    project::write_atomic(
        &native_path(project),
        toml::to_string(&Native {
            suspended: on,
            since: project::now(),
        })?
        .as_bytes(),
    )
}

/// The serialized writer's lock for this coordinator (D8). A2's outbox takes
/// the same lock around its read-and-prompt.
pub(crate) struct WriterLock {
    _lock: Locked,
}

pub(crate) fn writer_lock(project: &Project) -> Result<WriterLock> {
    Ok(WriterLock {
        _lock: lock_file(project, "writer.lock")?,
    })
}

/// The latest state of every request, in the order they were queued.
pub(crate) fn requests(journal: &Journal) -> Vec<(TalkInbound, String)> {
    let mut order: Vec<String> = Vec::new();
    let mut state: std::collections::BTreeMap<String, TalkInbound> = Default::default();
    let mut text: std::collections::BTreeMap<String, String> = Default::default();
    for line in &journal.lines {
        match &line.entry {
            Entry::Rolf {
                request, text: t, ..
            } => {
                text.insert(request.clone(), t.clone());
            }
            Entry::Inbound(inbound) => {
                if !state.contains_key(&inbound.request) {
                    order.push(inbound.request.clone());
                }
                state.insert(inbound.request.clone(), inbound.clone());
            }
            _ => {}
        }
    }
    order
        .into_iter()
        .filter_map(|r| {
            let inbound = state.remove(&r)?;
            let t = text.remove(&r).unwrap_or_default();
            Some((inbound, t))
        })
        .collect()
}

/// The most recent messages Rolf sent, oldest first, with the request id to
/// cite. The coordinator digest prints them so an id is always findable.
pub(crate) fn recent_requests(project: &Project, limit: usize) -> Vec<(String, String)> {
    let journal = read(project);
    let mut found: Vec<(String, String)> = journal
        .lines
        .iter()
        .filter_map(|line| match &line.entry {
            Entry::Rolf { request, text, .. } if !is_historical_system_prompt(text) => {
                Some((request.clone(), text.clone()))
            }
            _ => None,
        })
        .collect();
    if found.len() > limit {
        found.drain(..found.len() - limit);
    }
    found
}

fn notice(ctx: &Ctx, project: &Project, id: &str) {
    let _ = crate::ask::publish(ctx, project, &HumanMessage::Notice { id: id.to_string() });
}

fn coordinator_herdr<'a>(ctx: &'a Ctx, project: &Project) -> Option<(Herdr<'a>, String)> {
    let coord = project.coordinator()?;
    if coord.socket.is_empty() || coord.pane_id.is_empty() {
        return None;
    }
    Some((
        Herdr::new(ctx.env.herdr_bin(), &coord.socket, ctx.runner),
        coord.pane_id,
    ))
}

/// The coordinator's detector state, or `None` when herdr cannot be read.
fn coordinator_state(ctx: &Ctx, project: &Project) -> Option<String> {
    let (h, pane) = coordinator_herdr(ctx, project)?;
    let agents = h.agent_list().ok()?;
    Some(
        agents
            .into_iter()
            .find(|a| a.pane_id == pane)
            .map(|a| a.agent_status)
            .unwrap_or_else(|| "gone".into()),
    )
}

/// Sends queued requests in order through the serialized writer. Only
/// `queued` requests are ever sent; `uncertain` ones are never re-sent.
fn deliver_queued(ctx: &Ctx, project: &Project) -> Result<Vec<(String, TalkRequestState)>> {
    let _writer = writer_lock(project)?;
    let mut out = Vec::new();
    if writer_suspended(project) {
        return Ok(out);
    }
    let pending: Vec<(TalkInbound, String)> = requests(&read(project))
        .into_iter()
        .filter(|(i, _)| i.state == TalkRequestState::Queued)
        .collect();
    for (inbound, text) in pending {
        let current = recipient(project);
        if inbound.recipient != current {
            // Never retargeted: the request stays queued for the old
            // incarnation and Rolf is told once.
            let _ = crate::ask::publish_keyed(
                ctx,
                project,
                &HumanMessage::Notice {
                    id: "recipient_changed".into(),
                },
                Some(&format!("recipient_changed:{}", inbound.request)),
            );
            continue;
        }
        match coordinator_state(ctx, project).as_deref() {
            Some("idle") | Some("done") => {}
            _ => break,
        }
        let Some((h, pane)) = coordinator_herdr(ctx, project) else {
            break;
        };
        let mark = |state| {
            append(
                project,
                None,
                Entry::Inbound(TalkInbound {
                    request: inbound.request.clone(),
                    state,
                    recipient: inbound.recipient.clone(),
                }),
            )
        };
        // Uncertain is journalled before the line is typed: a crash after
        // typing leaves a request that is never sent again (D18).
        mark(TalkRequestState::Uncertain)?;
        mark_talk_delivery(project, &pane, &inbound.request, &text)?;
        let state = match h.agent_prompt(&pane, &text) {
            Ok(()) => TalkRequestState::Submitted,
            Err(e) if matches!(e.code.as_str(), "timeout" | "unreachable" | "failed") => {
                TalkRequestState::Uncertain
            }
            // herdr refused before typing (blocked, not found): still queued.
            Err(_) => {
                mark(TalkRequestState::Queued)?;
                break;
            }
        };
        if state == TalkRequestState::Submitted {
            mark(state)?;
        }
        if state == TalkRequestState::Uncertain {
            notice(ctx, project, "talk_uncertain");
        }
        out.push((inbound.request, state));
        if state != TalkRequestState::Submitted {
            break;
        }
    }
    Ok(out)
}

/// Records a message Rolf typed straight into the coordinator pane, verbatim,
/// under a new request id the coordinator can cite.
pub(crate) fn record_pane_request(project: &Project, text: &str) -> Result<String> {
    let request = fresh_request();
    append(
        project,
        None,
        Entry::Rolf {
            request: request.clone(),
            text: text.to_string(),
            answer: None,
        },
    )?;
    Ok(request)
}

fn fresh_request() -> String {
    format!(
        "q-{}-{}",
        jiff::Timestamp::now().as_millisecond(),
        std::process::id()
    )
}

/// Rolf typed a line: journal the intent first, then hand it to the writer.
fn submit(ctx: &Ctx, project: &Project, text: &str) -> Result<(String, TalkRequestState)> {
    submit_with_answer(ctx, project, text, None)
}

fn submit_with_answer(
    ctx: &Ctx,
    project: &Project,
    text: &str,
    answer: Option<AnswerRef>,
) -> Result<(String, TalkRequestState)> {
    let request = fresh_request();
    append(
        project,
        None,
        Entry::Rolf {
            request: request.clone(),
            text: text.to_string(),
            answer,
        },
    )?;
    append(
        project,
        None,
        Entry::Inbound(TalkInbound {
            request: request.clone(),
            state: TalkRequestState::Queued,
            recipient: recipient(project),
        }),
    )?;
    // The request is durable now. Delivery failure leaves it queued; it must
    // not keep the composer full and tempt a second submission.
    let sent = deliver_queued(ctx, project).unwrap_or_default();
    let state = sent
        .iter()
        .find(|(r, _)| *r == request)
        .map(|(_, s)| *s)
        .unwrap_or(TalkRequestState::Queued);
    if state == TalkRequestState::Queued {
        notice(ctx, project, "request_waiting");
    }
    Ok((request, state))
}

/// Called when the coordinator's turn ends after a submission (the
/// correction hook, A2): every `submitted` request becomes `accepted`.
pub(crate) fn mark_accepted(project: &Project) -> Result<usize> {
    let submitted: Vec<TalkInbound> = requests(&read(project))
        .into_iter()
        .map(|(i, _)| i)
        .filter(|i| i.state == TalkRequestState::Submitted)
        .collect();
    for mut inbound in submitted.iter().cloned() {
        inbound.state = TalkRequestState::Accepted;
        append(project, None, Entry::Inbound(inbound))?;
    }
    Ok(submitted.len())
}

/// Ordinary input keeps the existing commands and delivery path. Numeric
/// answering is exclusively the screen's last-drawn binding, never text parsing.
pub(crate) fn handle(ctx: &Ctx, project: &Project, line: &str) -> Result<()> {
    match line.trim() {
        "!stop" => {
            if let Some((h, pane)) = coordinator_herdr(ctx, project) {
                h.call(
                    &["agent", "send-keys", &pane, "esc"],
                    Duration::from_secs(10),
                )
                .map_err(|e| anyhow::anyhow!("{}", e.message))?;
            }
        }
        "!native" => {
            set_suspended(project, true)?;
            notice(ctx, project, "native_on");
        }
        "!back" => match coordinator_state(ctx, project).as_deref() {
            Some("idle" | "done" | "working") => {
                set_suspended(project, false)?;
                notice(ctx, project, "native_off");
                deliver_queued(ctx, project)?;
            }
            _ => notice(ctx, project, "native_not_ready"),
        },
        _ => {
            submit(ctx, project, line.trim_end_matches(['\n', '\r']))?;
        }
    }
    Ok(())
}

fn answer(ctx: &Ctx, project: &Project, target: &view::Target, choice: u32) -> Result<()> {
    match crate::ask::answer(
        ctx,
        &project.slug,
        &target.id,
        target.revision,
        choice,
        "talk",
    ) {
        Ok(a) => {
            let text = format!(
                "ANSWER {}@{} {choice}: Rolf chose \"{}\"",
                target.id, target.revision, a.text
            );
            submit_with_answer(
                ctx,
                project,
                &text,
                Some(AnswerRef {
                    id: target.id.clone(),
                    revision: target.revision,
                }),
            )?;
        }
        Err(e) => notice(
            ctx,
            project,
            if e.to_string().starts_with("ask_revision_stale") {
                "ask_redrawn"
            } else {
                "ask_not_found"
            },
        ),
    }
    Ok(())
}

pub(crate) fn replay(ctx: &Ctx, slug: &str) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    Ok(view::Conversation::load(&project, &read(&project)).replay())
}

// ------------------------------------------------------------ tab and tick

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct SurfaceTab {
    pub(crate) tab_id: String,
    pub(crate) pane_id: String,
    pub(crate) created: String,
}

fn surface_path(project: &Project) -> PathBuf {
    talk_dir(project).join("surface.toml")
}

/// Creates the `talk` tab in the coordinator workspace when talk is on and
/// the recorded tab is gone, and runs `ha talk <slug>` in it. Called by
/// `ha open` (A2's `coordinator::open`) after the coordinator is bound.
pub(crate) fn ensure_tab(ctx: &Ctx, project: &Project) -> Result<Option<SurfaceTab>> {
    if !enabled(project) {
        return Ok(None);
    }
    let coord = project.coordinator().context("the project is not open")?;
    let h = Herdr::new(ctx.env.herdr_bin(), &coord.socket, ctx.runner);
    if let Some(tab) = std::fs::read_to_string(surface_path(project))
        .ok()
        .and_then(|t| toml::from_str::<SurfaceTab>(&t).ok())
        && h.pane_list()
            .map(|panes| panes.iter().any(|p| p.pane_id == tab.pane_id))
            .unwrap_or(false)
    {
        return Ok(Some(tab));
    }
    let dir = project.dir().to_string_lossy().into_owned();
    let result = h
        .call(
            &[
                "tab",
                "create",
                "--workspace",
                &coord.workspace_id,
                "--cwd",
                &dir,
                "--label",
                "talk",
                "--no-focus",
            ],
            Duration::from_secs(10),
        )
        .map_err(|e| anyhow::anyhow!("could not create the talk tab: {}", e.message))?;
    let pane = &result["root_pane"];
    let tab = SurfaceTab {
        tab_id: pane["tab_id"].as_str().unwrap_or_default().to_string(),
        pane_id: pane["pane_id"].as_str().unwrap_or_default().to_string(),
        created: project::now(),
    };
    let prefix = crate::coordinator::current_prefix(&ctx.root)?;
    let command = format!("{prefix} talk {}", project.slug);
    h.call(
        &["pane", "run", &tab.pane_id, &command],
        Duration::from_secs(10),
    )
    .map_err(|e| anyhow::anyhow!("could not start the talk surface: {}", e.message))?;
    std::fs::create_dir_all(talk_dir(project))?;
    project::write_atomic(&surface_path(project), toml::to_string(&tab)?.as_bytes())?;
    Ok(Some(tab))
}

/// Ticker pass: a fixed notice when the coordinator reads `blocked` (once per
/// episode), and queued requests sent when it is ready.
pub(crate) fn tick(ctx: &Ctx, project: &Project) -> Result<()> {
    if !enabled(project) || !journal_path(project).exists() {
        return Ok(());
    }
    let flag = talk_dir(project).join("blocked.flag");
    match coordinator_state(ctx, project).as_deref() {
        Some("blocked") => {
            if !flag.exists() {
                notice(ctx, project, "needs_you_in_pane");
                let _ = std::fs::write(&flag, project::now());
            }
        }
        Some(_) => {
            let _ = std::fs::remove_file(&flag);
            deliver_queued(ctx, project)?;
        }
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::round::testkit::{Fx, fixture};
    use crate::runner::Output;
    use crate::runner::fake::{fail, ok, timeout};
    use crate::scenarios::agent_json;

    /// A fixture whose coordinator reads `state` and whose `agent prompt`
    /// answers with whatever `reply` holds.
    fn talk_fixture(state: &str) -> (Fx, Rc<RefCell<Output>>) {
        let fx = fixture();
        set_state(&fx, state);
        let reply = Rc::new(RefCell::new(ok(r#"{"result":{}}"#)));
        let r = reply.clone();
        fx.world.runner.on_fn(
            |cmd| cmd.display().contains("agent prompt"),
            move |_| Ok(r.borrow().clone()),
        );
        fx.world
            .runner
            .on("agent send-keys", ok(r#"{"result":{}}"#));
        (fx, reply)
    }

    fn set_state(fx: &Fx, state: &str) {
        *fx.world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json("w1", "w1:t1", "w1:p1", "/p", "hp-demo-coordinator", state)
        );
    }

    fn states(fx: &Fx) -> Vec<TalkRequestState> {
        requests(&read(&fx.project))
            .into_iter()
            .map(|(i, _)| i.state)
            .collect()
    }

    fn notices(fx: &Fx) -> Vec<String> {
        read(&fx.project)
            .lines
            .into_iter()
            .filter_map(|l| match l.entry {
                Entry::Notice { id } => Some(id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_inbound_line_parses_as_the_contract_record() {
        let (fx, _) = talk_fixture("idle");
        submit(&fx.world.ctx(), &fx.project, "hello there").unwrap();
        let text = std::fs::read_to_string(journal_path(&fx.project)).unwrap();
        let inbound: Vec<crate::contracts::TalkJournalRecord> = text
            .lines()
            .filter(|l| l.contains("\"inbound\""))
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        // Queued, uncertain before typing (a crash there never re-sends),
        // then submitted (review defect: re-send after a crash).
        let states: Vec<_> = inbound.iter().map(|r| r.inbound.state).collect();
        assert_eq!(
            states,
            [
                TalkRequestState::Queued,
                TalkRequestState::Uncertain,
                TalkRequestState::Submitted
            ]
        );
        assert_eq!(inbound[0].inbound.recipient.pane, "w1:p1");
    }

    #[test]
    fn a_cut_tail_is_skipped_terminated_and_reported_once() {
        let fx = fixture();
        append(
            &fx.project,
            None,
            Entry::Say {
                what: "One.".into(),
                means: None,
                landed_round: None,
            },
        )
        .unwrap();
        let mut f = File::options()
            .append(true)
            .open(journal_path(&fx.project))
            .unwrap();
        f.write_all(br#"{"seq":2,"say":{"wh"#).unwrap();
        drop(f);
        let j = read(&fx.project);
        assert!(j.tail_incomplete);
        assert_eq!(j.lines.len(), 1, "the cut line is never read as an entry");
        append(
            &fx.project,
            None,
            Entry::Say {
                what: "Two.".into(),
                means: None,
                landed_round: None,
            },
        )
        .unwrap();
        append(
            &fx.project,
            None,
            Entry::Say {
                what: "Three.".into(),
                means: None,
                landed_round: None,
            },
        )
        .unwrap();
        let j = read(&fx.project);
        assert!(!j.tail_incomplete);
        assert_eq!(j.skipped, 1);
        assert_eq!(notices(&fx), ["journal_tail"]);
        let seqs: Vec<u64> = j.lines.iter().map(|l| l.seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4]);
    }

    #[test]
    fn a_keyed_entry_is_appended_once_and_oversize_is_refused() {
        let fx = fixture();
        let e = || Entry::Notice {
            id: "native_on".into(),
        };
        assert_eq!(append(&fx.project, Some("k"), e()).unwrap(), Some(1));
        assert_eq!(append(&fx.project, Some("k"), e()).unwrap(), None);
        let big = Entry::Say {
            what: "x".repeat(MAX_ENTRY_BYTES),
            means: None,
            landed_round: None,
        };
        assert!(
            format!("{:#}", append(&fx.project, None, big).unwrap_err())
                .starts_with("talk_entry_too_large")
        );
        assert_eq!(read(&fx.project).lines.len(), 1);
    }

    #[test]
    fn a_request_is_submitted_then_accepted_at_turn_end() {
        let (fx, _) = talk_fixture("idle");
        let (_, state) = submit(&fx.world.ctx(), &fx.project, "please look at the tests").unwrap();
        assert_eq!(state, TalkRequestState::Submitted);
        assert_eq!(
            fx.world
                .runner
                .count("agent prompt w1:p1 please look at the tests"),
            1
        );
        assert_eq!(mark_accepted(&fx.project).unwrap(), 1);
        assert_eq!(states(&fx), [TalkRequestState::Accepted]);
        assert_eq!(mark_accepted(&fx.project).unwrap(), 0);
    }

    #[test]
    fn a_timeout_is_uncertain_and_never_resent() {
        let (fx, reply) = talk_fixture("idle");
        *reply.borrow_mut() = timeout();
        let (_, state) = submit(&fx.world.ctx(), &fx.project, "go on").unwrap();
        assert_eq!(state, TalkRequestState::Uncertain);
        assert_eq!(notices(&fx), ["talk_uncertain"]);
        *reply.borrow_mut() = ok(r#"{"result":{}}"#);
        deliver_queued(&fx.world.ctx(), &fx.project).unwrap();
        tick(&fx.world.ctx(), &fx.project).unwrap();
        assert_eq!(
            fx.world.runner.count("agent prompt"),
            1,
            "uncertain is never re-sent"
        );
        assert_eq!(states(&fx), [TalkRequestState::Uncertain]);
    }

    #[test]
    fn a_refusal_before_typing_stays_queued_and_is_sent_later_in_order() {
        let (fx, reply) = talk_fixture("idle");
        *reply.borrow_mut() = fail(1, r#"{"error":{"code":"agent_busy","message":"busy"}}"#);
        submit(&fx.world.ctx(), &fx.project, "first").unwrap();
        submit(&fx.world.ctx(), &fx.project, "second").unwrap();
        assert_eq!(
            states(&fx),
            [TalkRequestState::Queued, TalkRequestState::Queued]
        );
        assert_eq!(notices(&fx), ["request_waiting", "request_waiting"]);
        *reply.borrow_mut() = ok(r#"{"result":{}}"#);
        tick(&fx.world.ctx(), &fx.project).unwrap();
        assert_eq!(
            states(&fx),
            [TalkRequestState::Submitted, TalkRequestState::Submitted]
        );
        let calls: Vec<String> = fx
            .world
            .runner
            .calls
            .borrow()
            .iter()
            .map(|c| c.display())
            .filter(|d| d.contains("agent prompt"))
            .collect();
        assert!(
            calls[calls.len() - 2].ends_with("first") && calls[calls.len() - 1].ends_with("second")
        );
    }

    #[test]
    fn a_blocked_coordinator_keeps_the_request_queued_and_gets_one_notice() {
        let (fx, _) = talk_fixture("blocked");
        submit(&fx.world.ctx(), &fx.project, "hello").unwrap();
        tick(&fx.world.ctx(), &fx.project).unwrap();
        tick(&fx.world.ctx(), &fx.project).unwrap();
        assert_eq!(fx.world.runner.count("agent prompt"), 0);
        assert_eq!(notices(&fx), ["request_waiting", "needs_you_in_pane"]);
        set_state(&fx, "idle");
        tick(&fx.world.ctx(), &fx.project).unwrap();
        assert_eq!(states(&fx), [TalkRequestState::Submitted]);
        set_state(&fx, "blocked");
        tick(&fx.world.ctx(), &fx.project).unwrap();
        assert_eq!(
            notices(&fx)
                .iter()
                .filter(|n| *n == "needs_you_in_pane")
                .count(),
            2,
            "one per episode"
        );
    }

    #[test]
    fn a_changed_recipient_is_never_retargeted() {
        let (fx, reply) = talk_fixture("idle");
        *reply.borrow_mut() = fail(1, r#"{"error":{"code":"agent_busy","message":"busy"}}"#);
        submit(&fx.world.ctx(), &fx.project, "hello").unwrap();
        fx.project
            .update_coordinator(|c| c.pane_id = "w1:p9".into())
            .unwrap();
        *reply.borrow_mut() = ok(r#"{"result":{}}"#);
        tick(&fx.world.ctx(), &fx.project).unwrap();
        tick(&fx.world.ctx(), &fx.project).unwrap();
        assert_eq!(
            fx.world.runner.count("agent prompt"),
            1,
            "only the first refused try"
        );
        assert_eq!(states(&fx), [TalkRequestState::Queued]);
        assert_eq!(
            notices(&fx)
                .iter()
                .filter(|n| *n == "recipient_changed")
                .count(),
            1
        );
    }

    #[test]
    fn native_suspends_the_writer_and_back_rechecks_readiness() {
        let (fx, _) = talk_fixture("idle");
        let ctx = fx.world.ctx();
        handle(&ctx, &fx.project, "!native").unwrap();
        assert!(writer_suspended(&fx.project));
        handle(&ctx, &fx.project, "while you are there").unwrap();
        assert_eq!(fx.world.runner.count("agent prompt"), 0);
        set_state(&fx, "blocked");
        handle(&ctx, &fx.project, "!back").unwrap();
        assert!(writer_suspended(&fx.project));
        set_state(&fx, "idle");
        handle(&ctx, &fx.project, "!back").unwrap();
        assert!(!writer_suspended(&fx.project));
        assert_eq!(
            fx.world
                .runner
                .count("agent prompt w1:p1 while you are there"),
            1
        );
        assert_eq!(
            notices(&fx),
            [
                "native_on",
                "request_waiting",
                "native_not_ready",
                "native_off"
            ]
        );
        handle(&ctx, &fx.project, "!stop").unwrap();
        assert_eq!(fx.world.runner.count("agent send-keys w1:p1 esc"), 1);
    }

    #[test]
    fn slash_commands_pass_through_unchanged() {
        let (fx, _) = talk_fixture("idle");
        let ctx = fx.world.ctx();
        handle(&ctx, &fx.project, "/compact\n").unwrap();
        assert_eq!(fx.world.runner.count("agent prompt w1:p1 /compact"), 1);
    }

    fn open_ask(fx: &Fx, reask: Option<&str>) -> crate::contracts::Ask {
        crate::ask::ask(
            &fx.world.ctx(),
            "demo",
            crate::ask::NewAsk {
                question: "keep the experiment running another hour, or stop now?".into(),
                choices: vec!["keep it running".into(), "stop it now".into()],
                what: None,
                means: None,
                round: None,
                reask: reask.map(str::to_string),
            },
        )
        .unwrap()
    }

    #[test]
    fn replay_is_identical_and_sends_nothing() {
        let (fx, _) = talk_fixture("idle");
        let ctx = fx.world.ctx();
        crate::ask::say(
            &ctx,
            "demo",
            "The first lane is done.",
            Some("You can read its report now."),
        )
        .unwrap();
        submit(&ctx, &fx.project, "thanks").unwrap();
        open_ask(&fx, None);
        let calls = fx.world.runner.calls.borrow().len();
        let a = replay(&ctx, "demo").unwrap();
        let b = replay(&ctx, "demo").unwrap();
        assert_eq!(a, b);
        assert_eq!(
            fx.world.runner.calls.borrow().len(),
            calls,
            "replay makes no herdr call"
        );
        assert!(
            a.contains("The first lane is done.")
                && a.contains("thanks")
                && a.contains("sent")
                && a.contains("question"),
            "{a}"
        );
    }

    #[test]
    fn an_unchecked_say_is_not_appended() {
        let fx = fixture();
        assert!(
            crate::ask::say(&fx.world.ctx(), "demo", "Run the zorbulate gate now.", None).is_err()
        );
        assert!(!journal_path(&fx.project).exists());
    }
}
