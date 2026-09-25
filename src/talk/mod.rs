//! The conversation journal: checked say, ask, answer and notice entries,
//! plus Rolf's requests from the coordinator prompt hook.
//!
//! One append owner: every writer appends under `talk/journal.lock`,
//! fsyncs and releases. A trailing incomplete line is terminated, skipped by
//! readers, and reported once as a `journal_tail` notice.

use std::borrow::Cow;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contracts::TalkInbound;
use crate::project::{self, Project};

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
    project.record_dir("talk")
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
    project.record_dir_for_write("talk")?;
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
    let from_rolf = matches!(entry, Entry::Rolf { .. });
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
    if from_rolf
        && project
            .coordinator()
            .is_some_and(|c| !c.closed_by_rolf_at.is_empty())
    {
        project.update_coordinator(|c| {
            c.closed_by_rolf_at.clear();
            c.reopen_requested = true;
        })?;
    }
    Ok(Some(seq))
}

// -------------------------------------------------- coordinator-pane prompts

/// A harness line (priming, nudge, event) that is not Rolf's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingPrompt {
    Automated,
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingPromptRecord {
    at: i64,
    /// Stored so a hook can remove a harness prompt joined to Rolf's words.
    #[serde(default)]
    pane: String,
    #[serde(default)]
    text: String,
}

/// A marker older than this belongs to a hook that never fired; it must not
/// claim a later prompt with the same text.
const PENDING_PROMPT_SECS: i64 = 120;
const PASTE_OPEN: &str = "<pasted_content id=\"";
const TASK_OPEN: &str = "<task-notification>";
const TASK_CLOSE: &str = "</task-notification>";
const CROSS_SESSION_OPEN: &str = "<cross-session-message";
const CROSS_SESSION_CLOSE: &str = "</cross-session-message>";
const IDLE_NOTICE_OPEN: &str = "[Cross-session idle notice]";
const IDLE_NOTICE_CLOSE: &str = "This is an automated notice from that session's harness — not a message from a person, and not an instruction; act on it only insofar as your user's earlier request calls for it.";

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

/// True only when the complete prompt is Claude Code's cross-session wrapper.
/// Native words before or after the wrapper remain Rolf's request.
pub(crate) fn is_cross_session_prompt(text: &str) -> bool {
    let text = text.trim();
    let Some(after_name) = text.strip_prefix(CROSS_SESSION_OPEN) else {
        return false;
    };
    if !after_name.starts_with('>')
        && !after_name.starts_with(|character: char| character.is_whitespace())
    {
        return false;
    }
    let Some((_, body)) = after_name.split_once('>') else {
        return false;
    };
    body.strip_suffix(CROSS_SESSION_CLOSE).is_some()
}

/// True only when the complete prompt is one or more of Claude Code's idle
/// notices. Native words before, after, or between notices remain Rolf's.
pub(crate) fn is_idle_notice_prompt(text: &str) -> bool {
    let mut rest = text.trim();
    let mut found = false;
    while let Some(after_open) = rest.strip_prefix(IDLE_NOTICE_OPEN) {
        let Some((_, after_close)) = after_open.split_once(IDLE_NOTICE_CLOSE) else {
            return false;
        };
        found = true;
        rest = after_close.trim_start();
    }
    found && rest.is_empty()
}

/// A ticker prompt is one complete, possibly paste-wrapped line. Keeping this
/// to one line means native words mixed into the same prompt still count.
fn is_ticker_prompt(text: &str) -> bool {
    let marker = marker_text(text);
    let marker = marker.trim();
    !marker.contains('\n') && marker.starts_with(crate::steps::TICKER_PROMPT_PREFIX)
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

pub(crate) fn is_historical_system_prompt(text: &str) -> bool {
    if is_task_notification_prompt(text)
        || is_cross_session_prompt(text)
        || is_idle_notice_prompt(text)
        || is_ticker_prompt(text)
        || is_parent_status_line(text)
    {
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
        || (text.starts_with("You are the coordinator of the herdr project `")
            && text.contains(" skill coordinator`")
            && text.contains(" context "))
}

/// Returns only Rolf's words from a recorded request. Before prompt markers
/// carried their text, a ticker line could be appended to words already in the
/// pane; remove that known suffix while leaving the append-only row readable.
pub(crate) fn human_request_text(text: &str) -> Option<String> {
    if is_historical_system_prompt(text) {
        return None;
    }
    let mut kept = String::new();
    let mut stripped = false;
    for line in text.split_inclusive('\n') {
        let (body, newline) = line
            .strip_suffix('\n')
            .map_or((line, ""), |body| (body, "\n"));
        if let Some(start) = body.find(crate::steps::TICKER_PROMPT_PREFIX) {
            kept.push_str(&body[..start]);
            stripped = true;
        } else {
            kept.push_str(body);
        }
        kept.push_str(newline);
    }
    let kept = if stripped { kept.trim() } else { text };
    if kept.trim().is_empty() || is_historical_system_prompt(kept) {
        None
    } else {
        Some(kept.to_string())
    }
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

/// Visible characters and their ANSI intensity/inverse-video attributes.
/// An unrecognised control sequence means the input cannot be read safely.
struct StyledLine {
    text: String,
    faint: Vec<bool>,
    inverse: Vec<bool>,
}

fn styled_lines(screen: &str) -> Option<Vec<StyledLine>> {
    let mut lines = vec![StyledLine {
        text: String::new(),
        faint: Vec::new(),
        inverse: Vec::new(),
    }];
    let mut chars = screen.chars().peekable();
    let (mut faint, mut inverse) = (false, false);
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.next()? != '[' {
                return None;
            }
            let mut codes = String::new();
            loop {
                let next = chars.next()?;
                if next == 'm' {
                    break;
                }
                if !next.is_ascii_digit() && next != ';' {
                    return None;
                }
                codes.push(next);
            }
            let params: Vec<u16> = codes
                .split(';')
                .map(|code| {
                    if code.is_empty() {
                        Some(0)
                    } else {
                        code.parse().ok()
                    }
                })
                .collect::<Option<_>>()?;
            let mut i = 0;
            while i < params.len() {
                match params[i] {
                    0 => {
                        faint = false;
                        inverse = false;
                    }
                    2 => faint = true,
                    22 => faint = false,
                    7 => inverse = true,
                    27 => inverse = false,
                    38 | 48 | 58 => {
                        i += match params.get(i + 1)? {
                            2 if i + 4 < params.len() => 5,
                            5 if i + 2 < params.len() => 3,
                            _ => return None,
                        };
                        continue;
                    }
                    _ => {}
                }
                i += 1;
            }
        } else if ch == '\n' {
            lines.push(StyledLine {
                text: String::new(),
                faint: Vec::new(),
                inverse: Vec::new(),
            });
        } else {
            if ch.is_control() && ch != '\r' {
                return None;
            }
            let line = lines.last_mut()?;
            line.text.push(ch);
            line.faint.push(faint);
            line.inverse.push(inverse);
        }
    }
    Some(lines)
}

fn separator(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && line.chars().all(|ch| ch == '─')
}

/// Inspect the live editor, not scrollback. Claude's faint (SGR 2) suggestion
/// is not Rolf's text; a normal-intensity character is. Pi's cursor is a
/// single inverse-video space between two coloured separator rows.
pub(crate) fn coordinator_input_clear(screen: &str) -> bool {
    let Some(lines) = styled_lines(screen) else {
        return false;
    };
    if lines.iter().all(|line| line.text.trim().is_empty()) {
        return false;
    }
    // Pi: demand both borders and the cursor, and refuse any printable draft
    // in the middle. A border elsewhere in the transcript is not an editor.
    if lines.windows(3).enumerate().any(|(i, rows)| {
        lines.len().saturating_sub(i) <= 10
            && separator(&rows[0].text)
            && separator(&rows[2].text)
            && rows[1].text.trim().is_empty()
            && rows[1].inverse.iter().any(|inverse| *inverse)
            && lines[i + 3..]
                .iter()
                .all(|line| line.text.trim().is_empty())
    }) {
        return true;
    }
    // Claude's status, shell and background-agent rows follow the lower
    // rule. Only text inside the two editor rules can be a draft.
    let borders: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| separator(&line.text).then_some(i))
        .collect();
    if let Some(&bottom) = borders.last()
        && let Some(&top) = borders.iter().rev().nth(1)
        && lines[top + 1..bottom].iter().any(|line| {
            ['❯', '›', '>', '⟩']
                .iter()
                .any(|mark| line.text.trim_start().starts_with(*mark))
        })
    {
        return lines[top + 1..bottom].iter().all(|line| {
            let trimmed = line.text.trim_start();
            let marker = ['❯', '›', '>', '⟩']
                .iter()
                .find(|mark| trimmed.starts_with(**mark));
            let start = marker.map_or(0, |mark| line.text.len() - trimmed.len() + mark.len_utf8());
            line.text
                .char_indices()
                .zip(&line.faint)
                .all(|((byte, ch), faint)| byte < start || ch.is_whitespace() || *faint)
        });
    }
    let Some((index, line)) = lines.iter().enumerate().rev().find(|(_, line)| {
        ["❯", "›", ">", "⟩"]
            .iter()
            .any(|mark| line.text.trim_start().starts_with(mark))
    }) else {
        return false;
    };
    if lines.len().saturating_sub(index) > 8 {
        return false;
    }
    let Some(marker) = line.text.chars().position(|ch| !ch.is_whitespace()) else {
        return false;
    };
    let draft = line
        .text
        .trim_start()
        .trim_start_matches(['❯', '›', '>', '⟩'])
        .trim();
    let faint_suggestion = !draft.is_empty()
        && line
            .text
            .chars()
            .zip(&line.faint)
            .skip(marker + 1)
            .filter(|(ch, _)| !ch.is_whitespace())
            .all(|(_, faint)| *faint);
    let codex_hint = line.text.chars().nth(marker) == Some('›')
        && matches!(
            draft,
            "Ask Codex to do anything"
                | "Try \"debug this error\""
                | "Use /skills to list available skills"
        );
    let placeholder = draft.is_empty() || faint_suggestion || codex_hint;
    // Claude can render extra status rows below the editor (compaction,
    // background shells and the working index). They are not draft text.
    let status = &lines[index + 1..];
    placeholder
        && status.iter().all(|line| {
            let row = line.text.trim();
            row.is_empty()
                || separator(row)
                || row.contains(" · ")
                || row.ends_with("until auto-compact") && row.contains('%')
                || row.split_once(' ').is_some_and(|(n, rest)| {
                    n.parse::<u32>().is_ok() && matches!(rest, "shell" | "shells")
                })
                || row.ends_with("index")
                    && status
                        .iter()
                        .any(|line| line.text.contains("until auto-compact"))
        })
}

/// A previous automated line left in the editor is not Rolf's draft. Only
/// exact live markers for this pane count; never infer ownership from a prefix.
pub(crate) fn coordinator_prompt_clear(
    project: &Project,
    herdr: &crate::herdr::Herdr<'_>,
    pane: &str,
) -> Result<bool> {
    let screen = herdr.pane_read_ansi(pane, "visible")?;
    if coordinator_input_clear(&screen) {
        clear_input_hold(project);
        return Ok(true);
    }
    let dir = talk_dir(project).join("prompts");
    let entries = std::fs::read_dir(dir).ok();
    let now = jiff::Timestamp::now().as_second();
    let lines = styled_lines(&screen).unwrap_or_default();
    // A marker in scrollback does not own a newer draft. Only the last editor
    // line can be a leftover, with no other text following it.
    let editor = lines.iter().enumerate().rev().find(|(_, line)| {
        ['❯', '›', '>', '⟩']
            .iter()
            .any(|mark| line.text.trim_start().starts_with(*mark))
    });
    if let Some((_, line)) = editor.filter(|(index, _)| {
        lines.len() - index <= 8
            && lines[index + 1..]
                .iter()
                .all(|following| following.text.trim().is_empty())
    }) {
        let input = line
            .text
            .trim()
            .trim_start_matches(['❯', '›', '>', '⟩'])
            .trim();
        for entry in entries.into_iter().flatten().flatten() {
            let Some(record) = project::read_json::<PendingPromptRecord>(&entry.path()) else {
                continue;
            };
            if record.pane == pane
                && now - record.at <= PENDING_PROMPT_SECS
                && !record.text.is_empty()
                && input == record.text
            {
                clear_input_hold(project);
                return Ok(true);
            }
        }
    }
    hold_input(project)?;
    Ok(false)
}

fn input_hold_path(project: &Project) -> PathBuf {
    project.state_dir().join("coordinator-input-hold.json")
}

fn clear_input_hold(project: &Project) {
    let _ = std::fs::remove_file(input_hold_path(project));
}

fn hold_input(project: &Project) -> Result<()> {
    let path = input_hold_path(project);
    if !path.exists() {
        project::write_json(&path, &jiff::Timestamp::now().as_second())?;
    }
    Ok(())
}

pub(crate) fn long_input_hold(project: &Project) -> bool {
    project::read_json::<i64>(&input_hold_path(project))
        .is_some_and(|since| jiff::Timestamp::now().as_second() - since >= 30 * 60)
}

/// Marks a harness prompt so an exact leftover in the editor is recognised
/// and the prompt-submit hook does not record it as Rolf's request.
pub(crate) fn mark_automated_prompt(project: &Project, pane: &str, text: &str) -> Result<()> {
    // Check the hook binding as before, but keep the marker even for kinds
    // without a submit hook: their unfinished automated drafts need ownership.
    let _ = crate::hook::captures(project, pane)?;
    project.record_dir_for_write("talk")?;
    let path = pending_prompt_path(project, pane, text);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    project::write_json(
        &path,
        &PendingPromptRecord {
            at: jiff::Timestamp::now().as_second(),
            pane: pane.to_string(),
            text: marker_text(text).trim_end().to_string(),
        },
    )
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
    Some(PendingPrompt::Automated)
}

fn remove_marked_once(prompt: &str, marked: &str) -> Option<String> {
    let mut search = 0;
    while let Some(relative) = prompt[search..].find(PASTE_OPEN) {
        let start = search + relative;
        let after_open = &prompt[start + PASTE_OPEN.len()..];
        let Some((id, after_id)) = after_open.split_once("\">") else {
            break;
        };
        if id.is_empty() || id.contains(['<', '>', '"']) {
            search = start + PASTE_OPEN.len();
            continue;
        }
        let close = format!("</pasted_content id=\"{id}\">");
        let Some((content, after_close)) = after_id.split_once(&close) else {
            break;
        };
        if content.trim() == marked.trim() {
            let end = prompt.len() - after_close.len();
            let mut result = prompt.to_string();
            result.replace_range(start..end, "");
            return Some(result);
        }
        search = prompt.len() - after_close.len();
    }

    let start = prompt.find(marked)?;
    let mut result = prompt.to_string();
    result.replace_range(start..start + marked.len(), "");
    Some(result)
}

/// Consumes every live automated marker whose sent text occurs in this hook
/// prompt and returns the words that were already present in the pane.
pub(crate) fn take_automated_parts(project: &Project, pane: &str, text: &str) -> String {
    let dir = talk_dir(project).join("prompts");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return text.to_string();
    };
    let now = jiff::Timestamp::now().as_second();
    let mut markers = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(record) = project::read_json::<PendingPromptRecord>(&path) else {
            continue;
        };
        if now - record.at > PENDING_PROMPT_SECS {
            let _ = std::fs::remove_file(path);
            continue;
        }
        if record.pane == pane && !record.text.is_empty() && text.contains(&record.text) {
            markers.push((record.text, path));
        }
    }
    markers.sort_by_key(|marker| std::cmp::Reverse(marker.0.len()));
    let mut remainder = text.to_string();
    for (marked, path) in markers {
        let mut removed = false;
        while let Some(stripped) = remove_marked_once(&remainder, &marked) {
            remainder = stripped;
            removed = true;
        }
        if removed {
            let _ = std::fs::remove_file(path);
        }
    }
    remainder.trim().to_string()
}

// ---------------------------------------------------------------- writer

/// The serialized writer lock for coordinator prompts.
pub(crate) struct WriterLock {
    _lock: Locked,
}

pub(crate) fn writer_lock(project: &Project) -> Result<WriterLock> {
    Ok(WriterLock {
        _lock: lock_file(project, "writer.lock")?,
    })
}

/// Rolf's exact words for one durable request id.
pub(crate) fn request_text(project: &Project, id: &str) -> Option<String> {
    read(project)
        .lines
        .into_iter()
        .rev()
        .find_map(|line| match line.entry {
            Entry::Rolf { request, text, .. } if request == id => human_request_text(&text),
            _ => None,
        })
}

/// A request resolved against either the current project or an explicitly
/// named project in the same ADE root.
pub(crate) struct ResolvedRequest {
    pub(crate) project: String,
    pub(crate) id: String,
    pub(crate) text: String,
    qualified: bool,
}

impl ResolvedRequest {
    /// Keep local shorthand local, but retain an explicitly qualified source.
    pub(crate) fn basis(&self) -> String {
        if self.qualified {
            format!("request:{}/{}", self.project, self.id)
        } else {
            format!("request:{}", self.id)
        }
    }

    /// Coordinator recipe records are always portable across projects.
    pub(crate) fn qualified_basis(&self) -> String {
        format!("request:{}/{}", self.project, self.id)
    }
}

/// Resolve `request:<id>` or `request:<project>/<id>` once for every writer
/// and reader of request authority.
pub(crate) fn resolve_request(project: &Project, basis: &str) -> Result<ResolvedRequest> {
    let reference = basis
        .strip_prefix("request:")
        .filter(|value| !value.is_empty())
        .with_context(|| {
            format!("request_authority: `{basis}` is not request:<id> or request:<project>/<id>")
        })?;
    let (slug, id, qualified) = match reference.split_once('/') {
        Some((slug, id)) => (slug, id, true),
        None => (project.slug.as_str(), reference, false),
    };
    if slug.is_empty() || id.is_empty() || id.contains('/') {
        bail!("request_authority: no request `{id}` in project `{slug}`");
    }
    let source = Project::load(&project.root, slug)
        .map_err(|_| anyhow::anyhow!("request_authority: no request `{id}` in project `{slug}`"))?;
    let text = request_text(&source, id)
        .with_context(|| format!("request_authority: no request `{id}` in project `{slug}`"))?;
    Ok(ResolvedRequest {
        project: slug.to_string(),
        id: id.to_string(),
        text,
        qualified,
    })
}

pub(crate) fn recent_requests(project: &Project, limit: usize) -> Vec<(String, String)> {
    let journal = read(project);
    let mut found: Vec<(String, String)> = journal
        .lines
        .iter()
        .filter_map(|line| match &line.entry {
            Entry::Rolf { request, text, .. } => {
                human_request_text(text).map(|text| (request.clone(), text))
            }
            _ => None,
        })
        .collect();
    if found.len() > limit {
        found.drain(..found.len() - limit);
    }
    found
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::testkit::fixture;

    #[test]
    fn live_claude_suggestions_and_pi_cursor_are_empty_editors() {
        // Visible ANSI bytes captured from idle Mac coordinator panes.
        assert!(coordinator_input_clear(
            "❯ \x1b[0m\x1b[2mshow me the twelve\x1b[0m\n"
        ));
        assert!(coordinator_input_clear(
            "❯ \x1b[0m\x1b[2many news?\x1b[0m\n"
        ));
        let border = "\x1b[38;2;178;148;187m────────────────\x1b[0m";
        let pi = format!("{border}\n\x1b[0m\x1b[7m \x1b[0m   \n{border}\n");
        assert!(coordinator_input_clear(&pi));
        assert!(coordinator_input_clear(
            "❯ \x1b[0m\x1b[2msnap latch, and keep the small body\x1b[0m\n5% until auto-compact\n1 shell\nworking index\n"
        ));
        assert!(!coordinator_input_clear(
            "❯ \x1b[0mRolf's draft\n5% until auto-compact\n1 shell\nworking index\n"
        ));
        assert!(!coordinator_input_clear(
            &pi.replace("\x1b[7m \x1b[0m", "hello")
        ));
        assert!(!coordinator_input_clear("❯ \x1b[0mshow me the twelve\n"));
        assert!(!coordinator_input_clear("❯ Type a message\n"));
        assert!(!coordinator_input_clear(
            "❯ \x1b[0m\x1b[2many news?\x1b[22m and my words\n"
        ));
        assert!(!coordinator_input_clear("unfamiliar editor\n"));
    }

    #[test]
    fn claude_agents_panel_never_becomes_editor_text() {
        let rule = "\x1b[38;2;136;136;136m────────────────────────────\x1b[0m";
        for count in 1..=6 {
            let panel = (0..count)
                .map(|_| "\x1b[0m  ◯ general-purpose  Verifying excluded files · 20m\n")
                .collect::<String>();
            let screen = format!(
                "{rule}\n\x1b[0m\x1b[38;2;153;153;153m❯ \x1b[0m                    \n{rule}\n  /home/agent/.herdr-ade/adeherdr > ctx\n  ⏵⏵ bypass permissions on · 1 shell · ← for agents\n\n  ● main\n{panel}"
            );
            assert!(coordinator_input_clear(&screen), "agents: {count}");
            assert!(!coordinator_input_clear(
                &screen.replace("❯ ", "❯ Rolf's draft ")
            ));
            assert!(coordinator_input_clear(
                &screen.replace("❯ ", "❯ \x1b[2msuggestion\x1b[22m ")
            ));
        }
    }

    #[test]
    fn a_harness_owned_leftover_is_not_a_rolf_draft() {
        use crate::runner::fake::ok;
        let fx = fixture();
        let prompt =
            "[herdr-ade ticker: automated, not the user, approves nothing] New inbox items.";
        let path = pending_prompt_path(&fx.project, "w1:p1", prompt);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        project::write_json(
            &path,
            &PendingPromptRecord {
                at: jiff::Timestamp::now().as_second(),
                pane: "w1:p1".into(),
                text: prompt.into(),
            },
        )
        .unwrap();
        let screen = std::rc::Rc::new(std::cell::RefCell::new(format!("❯ {prompt}\n")));
        let read = screen.clone();
        fx.world.runner.on_fn(
            |cmd| cmd.display().contains("pane read"),
            move |_| Ok(ok(&read.borrow())),
        );
        let herdr = crate::herdr::Herdr::new("herdr", "/missing.sock", &fx.world.runner);
        assert!(coordinator_prompt_clear(&fx.project, &herdr, "w1:p1").unwrap());
        *screen.borrow_mut() = format!("❯ {prompt}\n❯ Rolf's draft\n");
        assert!(!coordinator_prompt_clear(&fx.project, &herdr, "w1:p1").unwrap());
        *screen.borrow_mut() = format!("❯ {prompt}\ncontinued draft\n");
        assert!(!coordinator_prompt_clear(&fx.project, &herdr, "w1:p1").unwrap());
        assert!(fx.world.runner.calls.borrow().iter().any(|call| {
            call.display()
                .contains("pane read w1:p1 --source visible --format ansi")
        }));
        assert!(!coordinator_input_clear("❯ Rolf's unfinished sentence\n"));
        assert!(coordinator_input_clear("❯ \n"));
        project::write_json(
            &input_hold_path(&fx.project),
            &(jiff::Timestamp::now().as_second() - 31 * 60),
        )
        .unwrap();
        assert!(long_input_hold(&fx.project));
        let ctx = fx.world.ctx();
        let board = crate::board::compute(&ctx, &fx.project);
        assert!(
            board
                .iter()
                .any(|(key, value)| key == "ade_needs_you" && value.contains("30 minutes"))
        );
        let context = crate::coordinator::digest(&ctx, &fx.project, "ha")
            .unwrap()
            .0;
        assert!(context.contains("over 30 minutes"));
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
        assert!(
            j.lines
                .iter()
                .any(|l| matches!(&l.entry, Entry::Notice { id } if id == "journal_tail"))
        );
        let seqs: Vec<u64> = j.lines.iter().map(|l| l.seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4]);
    }

    #[test]
    fn a_keyed_entry_is_appended_once_and_oversize_is_refused() {
        let fx = fixture();
        let e = || Entry::Notice {
            id: "journal_tail".into(),
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
}
