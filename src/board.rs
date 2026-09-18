//! The build board on the Spaces tab (SPEC-ADE §6 item 14, variant (a)):
//! workspace tokens `ade_stage`, `ade_lanes`, `ade_needs_you`, `ade_last`,
//! `ade_updated`, each from a binary-owned template, at most 80 characters,
//! free of registry names, and published only when the check passes (gate B,
//! D17 item 3). A value that fails is not published and the previous value
//! stays: it is re-sent with a fresh TTL instead.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::MergePhase;
use crate::herdr::Herdr;
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::thread;

/// Token lifetime: a dead ticker lets the rows expire rather than show a
/// stale all-clear. The ticker refreshes well inside it.
pub const TTL: Duration = Duration::from_secs(300);

pub const KEYS: [&str; 5] = [
    "ade_stage",
    "ade_lanes",
    "ade_needs_you",
    "ade_last",
    "ade_updated",
];

/// The fork cuts token values at 80 characters; the board never relies on it.
pub const MAX_VALUE_CHARS: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct BoardState {
    /// The last value published per key.
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    /// The last `say` line or fixed notice for `ade_last`, and when.
    #[serde(default)]
    pub last_say: String,
    #[serde(default)]
    pub last_say_at: String,
}

fn state_path(project: &Project) -> PathBuf {
    project.state_dir().join("board.json")
}

pub fn state(project: &Project) -> BoardState {
    project::read_json(&state_path(project)).unwrap_or_default()
}

fn save_state(project: &Project, change: impl FnOnce(&mut BoardState)) {
    let Ok(_lock) = project.lock() else { return };
    let mut s = state(project);
    change(&mut s);
    let _ = project::write_json(&state_path(project), &s);
}

/// Remembers a published `say` line so the next template pass keeps it.
pub fn remember_last(project: &Project, line: &str) {
    save_state(project, |s| {
        s.last_say = line.to_string();
        s.last_say_at = project::now();
    });
}

/// Gate B for one value: bounded, no control characters, passes the check.
pub fn check_value(project: &Project, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("board_refused: an empty value");
    }
    if value.chars().count() > MAX_VALUE_CHARS {
        bail!(
            "board_refused: {} characters, at most {MAX_VALUE_CHARS}",
            value.chars().count()
        );
    }
    if value.chars().any(char::is_control) {
        bail!("board_refused: control characters");
    }
    crate::glossary::gate(project, value).context("board_refused")
}

fn workspace_herdr<'a>(ctx: &'a Ctx, project: &Project) -> Option<(Herdr<'a>, String)> {
    let coord = project.coordinator()?;
    if coord.socket.is_empty() || coord.workspace_id.is_empty() {
        return None;
    }
    Some((
        Herdr::new(ctx.env.herdr_bin(), &coord.socket, ctx.runner),
        coord.workspace_id,
    ))
}

fn send(ctx: &Ctx, project: &Project, pairs: &[(String, String)]) -> Result<()> {
    let (h, workspace) =
        workspace_herdr(ctx, project).context("board_unavailable: the project is not open")?;
    let ttl = TTL.as_millis().to_string();
    let tokens: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let mut args = vec![
        "workspace",
        "report-metadata",
        workspace.as_str(),
        "--source",
        crate::herdr::SOURCE,
        "--ttl-ms",
        ttl.as_str(),
    ];
    for t in &tokens {
        args.push("--token");
        args.push(t);
    }
    h.call(&args, Duration::from_secs(10))
        .map_err(|e| anyhow::anyhow!("board_unavailable: {}", e.message))?;
    save_state(project, |s| {
        for (k, v) in pairs {
            s.values.insert(k.clone(), v.clone());
        }
    });
    Ok(())
}

/// Publishes one value; a failing value is refused and the old one stays.
pub fn publish_value(ctx: &Ctx, project: &Project, key: &str, value: &str) -> Result<()> {
    if !KEYS.contains(&key) {
        bail!("board_refused: `{key}` is not a board row");
    }
    check_value(project, value)?;
    send(ctx, project, &[(key.to_string(), value.to_string())])
}

/// Age as the checker admits it: `5m`, `2h`, `3d`.
pub fn age(since: &str) -> Option<String> {
    let then: jiff::Timestamp = since.parse().ok()?;
    let secs = jiff::Timestamp::now().as_second() - then.as_second();
    let secs = secs.max(0);
    Some(if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    })
}

/// Runtime state per pane from `agent list`, `None` when herdr is unreadable.
fn agent_states(ctx: &Ctx, project: &Project) -> Option<BTreeMap<String, String>> {
    let coord = project.coordinator()?;
    if coord.socket.is_empty() {
        return None;
    }
    let h = Herdr::new(ctx.env.herdr_bin(), &coord.socket, ctx.runner);
    Some(
        h.agent_list()
            .ok()?
            .into_iter()
            .map(|a| (a.pane_id, a.agent_status))
            .collect(),
    )
}

/// `ade_stage`: the newest round's phase, with its birth sentence if it
/// fits. Also the stage line of `ha overview`.
pub fn stage(project: &Project) -> String {
    let rounds = crate::round::list(project);
    match rounds.last() {
        None => "no round is open yet".to_string(),
        Some(r) => {
            let n = crate::round::round_number(&r.round);
            let merge = crate::round::read_merge(project, &r.round).ok().flatten();
            let members = r.manifest.members.len();
            let done = r
                .manifest
                .members
                .iter()
                .filter(|m| m.pin.is_some())
                .count();
            let phase = match merge.map(|m| m.phase) {
                Some(MergePhase::Checkpointed) => format!("round {n} has landed"),
                Some(MergePhase::MergeDiverged) => format!("round {n} stopped and needs a look"),
                Some(_) => format!("round {n} is being merged"),
                None if r.expected_head.is_some() => format!("round {n} is in review"),
                None if members == 0 => format!("round {n} is open with no lanes yet"),
                None if done == members => format!("round {n} has all {members} lanes done"),
                None => format!("round {n} has {} lanes working", members - done),
            };
            let with = format!("{phase}. {}", r.plain);
            if with.chars().count() <= MAX_VALUE_CHARS && check_value(project, &with).is_ok() {
                with
            } else {
                phase
            }
        }
    }
}

/// The five templated values, before the check.
pub fn compute(ctx: &Ctx, project: &Project) -> Vec<(String, String)> {
    let events = crate::round::sealed_events(project).unwrap_or_default();
    let mut out = Vec::new();

    out.push(("ade_stage".to_string(), stage(project)));

    // ade_lanes: done and waiting from sealed events, working and stuck from
    // runtime state, as separate counts.
    let states = agent_states(ctx, project);
    let (mut working, mut done, mut waiting, mut stuck) = (0, 0, 0, 0);
    for t in thread::list(project) {
        if t.status == thread::Status::Resolved || t.is_remote() {
            continue;
        }
        let attempt = crate::round::thread_attempt(project, &t.id).unwrap_or(1);
        match crate::round::latest_event(&events, &t.id, attempt) {
            Some(e) if e.payload.done.is_some() => done += 1,
            Some(e) if e.payload.waiting.is_some() => waiting += 1,
            _ => {
                let state = states.as_ref().and_then(|s| s.get(&t.pane_id).cloned());
                if t.status == thread::Status::Failed || state.as_deref() == Some("blocked") {
                    stuck += 1;
                } else {
                    working += 1;
                }
            }
        }
    }
    out.push((
        "ade_lanes".to_string(),
        format!("{working} working, {done} done, {waiting} waiting for you, {stuck} stuck"),
    ));

    // ade_needs_you: the compact line of the newest open ask, never from a
    // waiting event and never inferred from idle.
    out.push((
        "ade_needs_you".to_string(),
        crate::ask::newest_open(project)
            .map(|a| crate::ask::compact_line(&a))
            .unwrap_or_else(|| "nothing waits for you".to_string()),
    ));

    // ade_last: the last say line or a templated last event with its age.
    let s = state(project);
    let last_event = events.last().map(|e| {
        let what = if e.payload.done.is_some() {
            "a lane finished"
        } else {
            "a lane asked for help"
        };
        (e.created.clone(), what)
    });
    let last = match (last_event, s.last_say.is_empty()) {
        (Some((at, what)), true) => format!("{what} {} ago", age(&at).unwrap_or_default()),
        (Some((at, what)), false) if at > s.last_say_at => {
            format!("{what} {} ago", age(&at).unwrap_or_default())
        }
        (_, false) => s.last_say.clone(),
        (None, true) => "nothing has happened yet".to_string(),
    };
    out.push(("ade_last".to_string(), last.replace("  ", " ")));

    let now = jiff::Zoned::now().strftime("%H:%M").to_string();
    out.push(("ade_updated".to_string(), format!("updated at {now}")));
    out
}

/// Publishes every row that passes; a row that fails keeps its previous
/// value (re-sent with a fresh TTL). Returns the refused rows.
pub fn refresh(ctx: &Ctx, project: &Project) -> Result<Vec<(String, String)>> {
    if workspace_herdr(ctx, project).is_none() {
        return Ok(Vec::new());
    }
    let previous = state(project).values;
    let mut pairs = Vec::new();
    let mut refused = Vec::new();
    for (key, value) in compute(ctx, project) {
        match check_value(project, &value) {
            Ok(()) => pairs.push((key, value)),
            Err(e) => {
                refused.push((key.clone(), format!("{e:#}")));
                if let Some(old) = previous.get(&key) {
                    pairs.push((key, old.clone()));
                }
            }
        }
    }
    if !pairs.is_empty() {
        send(ctx, project, &pairs)?;
    }
    Ok(refused)
}
