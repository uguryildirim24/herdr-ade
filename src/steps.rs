//! The ticker's per-project steps: delivery and messages.
//! Thread facts update their owning records; only messages
//! without a thread or review home enter the inbox.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::herdr::{Agent, Herdr, Pane};
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::thread::{self, Status, Thread};
use crate::{events, inbox};

pub(crate) mod goal_check;

pub(crate) const TICKER_PROMPT_PREFIX: &str =
    "[herdr-ade ticker: automated, not the user, approves nothing]";
pub(crate) const DONE_RETENTION_DAYS: u64 = 30;

/// A transport submission is not a context receipt. Keep the unsighted lines
/// until the bound coordinator actually asks for its context.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(from = "WakeHistory")]
struct WakeCursor {
    revision: u64,
    notices: Vec<HeardNotice>,
    pending_announced: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct HeardNotice {
    entry: OutboxEntry,
    binding: String,
}

// Aligned vectors exist only at the historical read boundary, never at runtime.
#[derive(Default, Deserialize)]
#[serde(default)]
struct WakeHistory {
    revision: u64,
    notices: Vec<HeardNotice>,
    pending_announced: bool,
    lines: Vec<String>,
    heard_bindings: Vec<String>,
    binding: String,
    primed_binding: String,
}

impl From<WakeHistory> for WakeCursor {
    fn from(old: WakeHistory) -> Self {
        let binding = if old.primed_binding.is_empty() {
            old.binding
        } else {
            old.primed_binding
        };
        let mut notices = old.notices;
        notices.extend(old.lines.into_iter().enumerate().map(|(i, line)| {
            HeardNotice {
                entry: OutboxEntry::new(NoticeSource::Transition(line.clone()), &line),
                binding: old
                    .heard_bindings
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| binding.clone()),
            }
        }));
        Self {
            revision: old.revision,
            notices,
            pending_announced: old.pending_announced,
        }
    }
}

fn wake_path(project: &Project) -> std::path::PathBuf {
    project.state_dir().join("wake-cursor.json")
}

fn wake_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn wake_binding(project: &Project) -> String {
    project.coordinator().map_or(String::new(), |c| {
        format!("{}:{}:{}", c.socket, c.pane_id, c.attempt())
    })
}

pub(crate) fn wake_revision(project: &Project) -> u64 {
    project::read_json::<WakeCursor>(&wake_path(project))
        .unwrap_or_default()
        .revision
}

/// Only the exact bound pane's context read can consume the pending wake.
pub(crate) fn receipt(project: &Project, observed: u64) -> Result<()> {
    let _lock = project.lock()?;
    let mut cursor: WakeCursor = project::read_json(&wake_path(project)).unwrap_or_default();
    if cursor.revision != observed {
        return Ok(());
    }
    cursor.notices.clear();
    cursor.revision += 1;
    cursor.pending_announced = false;
    project::write_json(&wake_path(project), &cursor)
}

fn announce_pending(project: &Project, cursor: &WakeCursor) -> Result<()> {
    if cursor.pending_announced {
        return Ok(());
    }
    inbox::write(
        project,
        "wake_pending",
        &project.slug,
        &format!(
            "wake_pending: {} has unread transitions; resume the bound coordinator with ha open {}.",
            project.slug, project.slug
        ),
        "",
    )?;
    let _lock = project.lock()?;
    let mut latest: WakeCursor = project::read_json(&wake_path(project)).unwrap_or_default();
    if latest.revision == cursor.revision {
        latest.pending_announced = true;
        project::write_json(&wake_path(project), &latest)?;
    }
    Ok(())
}

/// Prime only transitions this binding never heard. Missing a context receipt
/// is not new news and must not wake the same binding again.
fn prime_unread(ctx: &Ctx, project: &Project) -> Result<()> {
    let binding = wake_binding(project);
    let cursor: WakeCursor = project::read_json(&wake_path(project)).unwrap_or_default();
    let unheard: Vec<_> = cursor
        .notices
        .iter()
        .filter(|notice| notice.binding != binding)
        .map(|notice| notice.entry.line.as_str())
        .collect();
    if unheard.is_empty() {
        return Ok(());
    }
    let line = format!(
        "Unread transitions since the last ha context ({}): {}. Run ha context {} to consume them.",
        unheard.len(),
        unheard.join(" | "),
        project.slug
    );
    // The digest shares batching, but must not become another unread line.
    enqueue(
        project,
        OutboxEntry::new(
            NoticeSource::Digest {
                revision: cursor.revision,
                binding,
            },
            &line,
        ),
        wake_now(),
    )?;
    if !flush_coordinator_notices(ctx, project)? {
        announce_pending(project, &cursor)?;
    }
    Ok(())
}

/// A transition-owned wake-up, independent of the lane's eventual status.
/// `submitted` means accepted by the durable outbox (which owns transport).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub(crate) struct Notice {
    pub(crate) line: String,
    pub(crate) submitted: bool,
}

pub(crate) fn deliver_transition_notices(ctx: &Ctx, project: &Project) -> Result<()> {
    let reviews = crate::review::list(project)?;
    let holds = crate::review::hold_notices(project)?;
    let lanes = thread::list_live(project);
    for (index, notice) in holds.iter().enumerate().filter(|(_, n)| !n.submitted) {
        enqueue(
            project,
            OutboxEntry::new(
                NoticeSource::Transition(format!("hold:{index}")),
                &notice.line,
            ),
            wake_now(),
        )?;
        crate::review::mark_hold_submitted(project, index)?;
    }
    for old in reviews {
        let Some(_lock) = crate::review::try_operation_lock(ctx, &old.repo)? else {
            continue;
        };
        let review = crate::review::load(project, &old.id)?;
        for notice in review.notices.iter().filter(|n| !n.submitted) {
            enqueue(
                project,
                OutboxEntry::new(
                    NoticeSource::Transition(format!("review:{}:{}", review.id, notice.line)),
                    &notice.line,
                ),
                wake_now(),
            )?;
            crate::review::mark_notice_submitted(project, &review.id, &notice.line)?;
        }
    }
    for lane in lanes {
        for index in 0..lane.start_notices.len() {
            if !lane.start_notices[index].submitted {
                enqueue(
                    project,
                    OutboxEntry::new(
                        NoticeSource::Transition(format!("lane:{}:{index}", lane.id)),
                        &lane.start_notices[index].line,
                    ),
                    wake_now(),
                )?;
                thread::update(project, &lane.id, |t| {
                    if let Some(notice) = t.start_notices.get_mut(index) {
                        notice.submitted = true;
                    }
                })?;
            }
        }
    }
    flush_coordinator_notices(ctx, project)?;
    Ok(())
}

pub(crate) fn short_error(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect::<String>()
        .trim()
        .to_string()
}
const DEFAULT_OUTAGE_SECS: i64 = 600;

/// `.state/ticker.json`: what the ticker compared against last time.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub(crate) struct State {
    /// Hashes of files a `config-error` item was already written for.
    pub(crate) config_errors: BTreeSet<String>,
    /// Hash of the unseen inbox item ids last announced.
    pub(crate) announced: String,
    pub(crate) session_item_written: bool,
    /// Coordinator pane whose live lanes were last reconciled by the ticker.
    pub(crate) lanes_parented_to: String,
}

pub(crate) fn load_state(project: &Project) -> State {
    project::read_json(&project.state_dir().join("ticker.json")).unwrap_or_default()
}

/// Only the ticker writes this file, so its own read-modify-write is safe; the
/// write still happens under the project lock, like every `.state/` write.
pub(crate) fn save_state(project: &Project, state: &State) -> Result<()> {
    let _lock = project.lock()?;
    project::write_json(&project.state_dir().join("ticker.json"), state)
}

/// Delivers every sealed event not already submitted or durably queued. The event, not the typed line or report hash, is authoritative.
/// The outbox preserves seal order even while a draft holds transport.
/// An event whose journal already holds `queued` or `submitted` is not typed
/// again; one read before its line went out (`acknowledged` or `handled` with
/// no `submitted`) is still typed once, so the wake-up always happens. An
/// event for a superseded lane attempt is left as it is, sealed and
/// undelivered.
pub(crate) fn deliver_events(ctx: &Ctx, project: &Project) -> Result<()> {
    let mut first: Option<anyhow::Error> = None;
    let sealed = crate::events::for_unresolved_threads(project);
    // Prompt order is serialized, but adoption is not: a held notice must
    // never delay reports from later seals in the same pass.
    let mut latest = std::collections::BTreeMap::new();
    for event in &sealed {
        if event.payload.done.is_some() {
            let key = (&event.thread, event.attempt);
            let sequence = event
                .id
                .rsplit('-')
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            if latest
                .get(&key)
                .is_none_or(|(number, _)| sequence >= *number)
            {
                latest.insert(key, (sequence, event));
            }
        }
    }
    for (_, event) in latest.values() {
        if let Err(error) = adopt_report(project, event) {
            first.get_or_insert(error.context(format!("adopt event {}", event.id)));
        }
    }
    deliver_transition_notices(ctx, project)?;
    for event in sealed {
        if event.payload.failed.is_some() {
            continue;
        }
        // A changed code lane joins the review pile. The review record owns
        // the wake-up, not the member's or reviewer's DONE seal.
        if pile_member(project, &event)
            || (event.payload.done.is_some()
                && thread::load(project, &event.thread).is_ok_and(|lane| lane.role == "reviewer"))
        {
            continue;
        }
        // A resolved lane is finished: its delivery journal is never replayed.
        if thread::load(project, &event.thread).is_ok_and(|lane| lane.status == Status::Resolved) {
            continue;
        }
        if thread::load(project, &event.thread)
            .is_ok_and(|lane| lane.attempt.max(1) != event.attempt)
        {
            continue;
        }
        if let Err(error) = deliver_notice(ctx, project, &event) {
            first.get_or_insert(error.context(format!("event {}", event.id)));
            break;
        }
    }
    if let Err(error) = flush_coordinator_notices(ctx, project) {
        first.get_or_insert(error);
    }
    if let Err(error) = prime_unread(ctx, project) {
        first.get_or_insert(error);
    }
    first.map_or(Ok(()), Err)
}

pub(crate) fn deliver_event(
    ctx: &Ctx,
    project: &Project,
    event: &crate::contracts::Event,
) -> Result<()> {
    adopt_report(project, event)?;
    if pile_member(project, event)
        || (event.payload.done.is_some()
            && thread::load(project, &event.thread).is_ok_and(|lane| lane.role == "reviewer"))
    {
        return Ok(());
    }
    deliver_notice(ctx, project, event)
}

fn pile_member(project: &Project, event: &crate::contracts::Event) -> bool {
    let Some(done) = &event.payload.done else {
        return false;
    };
    thread::load(project, &event.thread).is_ok_and(|lane| {
        lane.role != "reviewer"
            && !lane.repo.is_empty()
            && (lane.changes_seal == event.id)
                .then_some(lane.has_changes)
                .flatten()
                .or(done.has_changes)
                .unwrap_or(done.sha != lane.base)
    })
}

fn deliver_notice(ctx: &Ctx, project: &Project, event: &crate::contracts::Event) -> Result<()> {
    let states = crate::events::states(project, &event.id)?;
    if states.contains(&crate::contracts::DeliveryState::Submitted)
        || states.contains(&crate::contracts::DeliveryState::Queued)
    {
        return Ok(());
    }
    let lane = thread::load(project, &event.thread)?;
    if lane.attempt.max(1) != event.attempt {
        bail!(
            "stale_attempt: event {} is not for the current lane attempt",
            event.id
        );
    }
    // A box lane's DONE is not typed until the Mac itself holds the pinned code
    // commit: fetch the lane branch from the configured GitHub URL and check
    // the event's sha is reachable there (SPEC-remote §4.3, gate R5).
    if lane.is_remote()
        && let Some(done) = &event.payload.done
    {
        verify_published_sha(
            ctx,
            project,
            &lane,
            &done.sha,
            done.published_ref.as_deref(),
        )?;
    }

    // A box lane's tokens were set by the box's own `ha done`; the Mac has no
    // socket into that server, so delivery never projects them again
    // (SPEC-remote §4.3).
    if !lane.is_remote()
        && !lane.pane_id.is_empty()
        && let Some(coordinator) = project.coordinator()
    {
        let herdr = Herdr::new(ctx.env.herdr_bin(), &coordinator.socket, ctx.runner);
        let _ = herdr.pane_clear_tokens(&lane.pane_id, &["done", "waiting"]);
        let value = event
            .payload
            .waiting
            .as_ref()
            .map(|waiting| waiting.text.chars().take(80).collect::<String>())
            .unwrap_or_else(|| "1".into());
        let token = if event.payload.done.is_some() {
            "done"
        } else {
            "waiting"
        };
        let _ = herdr.pane_report_tokens(
            &lane.pane_id,
            &[("lane", &event.thread), (token, &value)],
            crate::coordinator::TOKEN_TTL,
        );
    }

    // A seal can overtake a queued correction. Store a stable event-bound
    // notice even when the coordinator is offline, and say it in the wake-up.
    let queued: Vec<_> = if event.payload.done.is_some() {
        lane.follow_ups
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                f.attempt == event.attempt
                    && (matches!(
                        f.state,
                        crate::thread::FollowUpState::Queued
                            | crate::thread::FollowUpState::Uncertain
                    ) || f.state == crate::thread::FollowUpState::Delivered
                        && f.after_seal == event.id)
            })
            .map(|(i, f)| {
                format!(
                    "#{} ({})",
                    i + 1,
                    f.text.split_whitespace().collect::<Vec<_>>().join(" ")
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    if !queued.is_empty() {
        inbox::write_event(
            project,
            event,
            "follow-up-pending",
            &format!(
                "{} sealed before follow-up {} landed; hold until idle verification (changed work needs a new seal)",
                event.thread,
                queued.join("; ")
            ),
        )?;
    }
    let mut line = crate::events::typed_line(project, event)?;
    if !queued.is_empty() {
        line.push_str(&format!(
            " Follow-up overtook this seal: {}. It can be restored at idle if HEAD, tree and report are unchanged; changed work needs a new seal.",
            queued.join("; ")
        ));
    }
    enqueue(
        project,
        OutboxEntry::new(NoticeSource::Event(event.id.clone()), &line),
        wake_now(),
    )?;
    flush_coordinator_notices(ctx, project)?;
    Ok(())
}

/// Durable outbox shared by automated notices. Source journals advance after
/// acceptance here; a held batch survives seals, cleanup and rebinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum NoticeSource {
    Transition(String),
    Event(String),
    Digest { revision: u64, binding: String },
    Goal(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OutboxEntry {
    source: NoticeSource,
    line: String,
}

impl OutboxEntry {
    fn new(source: NoticeSource, line: &str) -> Self {
        Self {
            source,
            line: line.into(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(from = "BatchHistory")]
struct NoticeBatch {
    first_at: u64,
    entries: Vec<OutboxEntry>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct BatchHistory {
    first_at: u64,
    entries: Vec<OutboxEntry>,
    lines: Vec<String>,
    events: Vec<String>,
    wake_lines: Vec<String>,
    digests: Vec<u64>,
    goals: Vec<(String, String)>,
}

impl From<BatchHistory> for NoticeBatch {
    fn from(old: BatchHistory) -> Self {
        let mut entries = old.entries;
        let mut digests = old.digests.into_iter();
        let mut events = old.events;
        for line in old.lines {
            let event = events.iter().position(|id| {
                line.split_whitespace()
                    .nth(1)
                    .is_some_and(|thread| id.starts_with(&format!("{thread}-")))
            });
            let source = if let Some(index) = event {
                NoticeSource::Event(events.remove(index))
            } else if !old.wake_lines.contains(&line)
                && let Some(revision) = digests.next()
            {
                NoticeSource::Digest {
                    revision,
                    binding: String::new(),
                }
            } else {
                NoticeSource::Transition(line.clone())
            };
            entries.push(OutboxEntry::new(source, &line));
        }
        entries.extend(
            old.goals
                .into_iter()
                .map(|(token, line)| OutboxEntry::new(NoticeSource::Goal(token), &line)),
        );
        Self {
            first_at: old.first_at,
            entries,
        }
    }
}

fn batch_path(project: &Project) -> std::path::PathBuf {
    project.state_dir().join("notice-batch.json")
}

fn save_batch(project: &Project, batch: &NoticeBatch) -> Result<()> {
    let _lock = project.lock()?;
    project::write_json(&batch_path(project), batch)
}

pub(crate) fn deliver_coordinator_prompt(
    project: &Project,
    herdr: &Herdr<'_>,
    pane: &str,
    line: &str,
) -> Result<bool> {
    coordinator_notice_at(project, herdr, pane, Some(line), wake_now())
}

pub(crate) fn deliver_goal_check(
    project: &Project,
    herdr: &Herdr<'_>,
    pane: &str,
    token: &str,
    line: &str,
) -> Result<bool> {
    enqueue(
        project,
        OutboxEntry::new(NoticeSource::Goal(token.into()), line),
        wake_now(),
    )?;
    flush_notices_at(project, herdr, pane, wake_now())?;
    Ok(true)
}

fn coordinator_notice_at(
    project: &Project,
    herdr: &Herdr<'_>,
    pane: &str,
    line: Option<&str>,
    now: u64,
) -> Result<bool> {
    if let Some(line) = line {
        enqueue(
            project,
            OutboxEntry::new(NoticeSource::Transition(line.into()), line),
            now,
        )?;
    }
    flush_notices_at(project, herdr, pane, now)?;
    Ok(true)
}

fn enqueue(project: &Project, entry: OutboxEntry, now: u64) -> Result<()> {
    let _writer = crate::prompt::writer_lock(project)?;
    let mut batch: NoticeBatch = project::read_json(&batch_path(project)).unwrap_or_default();
    if !batch.entries.iter().any(|old| old.source == entry.source) {
        if batch.entries.is_empty() {
            batch.first_at = now;
        }
        batch.entries.push(entry.clone());
        save_batch(project, &batch)?;
    }
    match &entry.source {
        NoticeSource::Event(id) => {
            events::append_delivery(project, id, crate::contracts::DeliveryState::Queued)
        }
        NoticeSource::Goal(token) => goal_check::queued(project, token),
        _ => Ok(()),
    }
}

fn flush_notices_at(project: &Project, herdr: &Herdr<'_>, pane: &str, now: u64) -> Result<bool> {
    if project.coordinator().is_some_and(|record| {
        crate::adapters::dependency_waiting(
            &project.root,
            crate::contracts::MACHINE_LOCAL,
            &record.launch,
        )
    }) {
        return Ok(false);
    }
    let _writer = crate::prompt::writer_lock(project)?;
    let Some(agent) = herdr.agent_list()?.into_iter().find(|a| {
        project.coordinator().is_some_and(|c| {
            c.closed_by_rolf_at.is_empty()
                && c.pane_id == pane
                && crate::coordinator::agent_matches(&c, a)
        }) && a.promptable()
    }) else {
        return Ok(false);
    };
    // Never alter or submit Rolf's draft, whether idle or mid-turn.
    if !crate::prompt::coordinator_prompt_clear(project, herdr, pane)? {
        return Ok(false);
    }
    let mut batch: NoticeBatch = project::read_json(&batch_path(project)).unwrap_or_default();
    let binding = wake_binding(project);
    let mut cursor: WakeCursor = project::read_json(&wake_path(project)).unwrap_or_default();
    let before = batch.entries.len();
    batch.entries.retain(|entry| match &entry.source {
        NoticeSource::Goal(token) => goal_check::current(project, token),
        NoticeSource::Digest {
            binding: target,
            revision,
        } => (target.is_empty() || target == &binding) && *revision == cursor.revision,
        _ => true,
    });
    if batch.entries.len() != before {
        save_batch(project, &batch)?;
    }
    if !batch.entries.is_empty() && (!agent.ready() || now.saturating_sub(batch.first_at) >= 120) {
        let text = batch
            .entries
            .iter()
            .map(|entry| entry.line.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        crate::prompt::mark_automated_prompt(project, pane, &text)?;
        if let Err(error) = herdr.agent_prompt(pane, &text) {
            // Acceptance into the outbox is durable even if transport fails.
            // Keep the batch; sources must not enqueue it again on retry.
            eprintln!("{}: notice batch will retry: {error}", project.slug);
            return Ok(true);
        }
        for entry in &batch.entries {
            match &entry.source {
                NoticeSource::Event(id) => events::append_delivery(
                    project,
                    id,
                    crate::contracts::DeliveryState::Submitted,
                )?,
                NoticeSource::Goal(token) => goal_check::delivered(project, token, &agent, now)?,
                _ => {}
            }
        }
        let _lock = project.lock()?;
        cursor = project::read_json(&wake_path(project)).unwrap_or_default();
        let observed = cursor.revision;
        for entry in batch.entries {
            match entry.source {
                NoticeSource::Digest { revision, .. } => {
                    if revision == observed {
                        for notice in &mut cursor.notices {
                            notice.binding.clone_from(&binding);
                        }
                    }
                }
                // Goal obligations own their retry/rebind state, not the unread journal.
                NoticeSource::Goal(_) => {}
                _ => {
                    cursor.notices.push(HeardNotice {
                        entry,
                        binding: binding.clone(),
                    });
                    cursor.revision += 1;
                    cursor.pending_announced = false;
                }
            }
        }
        project::write_json(&wake_path(project), &cursor)?;
        project::write_json(&batch_path(project), &NoticeBatch::default())?;
    }
    Ok(true)
}

/// The cheap ticker flushes even when no new transitions arrive.
pub(crate) fn flush_coordinator_notices(ctx: &Ctx, project: &Project) -> Result<bool> {
    let Some(c) = project.coordinator() else {
        return Ok(false);
    };
    if project::read_json::<NoticeBatch>(&batch_path(project)).is_none_or(|b| b.entries.is_empty())
    {
        return Ok(true);
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &c.socket, ctx.runner);
    flush_notices_at(project, &herdr, &c.pane_id, wake_now())
}

#[cfg(test)]
pub(crate) fn flush_notices_for_test(project: &Project, herdr: &Herdr<'_>) {
    let pane = project.coordinator().unwrap().pane_id;
    coordinator_notice_at(project, herdr, &pane, None, wake_now() + 120).unwrap();
}

fn adopt_report(project: &Project, event: &crate::contracts::Event) -> Result<()> {
    let Some(done) = &event.payload.done else {
        return Ok(());
    };
    let lane = thread::load(project, &event.thread)?;
    if lane.status != Status::Open
        || lane.attempt.max(1) != event.attempt
        || !lane.is_remote()
        || lane.report_hash == done.artifact
    {
        return Ok(());
    }
    // The imported artifact was checked against this hash before the event
    // was written. Do not claim a report if the artifact is not here yet.
    if !crate::events::artifact_path(project, &done.artifact).is_file() {
        return Ok(());
    }
    thread::update(project, &event.thread, |t| {
        t.report_hash = done.artifact.clone();
        t.last_report_change = project::now();
    })?;
    Ok(())
}

/// The configured publish URL for a repository (SPEC-remote §4.1): the
/// project's own row wins, then the committed Mac→box map.
fn publish_url_for(
    ctx: &Ctx,
    project: &Project,
    machine: &str,
    repo: &str,
) -> Result<Option<String>> {
    if let Ok((settings, _)) = project.read_project_md()
        && let Some(row) = settings.repos.iter().find(|row| row.path == repo)
        && let Some(url) = &row.publish_url
    {
        return Ok(Some(url.clone()));
    }
    Ok(crate::remote::box_repo_for_route(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        machine,
        repo,
    )?
    .and_then(|row| row.publish_url))
}

/// Before a box lane's DONE is typed, the Mac fetches the lane branch from the
/// URL-matched GitHub remote and checks the event's sha is reachable there
/// (SPEC-remote §4.3, gate R5). A failed fetch or an absent commit leaves the
/// event undelivered; the next pass retries.
fn verify_published_sha(
    ctx: &Ctx,
    project: &Project,
    lane: &Thread,
    sha: &str,
    published_ref: Option<&str>,
) -> Result<()> {
    if sha.is_empty() {
        bail!("published_sha_missing: the done event has no sha");
    }
    if lane.branch.is_empty() {
        bail!("published_branch_missing: {} has no lane branch", lane.id);
    }
    let url = publish_url_for(ctx, project, lane.machine_route(), &lane.repo)?
        .with_context(|| format!("box_repo_unmapped: {} has no publish URL", lane.repo))?;
    let remote = crate::remote::remote_for_url(ctx.runner, &lane.repo, &url)?;
    let git = crate::repo::Git::new(ctx.runner, &lane.repo);
    git.run(&[
        "fetch",
        "--quiet",
        remote.as_str(),
        &format!("refs/heads/{}", published_ref.unwrap_or(&lane.branch)),
    ])
    .with_context(|| format!("published_fetch_failed: {url}"))?;
    let matches = if published_ref.is_some() {
        git.run(&["rev-parse", "FETCH_HEAD"])? == sha
    } else {
        // Historical seals pointed at a mutable lane branch; their commit
        // may be an ancestor of its current tip.
        git.with_timeout(Duration::from_secs(20))
            .is_ancestor(sha, "FETCH_HEAD")?
    };
    if !matches {
        bail!("published_sha_missing: {sha} is not reachable on {url}");
    }
    Ok(())
}

/// Continuous-failure tracking for `gh` or a machine: one item when it has
/// failed for the threshold, one more when it recovers, nothing for blips.
#[derive(Debug, Clone, Default)]
pub(crate) struct Outage {
    failing_since: Option<jiff::Timestamp>,
    reported: bool,
    pub(crate) last_error: String,
}

#[derive(Debug, PartialEq)]
pub(crate) enum OutageEvent {
    Down,
    Recovered,
}

impl Outage {
    pub(crate) fn record(
        &mut self,
        ok: bool,
        error: &str,
        now: jiff::Timestamp,
        threshold_secs: i64,
    ) -> Option<OutageEvent> {
        if ok {
            let was_reported = self.reported;
            *self = Outage::default();
            return was_reported.then_some(OutageEvent::Recovered);
        }
        self.last_error = error.to_string();
        let since = *self.failing_since.get_or_insert(now);
        if !self.reported && crate::awake::elapsed(since, now) >= threshold_secs {
            self.reported = true;
            return Some(OutageEvent::Down);
        }
        None
    }
}

const REMOTE_EVERY_TICKS: u64 = 1;
const SKIP_TICKS_AFTER_FAILURE: u64 = 8;

#[derive(Debug, Clone, Default)]
pub(crate) struct MachineMemory {
    pub(crate) outage: Outage,
    /// Not polled again before this tick: one sleeping machine must not slow
    /// the other projects' ticks.
    pub(crate) skip_until_tick: u64,
    pub(crate) last_poll_tick: u64,
}

/// What the ticker process remembers between ticks (not persisted).
pub(crate) struct Memory {
    pub(crate) outage_secs: i64,
    pub(crate) tick: u64,
    pub(crate) machines: BTreeMap<String, MachineMemory>,
    /// This tick's one courier pass per due machine (SPEC-remote §4.3). The
    /// pass is machine-level, not project-level, so a later project on the same
    /// machine reuses it instead of moving the cadence. An `Err` is this
    /// tick's unreachable reason.
    pub(crate) machine_views: BTreeMap<String, Result<CourierOutcome, String>>,
}

impl Memory {
    pub(crate) fn new(ctx: &Ctx) -> Memory {
        Memory {
            // Overridable so an outage can be exercised without waiting ten minutes.
            outage_secs: ctx
                .env
                .var("HERDR_ADE_OUTAGE_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_OUTAGE_SECS),
            tick: 0,
            machines: BTreeMap::new(),
            machine_views: BTreeMap::new(),
        }
    }

    /// Poll each machine on every 15-second tick; a fresh reviewer seal
    /// should not sit through a minute of courier cadence. Failures back off.
    pub(crate) fn machine_is_due(&mut self, machine: &str) -> bool {
        let tick = self.tick;
        let entry = self.machines.entry(machine.to_string()).or_default();
        let due = tick >= entry.skip_until_tick
            && (entry.last_poll_tick == 0 || tick >= entry.last_poll_tick + REMOTE_EVERY_TICKS);
        if due {
            entry.last_poll_tick = tick;
        }
        due
    }

    pub(crate) fn record_machine(
        &mut self,
        machine: &str,
        error: Option<&str>,
        now: jiff::Timestamp,
    ) -> Option<OutageEvent> {
        let (tick, threshold) = (self.tick, self.outage_secs);
        let entry = self.machines.entry(machine.to_string()).or_default();
        if error.is_some() {
            entry.skip_until_tick = tick + SKIP_TICKS_AFTER_FAILURE + 1;
        }
        entry
            .outage
            .record(error.is_none(), error.unwrap_or(""), now, threshold)
    }
}

/// One `outage` item when a machine has been unreachable for the threshold,
/// one more when it is back. Short outages write nothing.
pub(crate) fn write_machine_outage(
    project: &Project,
    machine: &str,
    event: Option<&OutageEvent>,
    memory: &Memory,
) -> Result<()> {
    match event {
        Some(OutageEvent::Down) => {
            let error = memory
                .machines
                .get(machine)
                .map(|m| short_error(&m.outage.last_error))
                .unwrap_or_default();
            let summary = format!(
                "machine `{machine}` has been unreachable for {} minutes; its threads keep their last known state. Last error: {error}",
                memory.outage_secs / 60
            );
            inbox::write(project, "outage", machine, &summary, "").map(|_| ())
        }
        Some(OutageEvent::Recovered) => inbox::write(
            project,
            "outage",
            machine,
            &format!("machine `{machine}` is reachable again"),
            "",
        )
        .map(|_| ()),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------- courier

/// The courier's helper timeout (SPEC-remote §4.3): one short-lived call.
const COURIER_TIMEOUT: Duration = Duration::from_secs(30);

/// One envelope the box helper reported (SPEC-remote §4.3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BoxEnvelope {
    pub(crate) slug: String,
    pub(crate) event: String,
    pub(crate) event_path: String,
    pub(crate) event_hash: String,
    /// Empty for a `waiting` envelope.
    pub(crate) artifact_path: String,
    pub(crate) artifact_hash: String,
}

/// One completion receipt the box wrote at seal (D5): the event bytes and
/// report bytes the box itself hashed. The Mac compares both with its own
/// before the taken cursor advances.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CompletionReceipt {
    pub(crate) slug: String,
    pub(crate) event: String,
    pub(crate) event_hash: String,
    /// Empty for a `waiting` receipt.
    pub(crate) artifact_hash: String,
}

/// One bootstrap receipt the box wrote when its lane ran `skill lane` (D14).
/// The courier carries it to the Mac, which marks the thread's receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BootstrapReceipt {
    pub(crate) slug: String,
    pub(crate) thread: String,
    pub(crate) brief_hash: String,
    pub(crate) pane: String,
}

/// A box lane's detection-source screen and branch head, read in the courier trip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LaneProgress {
    pub(crate) pane: String,
    pub(crate) screen: String,
    pub(crate) head: String,
}

/// What one box helper call returned, after the taken cursor it was asked for.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct CourierManifest {
    pub(crate) boot_id: String,
    /// Box-local facts; `None` when the server did not answer this pass.
    pub(crate) agents: Option<Vec<Agent>>,
    pub(crate) panes: Option<Vec<Pane>>,
    pub(crate) envelopes: Vec<BoxEnvelope>,
    pub(crate) receipts: Vec<CompletionReceipt>,
    pub(crate) bootstraps: Vec<BootstrapReceipt>,
    pub(crate) progress: Vec<((String, String), LaneProgress)>,
}

/// The stable identity a courier pass resolved and the live facts it read, so
/// the caller can key lane state, detect a reboot and needs no second bridge.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CourierOutcome {
    pub(crate) machine_id: String,
    pub(crate) boot_id: String,
    /// `None` when the box's own server did not answer: the pass still imports
    /// sealed events, but changes no lane state.
    pub(crate) agents: Option<Vec<Agent>>,
    pub(crate) panes: Option<Vec<Pane>>,
    pub(crate) progress: BTreeMap<(String, String), LaneProgress>,
}

/// Recover before observing immutable records; failed server lists stay unknown.
pub(crate) fn box_manifest(
    ctx: &Ctx,
    session: &str,
    taken: &[(String, String)],
) -> Result<CourierManifest> {
    crate::ops::recover_box(ctx).context("box recovery failed")?;
    let bin = ctx.env.herdr_bin();
    let mut manifest = CourierManifest {
        boot_id: std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .unwrap_or_default()
            .trim()
            .into(),
        agents: crate::doctor::observe_list(ctx.runner, &bin, "agent", "agents", session).ok(),
        panes: crate::doctor::observe_list(ctx.runner, &bin, "pane", "panes", session).ok(),
        ..Default::default()
    };
    let taken: BTreeSet<_> = taken.iter().cloned().collect();
    for slug in project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        for (id, path) in box_records(&project.record_dir("events"), "toml")? {
            if taken.contains(&(slug.clone(), id.clone())) {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            let event: crate::contracts::Event = toml::from_str(std::str::from_utf8(&bytes)?)?;
            let artifact = event
                .payload
                .done
                .as_ref()
                .map(|done| done.artifact.clone())
                .unwrap_or_default();
            manifest.envelopes.push(BoxEnvelope {
                slug: slug.clone(),
                event: id.clone(),
                event_path: path.to_string_lossy().into_owned(),
                event_hash: thread::sha256_hex(&bytes),
                artifact_path: if artifact.is_empty() {
                    String::new()
                } else {
                    project
                        .record_dir("artifacts")
                        .join(&artifact)
                        .to_string_lossy()
                        .into_owned()
                },
                artifact_hash: artifact,
            });
            let receipt = project.record_dir("receipts").join(format!("{id}.toml"));
            match std::fs::read_to_string(receipt) {
                Ok(text) => {
                    let receipt: events::Receipt = toml::from_str(&text)?;
                    manifest.receipts.push(CompletionReceipt {
                        slug: slug.clone(),
                        event: id,
                        event_hash: receipt.event_hash,
                        artifact_hash: receipt.artifact_hash,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        for (id, path) in box_records(&project.record_dir("bootstrap"), "json")? {
            #[derive(Deserialize)]
            struct Receipt {
                brief_hash: String,
                pane: String,
            }
            let receipt: Receipt = serde_json::from_slice(&std::fs::read(path)?)?;
            manifest.bootstraps.push(BootstrapReceipt {
                slug: slug.clone(),
                thread: id,
                brief_hash: receipt.brief_hash,
                pane: receipt.pane,
            });
        }
        if let Some(panes) = &manifest.panes {
            for (id, path) in box_records(&project.record_dir("lanes"), "toml")? {
                let card: crate::contracts::LaneCard =
                    toml::from_str(&std::fs::read_to_string(path)?)?;
                if !panes.iter().any(|pane| pane.pane_id == card.pane_id) {
                    continue;
                }
                let Ok(screen) = ctx.runner.run(
                    &crate::runner::Cmd::new(&bin, COURIER_TIMEOUT)
                        .env("HERDR_SESSION", session)
                        .args([
                            "pane",
                            "read",
                            &card.pane_id,
                            "--source",
                            "detection",
                            "--format",
                            "text",
                        ]),
                ) else {
                    continue;
                };
                if !screen.success() {
                    continue;
                }
                let head = if card.box_worktree.is_empty() {
                    String::new()
                } else {
                    if card.branch.is_empty() {
                        continue;
                    }
                    let Ok(head) = crate::git::rev_parse(
                        ctx.runner,
                        &card.box_worktree,
                        &format!("refs/heads/{}", card.branch),
                    ) else {
                        continue;
                    };
                    head
                };
                manifest.progress.push((
                    (slug.clone(), id),
                    LaneProgress {
                        pane: card.pane_id,
                        screen: thread::sha256_hex(screen.stdout.trim_end_matches('\n').as_bytes()),
                        head,
                    },
                ));
            }
        }
    }
    Ok(manifest)
}

fn box_records(
    dir: &std::path::Path,
    extension: &str,
) -> Result<Vec<(String, std::path::PathBuf)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some(extension) {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("non UTF-8 box record")?;
        if !id.starts_with('.') {
            records.push((id.into(), path));
        }
    }
    records.sort();
    Ok(records)
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[derive(Debug)]
struct MachineLookupError(String);

impl std::fmt::Display for MachineLookupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for MachineLookupError {}

pub(crate) fn courier_lookup_failed(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<MachineLookupError>().is_some())
}

/// One courier pass for one saved machine, covering every project with lanes
/// on it (SPEC-remote §4.3): one multiplexed helper call, the helper reads the
/// box's own live lists, then one batched `scp` per project over that same
/// connection, hash checks, receipt checks, and a create-only import into the
/// Mac's canonical event records. The box keeps its copies; only the taken cursor
/// advances, and only after the import is durable. Returns the box's live
/// facts, so the caller can key lane state and push GONE after a reboot.
pub(crate) fn courier(ctx: &Ctx, projects: &[&Project], machine: &str) -> Result<CourierOutcome> {
    courier_inner(ctx, projects, machine)
}

fn courier_inner(ctx: &Ctx, projects: &[&Project], machine: &str) -> Result<CourierOutcome> {
    let profile =
        crate::remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, machine)
            .map_err(|error| MachineLookupError(format!("machine lookup failed: {error:#}")))?;
    if profile.is_local() {
        bail!("courier called for the local machine");
    }
    if profile.target.is_empty() {
        bail!("machine `{machine}` has no SSH target");
    }
    let target = profile.target.clone();
    let control = ctx.root.join(".state/remote");

    // The taken cursor, per project: the helper answers after it.
    let mut states = BTreeMap::new();
    let mut cursor = Vec::new();
    for project in projects {
        let state = events::remote_state(project, &profile.id);
        for id in state.taken.keys() {
            cursor.push((project.slug.clone(), id.clone()));
        }
        states.insert(project.slug.clone(), state);
    }

    let machine_paths = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)
        .map_err(|error| MachineLookupError(format!("machine declaration failed: {error:#}")))?;
    let manifest: CourierManifest = crate::box_helper::call(
        ctx.runner,
        &target,
        &machine_paths,
        crate::box_helper::Request::Courier {
            session: profile.session.clone(),
            taken: cursor,
        },
        COURIER_TIMEOUT,
        Some(&control),
    )?;

    let receipts: BTreeMap<(String, String), &CompletionReceipt> = manifest
        .receipts
        .iter()
        .map(|receipt| ((receipt.slug.clone(), receipt.event.clone()), receipt))
        .collect();

    for project in projects {
        let mut state = states.remove(&project.slug).unwrap_or_default();
        let wanted: Vec<&BoxEnvelope> = manifest
            .envelopes
            .iter()
            .filter(|env| env.slug == project.slug && !state.taken.contains_key(&env.event))
            .collect();
        if !wanted.is_empty() {
            let staging = ctx
                .root
                .join(".state/remote/staging")
                .join(&profile.id)
                .join(&project.slug);
            let _ = std::fs::remove_dir_all(&staging);
            let mut paths = Vec::new();
            for env in &wanted {
                paths.push(env.event_path.clone());
                if !env.artifact_path.is_empty() {
                    paths.push(env.artifact_path.clone());
                }
            }
            crate::remote::fetch_batch(ctx.runner, &target, &control, &paths, &staging)?;
            for env in &wanted {
                let bytes = std::fs::read(staging.join(basename(&env.event_path)))
                    .with_context(|| format!("staged event {}", env.event))?;
                let got = thread::sha256_hex(&bytes);
                if got != env.event_hash {
                    bail!(
                        "event_hash_mismatch: {} is {got}, the helper said {}",
                        env.event,
                        env.event_hash
                    );
                }
                // The box's own receipt must agree with the fetched bytes before
                // the cursor moves (D5, SPEC-remote §4.3).
                let receipt = receipts
                    .get(&(env.slug.clone(), env.event.clone()))
                    .with_context(|| {
                        format!("receipt_missing: no completion receipt for {}", env.event)
                    })?;
                if receipt.event_hash != env.event_hash {
                    bail!(
                        "receipt_mismatch: {} box receipt says {}, fetched {}",
                        env.event,
                        receipt.event_hash,
                        env.event_hash
                    );
                }
                let artifact = if env.artifact_path.is_empty() {
                    None
                } else {
                    let bytes = std::fs::read(staging.join(basename(&env.artifact_path)))
                        .with_context(|| format!("staged artifact {}", env.artifact_hash))?;
                    let got = thread::sha256_hex(&bytes);
                    if got != env.artifact_hash {
                        bail!(
                            "artifact_hash_mismatch: staged bytes are {got}, the helper said {}",
                            env.artifact_hash
                        );
                    }
                    if receipt.artifact_hash != env.artifact_hash {
                        bail!(
                            "receipt_mismatch: {} artifact receipt is {}, fetched {}",
                            env.event,
                            receipt.artifact_hash,
                            env.artifact_hash
                        );
                    }
                    Some(bytes)
                };
                events::import_box_event(project, &profile.id, &bytes, artifact.as_deref())?;
                // The sealed event is the durable coordinator-facing record.
                // Delivery is handled by `deliver_events`; a second inbox copy
                // only repeats an event the coordinator may already have acted on.
                let _imported = events::load(project, &env.event)?;
                state
                    .taken
                    .insert(env.event.clone(), env.event_hash.clone());
            }
            let _ = std::fs::remove_dir_all(&staging);
        }
        // A successful answer refreshes remote age even when no envelope was
        // new. The boot id is left for the lane observation pass to advance: it must
        // see the change to type GONE for the pre-reboot lanes.
        state.last_pass = project::now();
        events::save_remote_state(project, &profile.id, &state)?;
    }

    apply_bootstraps(projects, &manifest.bootstraps);

    Ok(CourierOutcome {
        machine_id: profile.id,
        boot_id: manifest.boot_id,
        agents: manifest.agents,
        panes: manifest.panes,
        progress: manifest.progress.into_iter().collect(),
    })
}

/// Carries a box lane's bootstrap receipt to the Mac record (D14): the thread
/// is marked only when the receipt's brief hash and pane still match it.
fn apply_bootstraps(projects: &[&Project], receipts: &[BootstrapReceipt]) {
    for receipt in receipts {
        let Some(project) = projects.iter().find(|project| project.slug == receipt.slug) else {
            continue;
        };
        let Ok(lane) = thread::load(project, &receipt.thread) else {
            continue;
        };
        if !lane.is_remote()
            || lane.pane_id != receipt.pane
            || (!lane.launch.brief_hash.is_empty() && lane.launch.brief_hash != receipt.brief_hash)
            || lane.bootstrap == "acknowledged"
        {
            continue;
        }
        let _ = thread::update(project, &receipt.thread, |lane| {
            lane.bootstrap = "acknowledged".into();
        });
    }
}

/// Types one line into the coordinator through the same serialized writer D5
/// uses (SPEC-remote §4.3). Returns whether it was typed.
pub(crate) fn type_remote_line(ctx: &Ctx, project: &Project, text: &str) -> Result<bool> {
    let Some(record) = project.coordinator() else {
        return Ok(false);
    };
    if record.pane_id.is_empty() {
        return Ok(false);
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, ctx.runner);
    deliver_coordinator_prompt(project, &herdr, &record.pane_id, text)
}

/// A server restart is a machine event, not an individual thread change.
pub(crate) fn session_notice(
    project: &Project,
    state: &mut State,
    session_lost: bool,
) -> Result<()> {
    if session_lost {
        if !state.session_item_written {
            let open = thread::list(project)
                .iter()
                .filter(|t| t.status == Status::Open && !t.is_remote())
                .count();
            inbox::write(
                project,
                "session",
                "session",
                &format!(
                    "herdr session restarted; {open} threads need `thread retry`, and the coordinator needs `open`"
                ),
                "",
            )?;
            state.session_item_written = true;
        }
        return Ok(());
    }
    state.session_item_written = false;

    Ok(())
}

fn hash_ids(ids: &BTreeSet<String>) -> String {
    thread::sha256_hex(
        ids.iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
            .as_bytes(),
    )
}

/// Step 6. Announce each unseen inbox set once as a Herdr notification.
/// Its contents remain available to the coordinator's next context read.
pub(crate) fn announce_inbox(project: &Project, state: &mut State, herdr: &Herdr) -> Result<()> {
    let seen = inbox::seen(project);
    let unseen: BTreeSet<String> = inbox::unhandled(project)
        .into_iter()
        .map(|i| i.id)
        .filter(|id| !seen.contains(id))
        .collect();
    if unseen.is_empty() {
        state.announced.clear();
        return Ok(());
    }
    let hash = hash_ids(&unseen);
    if hash == state.announced {
        return Ok(());
    }
    let body = format!(
        "{} new inbox item(s). The coordinator reads them at its next turn.",
        unseen.len()
    );
    if let Err(error) = herdr.notification_show(&format!("herdr-ade: {}", project.slug), &body) {
        eprintln!("note: inbox notification will retry: {error:#}");
        return Ok(());
    }
    state.announced = hash;
    Ok(())
}

/// Step 3, plus `config-error` items for files that do not parse.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::{World, agent_json};
    use crate::ticker::{ObservationView, observation_pass};

    #[test]
    fn historical_pr_check_in_ticker_state_does_not_prevent_loading() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let path = project.state_dir().join("ticker.json");
        std::fs::write(
            &path,
            r#"{"last_pr_check":"2026-01-01T00:00:00Z","announced":"known"}"#,
        )
        .unwrap();
        assert_eq!(load_state(&project).announced, "known");
        save_state(&project, &load_state(&project)).unwrap();
        assert!(
            !std::fs::read_to_string(path)
                .unwrap()
                .contains("last_pr_check")
        );
    }

    fn at(text: &str) -> jiff::Timestamp {
        text.parse().unwrap()
    }

    /// A project with a mid-turn coordinator (notices queue without a wake),
    /// and the fake runner answering `agent prompt`.
    fn delivery_world() -> (World, Project) {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        world
            .runner
            .on("agent prompt", crate::runner::fake::ok(r#"{"result":{}}"#));
        let cwd = project.canonical_dir().to_string_lossy().into_owned();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json(
                "w1",
                "w1:t1",
                "w1:p1",
                &cwd,
                "hp-demo-coordinator",
                "working"
            )
        );
        (world, project)
    }

    /// A lane with one sealed `done` event bound to the project's coordinator.
    fn sealed_done(project: &Project, id: &str) -> crate::contracts::Event {
        let coordinator = project.coordinator().unwrap();
        let event = crate::contracts::Event {
            id: format!("{id}-1-1"),
            op: format!("{id}-1-1"),
            usage: None,
            thread: id.into(),
            attempt: 1,
            recipient: crate::contracts::Recipient {
                pane: coordinator.pane_id.clone(),
                coordinator_attempt: coordinator.attempt(),
            },
            created: "2026-09-19T00:00:00Z".into(),
            payload: crate::contracts::EventPayload {
                done: Some(crate::contracts::DonePayload {
                    has_changes: None,
                    sha: "abc".into(),
                    report_path: ".reports/lane.md".into(),
                    artifact: "def".into(),
                    attestation: None,
                    published_ref: None,
                }),
                waiting: None,
                failed: None,
            },
        };
        crate::events::seal_create_if_absent(project, &event).unwrap();
        event
    }

    fn typed_lines(world: &World) -> Vec<String> {
        world
            .runner
            .calls
            .borrow()
            .iter()
            .filter(|cmd| cmd.display().contains("agent prompt"))
            .map(|cmd| cmd.display())
            .collect()
    }

    fn coordinator_state(world: &World, project: &Project, state: &str) {
        let c = project.coordinator().unwrap();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json(
                &c.workspace_id,
                &c.tab_id,
                &c.pane_id,
                &c.cwd,
                &c.agent_name,
                state,
            )
        );
    }

    #[test]
    fn idle_notices_coalesce_at_120_seconds_and_busy_notices_flush_now() {
        let (world, project) = delivery_world();
        coordinator_state(&world, &project, "idle");
        let c = project.coordinator().unwrap();
        let herdr = Herdr::new(world.env.herdr_bin(), &c.socket, &world.runner);
        let lines = [
            "DONE t-0001 report sha",
            "GONE t-0002 attempt 1",
            "REVIEW review-1 merged pushed",
        ];
        // Measured baseline: the previous writer submitted every line separately.
        for line in lines {
            herdr.agent_prompt(&c.pane_id, line).unwrap();
        }
        let before = typed_lines(&world).len();
        world.runner.calls.borrow_mut().clear();
        for (line, now) in lines.into_iter().zip([100, 130, 219]) {
            assert!(coordinator_notice_at(&project, &herdr, &c.pane_id, Some(line), now).unwrap());
        }
        coordinator_notice_at(&project, &herdr, &c.pane_id, None, 219).unwrap();
        assert!(typed_lines(&world).is_empty());
        coordinator_notice_at(&project, &herdr, &c.pane_id, None, 220).unwrap();
        let after = typed_lines(&world).len();
        assert_eq!(after, 1);
        let prompt = world
            .runner
            .calls
            .borrow()
            .iter()
            .find(|cmd| cmd.display().contains("agent prompt"))
            .unwrap()
            .args
            .clone();
        assert!(
            prompt.iter().any(|arg| arg == &lines.join("\n")),
            "{prompt:?}"
        );
        prime_unread(&world.ctx(), &project).unwrap();
        assert_eq!(
            typed_lines(&world).len(),
            1,
            "already-delivered lines need no digest"
        );
        println!("synthetic idle burst: notice prompts {before} -> {after} (3 lines retained)");

        coordinator_notice_at(
            &project,
            &herdr,
            &c.pane_id,
            Some("WAITING t-0003 input"),
            300,
        )
        .unwrap();
        coordinator_state(&world, &project, "working");
        coordinator_notice_at(&project, &herdr, &c.pane_id, Some("BLOCKED t-0004"), 301).unwrap();
        assert_eq!(
            typed_lines(&world).len(),
            2,
            "busy flushes even the older held line immediately"
        );
        assert!(typed_lines(&world)[1].contains("WAITING t-0003"));
        assert!(typed_lines(&world)[1].contains("BLOCKED t-0004"));
        assert!(
            project::read_json::<NoticeBatch>(&batch_path(&project))
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn goal_batch_keeps_its_deadline_and_consumed_checks_do_not_replay_on_rebind() {
        let (world, project) = delivery_world();
        coordinator_state(&world, &project, "idle");
        goal_check::reconcile(&project, None, 10).unwrap();
        let (token, line) = goal_check::notice(&project).unwrap();
        let c = project.coordinator().unwrap();
        let herdr = Herdr::new(world.env.herdr_bin(), &c.socket, &world.runner);
        enqueue(
            &project,
            OutboxEntry::new(NoticeSource::Goal(token), &line),
            100,
        )
        .unwrap();
        coordinator_notice_at(
            &project,
            &herdr,
            &c.pane_id,
            Some("Independent notice"),
            219,
        )
        .unwrap();
        coordinator_notice_at(&project, &herdr, &c.pane_id, None, 220).unwrap();
        assert_eq!(
            typed_lines(&world).len(),
            1,
            "keep the first notice's deadline"
        );
        assert!(typed_lines(&world)[0].contains("Goal check owed"));
        goal_check::record(
            &project,
            goal_check::Disposition::Wait {
                tasks: vec![],
                party: "upstream service".into(),
                condition: "data arrives".into(),
            },
            "No authorized work can proceed before the data arrives",
        )
        .unwrap();
        // No context receipt: a rebind must retain ordinary transitions, but
        // must not turn the consumed goal prompt into untyped new news.
        project
            .update_coordinator(|c| {
                c.generation += 1;
                c.pane_id = "w1:p9".into();
            })
            .unwrap();
        coordinator_state(&world, &project, "working");
        prime_unread(&world.ctx(), &project).unwrap();
        let prompts = typed_lines(&world);
        assert_eq!(prompts.len(), 2);
        assert!(prompts[1].contains("Independent notice"));
        assert!(!prompts[1].contains("Goal check owed"));
    }

    #[test]
    fn held_notices_survive_receipt_rebind_and_transport_failure() {
        let (world, project) = delivery_world();
        coordinator_state(&world, &project, "idle");
        let c = project.coordinator().unwrap();
        let failing = crate::runner::fake::FakeRunner::new();
        failing.on(
            "agent list",
            crate::runner::fake::ok(&format!(
                r#"{{"result":{{"agents":{}}}}}"#,
                world.agents.borrow()
            )),
        );
        failing.on("pane read", crate::runner::fake::ok("❯ \n"));
        failing.on(
            "agent prompt",
            crate::runner::fake::fail(
                1,
                r#"{"error":{"code":"unreachable","message":"transport failed"}}"#,
            ),
        );
        let herdr = Herdr::new("herdr", &c.socket, &failing);
        assert!(
            coordinator_notice_at(&project, &herdr, &c.pane_id, Some("DONE t-0001"), 100).unwrap()
        );
        receipt(&project, wake_revision(&project)).unwrap();
        assert!(coordinator_notice_at(&project, &herdr, &c.pane_id, None, 220).unwrap());
        assert_eq!(
            project::read_json::<NoticeBatch>(&batch_path(&project))
                .unwrap()
                .entries
                .into_iter()
                .map(|entry| entry.line)
                .collect::<Vec<_>>(),
            ["DONE t-0001"]
        );
        project
            .update_coordinator(|c| {
                c.pane_id = "w1:p9".into();
                c.generation += 1;
            })
            .unwrap();
        coordinator_state(&world, &project, "working");
        flush_coordinator_notices(&world.ctx(), &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
        assert!(typed_lines(&world)[0].contains("w1:p9"));
        prime_unread(&world.ctx(), &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
    }

    #[test]
    fn idle_event_journal_distinguishes_durable_queue_from_transport() {
        let (world, project) = delivery_world();
        coordinator_state(&world, &project, "idle");
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Open;
        })
        .unwrap();
        let event = sealed_done(&project, &lane.id);
        deliver_events(&world.ctx(), &project).unwrap();
        deliver_event(&world.ctx(), &project, &event).unwrap();
        deliver_events(&world.ctx(), &project).unwrap();
        assert!(typed_lines(&world).is_empty());
        assert_eq!(
            events::states(&project, &event.id).unwrap(),
            [crate::contracts::DeliveryState::Queued]
        );
        assert_eq!(
            project::read_json::<NoticeBatch>(&batch_path(&project))
                .unwrap()
                .entries
                .len(),
            1
        );
        receipt(&project, wake_revision(&project)).unwrap();
        let c = project.coordinator().unwrap();
        let herdr = Herdr::new("herdr", &c.socket, &world.runner);
        flush_notices_for_test(&project, &herdr);
        assert_eq!(
            events::states(&project, &event.id).unwrap(),
            [
                crate::contracts::DeliveryState::Queued,
                crate::contracts::DeliveryState::Submitted
            ]
        );
        deliver_event(&world.ctx(), &project, &event).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
    }

    #[test]
    fn offline_transition_sources_queue_once_even_when_their_text_is_identical() {
        let (world, project) = delivery_world();
        *world.agents.borrow_mut() = "[]".into();
        for _ in 0..2 {
            thread::allocate(&project, |t| {
                t.status = Status::Open;
                t.start_notices.push(Notice {
                    line: "Same transition".into(),
                    submitted: false,
                });
            })
            .unwrap();
        }
        for _ in 0..3 {
            deliver_transition_notices(&world.ctx(), &project).unwrap();
        }
        let batch: NoticeBatch = project::read_json(&batch_path(&project)).unwrap();
        assert_eq!(batch.entries.len(), 2);
        assert_ne!(batch.entries[0].source, batch.entries[1].source);
        assert!(
            thread::list_live(&project)
                .iter()
                .all(|lane| lane.start_notices[0].submitted)
        );
        assert!(typed_lines(&world).is_empty());
        coordinator_state(&world, &project, "working");
        deliver_transition_notices(&world.ctx(), &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
        let cursor: WakeCursor = project::read_json(&wake_path(&project)).unwrap();
        assert_eq!(cursor.notices.len(), 2);
        receipt(&project, cursor.revision).unwrap();
        assert!(
            project::read_json::<WakeCursor>(&wake_path(&project))
                .unwrap()
                .notices
                .is_empty()
        );
    }

    #[test]
    fn offline_event_keeps_queue_transport_and_context_receipts_separate() {
        let (world, project) = delivery_world();
        let lane = thread::allocate(&project, |t| t.status = Status::Open).unwrap();
        let event = sealed_done(&project, &lane.id);
        *world.agents.borrow_mut() = "[]".into();
        for _ in 0..3 {
            deliver_events(&world.ctx(), &project).unwrap();
        }
        events::append_delivery(
            &project,
            &event.id,
            crate::contracts::DeliveryState::Acknowledged,
        )
        .unwrap();
        assert_eq!(
            events::states(&project, &event.id).unwrap(),
            [
                crate::contracts::DeliveryState::Queued,
                crate::contracts::DeliveryState::Acknowledged
            ]
        );
        assert!(typed_lines(&world).is_empty());
        assert_eq!(
            project::read_json::<NoticeBatch>(&batch_path(&project))
                .unwrap()
                .entries
                .len(),
            1
        );
        coordinator_state(&world, &project, "working");
        deliver_events(&world.ctx(), &project).unwrap();
        assert_eq!(
            events::states(&project, &event.id).unwrap(),
            [
                crate::contracts::DeliveryState::Queued,
                crate::contracts::DeliveryState::Acknowledged,
                crate::contracts::DeliveryState::Submitted
            ]
        );
        assert_eq!(typed_lines(&world).len(), 1);
    }

    #[test]
    fn historical_batch_decodes_distinct_event_digest_goal_and_transition_entries() {
        let batch: NoticeBatch = serde_json::from_value(serde_json::json!({
            "first_at": 100,
            "lines": ["A transition", "WAITING t-0001 input", "DONE t-0001 report sha", "Unread transitions"],
            "events": ["t-0001-1-1", "t-0001-1-2"],
            "wake_lines": ["A transition", "WAITING t-0001 input", "DONE t-0001 report sha"],
            "digests": [7], "goals": [["goal-token", "Goal check owed"]]
        })).unwrap();
        assert_eq!(batch.first_at, 100);
        assert_eq!(
            batch
                .entries
                .iter()
                .map(|entry| entry.source.clone())
                .collect::<Vec<_>>(),
            [
                NoticeSource::Transition("A transition".into()),
                NoticeSource::Event("t-0001-1-1".into()),
                NoticeSource::Event("t-0001-1-2".into()),
                NoticeSource::Digest {
                    revision: 7,
                    binding: String::new()
                },
                NoticeSource::Goal("goal-token".into())
            ]
        );
        let saved = serde_json::to_value(&batch).unwrap();
        assert!(saved.get("lines").is_none());
        assert_eq!(
            serde_json::from_value::<NoticeBatch>(saved)
                .unwrap()
                .entries
                .len(),
            5
        );
        let cursor: WakeCursor = serde_json::from_value(serde_json::json!({
            "revision": 7, "binding": "old", "lines": ["A", "B"], "heard_bindings": ["new"]
        }))
        .unwrap();
        assert_eq!(cursor.notices[0].binding, "new");
        assert_eq!(cursor.notices[1].binding, "old");
        assert!(
            serde_json::to_value(cursor)
                .unwrap()
                .get("heard_bindings")
                .is_none()
        );
    }

    #[test]
    fn context_receipt_cancels_an_idle_queued_digest() {
        let (world, project) = delivery_world();
        type_remote_line(&world.ctx(), &project, "REVIEW old transition").unwrap();
        project
            .update_coordinator(|c| {
                c.generation += 2;
            })
            .unwrap();
        coordinator_state(&world, &project, "idle");
        prime_unread(&world.ctx(), &project).unwrap();
        assert_eq!(
            project::read_json::<NoticeBatch>(&batch_path(&project))
                .unwrap()
                .entries
                .len(),
            1
        );
        receipt(&project, wake_revision(&project)).unwrap();
        coordinator_state(&world, &project, "working");
        flush_coordinator_notices(&world.ctx(), &project).unwrap();
        assert_eq!(
            typed_lines(&world).len(),
            1,
            "received transitions are not replayed from a queued digest"
        );
    }

    #[test]
    fn historical_delivered_cursor_does_not_repeat_a_digest_on_idle() {
        for primed in [false, true] {
            let (world, project) = delivery_world();
            coordinator_state(&world, &project, "idle");
            let binding = wake_binding(&project);
            project::write_json(
                &wake_path(&project),
                &serde_json::json!({
                    "binding": if primed { "previous" } else { &binding }, "revision": 1,
                    "primed_binding": if primed { &binding } else { "" },
                    "lines": ["REVIEW review-1 merged pushed"], "submitted_at": 1
                }),
            )
            .unwrap();
            prime_unread(&world.ctx(), &project).unwrap();
            assert!(typed_lines(&world).is_empty());
            coordinator_state(&world, &project, "working");
            type_remote_line(&world.ctx(), &project, "BLOCKED t-0002").unwrap();
            prime_unread(&world.ctx(), &project).unwrap();
            assert_eq!(
                typed_lines(&world).len(),
                1,
                "new notices cannot re-arm an already-delivered historical digest"
            );
        }
    }

    #[test]
    fn sealed_park_clears_the_fork_link_before_close() {
        let (world, project) = delivery_world();
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Open;
            t.pane_id = "w2:p1".into();
            t.tab_id = "w2:t1".into();
            t.workspace_id = "w2".into();
            t.cwd = "/lane".into();
            t.repo = "/repo".into();
            t.base = "base".into();
            t.prompt_pending = false;
        })
        .unwrap();
        *world.panes.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::pane_json("w2", "w2:t1", "w2:p1", "/lane")
        );
        sealed_done(&project, &lane.id);
        deliver_events(&world.ctx(), &project).unwrap();
        assert!(
            typed_lines(&world).is_empty(),
            "a changed pile member has no individual DONE notice"
        );
        crate::threads::park_completed(&world.ctx(), &project).unwrap();
        assert!(thread::load(&project, &lane.id).unwrap().parked);
        // Replay the fork's documented parent-token/closure fixture. Before,
        // closing an attached pane produced one GONE; now the link is removed.
        let mut parent = true;
        let mut gone = 0;
        for cmd in world.runner.calls.borrow().iter() {
            let line = cmd.display();
            if line.contains("--clear-token parent") {
                parent = false;
            }
            if line.contains("workspace close") || line.contains("tab close") {
                gone += usize::from(parent);
            }
        }
        assert_eq!(gone, 0);
        println!("synthetic sealed park: GONE 1 -> {gone}");
    }

    #[test]
    fn live_box_observation_keeps_the_parent_link() {
        let (world, project) = delivery_world();
        let lane = world.thread(&project, world.home.path(), |t| {
            t.machine_id = "box".into();
            t.machine = "box".into();
            t.launch_attempts = 1;
        });
        let parent = project.coordinator().unwrap().pane_id;
        let agent = Agent {
            pane_id: lane.pane_id.clone(),
            tab_id: lane.tab_id.clone(),
            workspace_id: lane.workspace_id.clone(),
            cwd: lane.cwd.clone(),
            name: lane.agent_name.clone(),
            agent: lane.agent.clone(),
            agent_status: "working".into(),
            tokens: [("parent".into(), parent.clone())].into(),
            ..Agent::default()
        };
        let pane = serde_json::from_str(&crate::scenarios::pane_json(
            &lane.workspace_id,
            &lane.tab_id,
            &lane.pane_id,
            &lane.cwd,
        ))
        .unwrap();
        assert!(thread::agent_matches(&lane, &agent));
        assert!(thread::pane_matches(&lane, &pane));
        assert_eq!(agent.parent(), Some(parent.as_str()));
        for _ in 0..3 {
            let errors = observation_pass(
                &world.ctx(),
                &project,
                ObservationView {
                    machine_id: "box",
                    threads: std::slice::from_ref(&lane),
                    agents: std::slice::from_ref(&agent),
                    panes: std::slice::from_ref(&pane),
                    boot_id: "boot-1",
                    now: at("2026-09-19T00:00:00Z"),
                },
            );
            assert!(errors.is_empty(), "{errors:?}");
        }
        assert_eq!(world.runner.count("--clear-token parent"), 0);
        assert!(typed_lines(&world).is_empty());
    }

    #[test]
    fn sealed_box_attempts_ignore_missing_snapshots_and_reboots() {
        for waiting in [false, true] {
            let (world, project) = delivery_world();
            let lane = thread::allocate(&project, |t| {
                t.status = Status::Open;
                t.pane_id = "w2:p1".into();
                t.tab_id = "w2:t1".into();
                t.workspace_id = "w2".into();
                t.cwd = "/lane".into();
                t.machine_id = "box".into();
                t.machine = "box".into();
                t.launch_attempts = 1;
            })
            .unwrap();
            let mut event = sealed_done(&project, &lane.id);
            if waiting {
                event.id = format!("{}-1-2", lane.id);
                event.op = event.id.clone();
                event.payload.done = None;
                event.payload.waiting = Some(crate::contracts::WaitingPayload {
                    text: "missing input".into(),
                    class: crate::contracts::FailureClass::Unknown,
                    provider_kind: None,
                });
                events::seal_create_if_absent(&project, &event).unwrap();
            }
            let mut state = events::remote_state(&project, "box");
            state.boot_id = "boot-1".into();
            events::save_remote_state(&project, "box", &state).unwrap();
            for _ in 0..5 {
                let errors = observation_pass(
                    &world.ctx(),
                    &project,
                    ObservationView {
                        machine_id: "box",
                        threads: std::slice::from_ref(&lane),
                        agents: &[],
                        panes: &[],
                        boot_id: "boot-2",
                        now: at("2026-09-19T00:00:00Z"),
                    },
                );
                assert!(errors.is_empty(), "{errors:?}");
            }
            assert!(typed_lines(&world).is_empty());
            assert!(
                thread::load(&project, &lane.id)
                    .unwrap()
                    .start_notices
                    .is_empty()
            );
            assert!(
                events::remote_state(&project, "box")
                    .pending_gone
                    .is_empty()
            );
        }
    }

    #[test]
    fn live_box_reboot_recovers_once_and_stale_terminal_views_cannot_repeat_gone() {
        let (world, project) = delivery_world();
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Open;
            t.pane_id = "w2:p1".into();
            t.tab_id = "w2:t1".into();
            t.workspace_id = "w2".into();
            t.cwd = "/lane".into();
            t.machine_id = "box".into();
            t.machine = "box".into();
            t.launch_attempts = 1;
        })
        .unwrap();
        *world.panes.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::pane_json("w2", "w2:t1", "w2:p1", "/lane")
        );
        let mut state = events::remote_state(&project, "box");
        state.boot_id = "boot-1".into();
        events::save_remote_state(&project, "box", &state).unwrap();
        let c = project.coordinator().unwrap();
        let remote = Herdr::new("herdr", &c.socket, &world.runner).on_machine("box");
        let panes = remote.pane_list().unwrap();
        for _ in 0..3 {
            let errors = observation_pass(
                &world.ctx(),
                &project,
                ObservationView {
                    machine_id: "box",
                    threads: std::slice::from_ref(&lane),
                    agents: &[],
                    panes: &panes,
                    boot_id: "boot-2",
                    now: at("2026-09-19T00:00:00Z"),
                },
            );
            assert!(errors.is_empty(), "{errors:?}");
        }
        deliver_transition_notices(&world.ctx(), &project).unwrap();
        assert_eq!(world.runner.count("workspace close w2"), 1);
        assert_eq!(
            typed_lines(&world)
                .iter()
                .filter(|line| line.contains("GONE"))
                .count(),
            1
        );
        let calls = world.runner.calls.borrow();
        let clear = calls
            .iter()
            .position(|cmd| cmd.display().contains("--clear-token parent"))
            .unwrap();
        let close = calls
            .iter()
            .position(|cmd| cmd.display().contains("workspace close w2"))
            .unwrap();
        assert!(clear < close);
    }

    #[test]
    fn unsealed_box_crash_is_durable_and_stale_reobservation_cannot_repeat_it() {
        let (world, project) = delivery_world();
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Open;
            t.pane_id = "w2:p1".into();
            t.tab_id = "w2:t1".into();
            t.workspace_id = "w2".into();
            t.cwd = "/lane".into();
            t.machine_id = "box".into();
            t.machine = "box".into();
        })
        .unwrap();
        for _ in 0..5 {
            let errors = observation_pass(
                &world.ctx(),
                &project,
                ObservationView {
                    machine_id: "box",
                    threads: std::slice::from_ref(&lane),
                    agents: &[],
                    panes: &[],
                    boot_id: "boot-1",
                    now: at("2026-09-19T00:00:00Z"),
                },
            );
            assert!(errors.is_empty(), "{errors:?}");
        }
        deliver_transition_notices(&world.ctx(), &project).unwrap();
        deliver_transition_notices(&world.ctx(), &project).unwrap();
        assert_eq!(
            typed_lines(&world)
                .iter()
                .filter(|line| line.contains("GONE"))
                .count(),
            1
        );
        assert_eq!(
            thread::load(&project, &lane.id)
                .unwrap()
                .start_notices
                .iter()
                .filter(|n| n.line.starts_with("GONE"))
                .count(),
            1
        );
    }

    #[test]
    fn unread_wake_is_digested_once_after_rebind_not_retyped() {
        let (world, project) = delivery_world();
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Open;
        })
        .unwrap();
        let event = sealed_done(&project, &lane.id);
        deliver_events(&world.ctx(), &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
        project
            .update_coordinator(|c| {
                c.generation += 1;
                c.pane_id = "w1:p2".into();
            })
            .unwrap();
        let cwd = project.canonical_dir().to_string_lossy().into_owned();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json("w1", "w1:t1", "w1:p2", &cwd, "hp-demo-coordinator", "idle")
        );
        deliver_events(&world.ctx(), &project).unwrap();
        assert_eq!(
            typed_lines(&world).len(),
            1,
            "the unheard digest also waits for idle batching"
        );
        type_remote_line(&world.ctx(), &project, "REVIEW review-1 merged pushed").unwrap();
        let c = project.coordinator().unwrap();
        let herdr = Herdr::new("herdr", &c.socket, &world.runner);
        flush_notices_for_test(&project, &herdr);
        assert_eq!(typed_lines(&world).len(), 2);
        assert!(typed_lines(&world)[1].contains("REVIEW review-1"));
        assert!(typed_lines(&world)[1].contains("Unread transitions"));
        assert!(typed_lines(&world)[1].contains("DONE"));
        deliver_events(&world.ctx(), &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 2);
        assert!(
            crate::events::states(&project, &event.id)
                .unwrap()
                .contains(&crate::contracts::DeliveryState::Submitted)
        );
        // A new delivered line must not re-arm the old digest on this binding.
        coordinator_state(&world, &project, "working");
        type_remote_line(&world.ctx(), &project, "BLOCKED t-0002").unwrap();
        deliver_events(&world.ctx(), &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 3);
    }

    #[test]
    fn context_started_before_a_wake_cannot_consume_it() {
        let (world, project) = delivery_world();
        let observed = wake_revision(&project);
        let bound = project.coordinator().unwrap();
        let herdr = Herdr::new(world.env.herdr_bin(), &bound.socket, &world.runner);
        assert!(
            deliver_coordinator_prompt(
                &project,
                &herdr,
                &bound.pane_id,
                "REVIEW review-1 needs attention"
            )
            .unwrap()
        );
        receipt(&project, observed).unwrap();
        let cursor: WakeCursor = project::read_json(&wake_path(&project)).unwrap();
        assert_eq!(cursor.notices.len(), 1);
    }

    #[test]
    fn received_wake_is_not_digested_on_rebind() {
        let (world, project) = delivery_world();
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Open;
        })
        .unwrap();
        sealed_done(&project, &lane.id);
        deliver_events(&world.ctx(), &project).unwrap();
        receipt(&project, wake_revision(&project)).unwrap();
        project
            .update_coordinator(|c| {
                c.generation += 1;
                c.pane_id = "w1:p2".into();
            })
            .unwrap();
        let cwd = project.canonical_dir().to_string_lossy().into_owned();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json("w1", "w1:t1", "w1:p2", &cwd, "hp-demo-coordinator", "idle")
        );
        deliver_events(&world.ctx(), &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
    }

    #[test]
    fn seal_reports_a_queued_follow_up_even_if_it_lands_before_delivery() {
        let (world, project) = delivery_world();
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Open;
            t.attempt = 1;
            t.follow_ups.push(crate::thread::FollowUp {
                attempt: 1,
                text: "Check the missing gate".into(),
                queued_at: project::now(),
                ..Default::default()
            });
        })
        .unwrap();
        let event = sealed_done(&project, &lane.id);
        thread::update(&project, &lane.id, |t| {
            t.follow_ups[0].state = crate::thread::FollowUpState::Delivered;
            t.follow_ups[0].after_seal = event.id.clone();
            t.follow_ups[0].delivered_at = project::now();
        })
        .unwrap();
        deliver_event(&world.ctx(), &project, &event).unwrap();
        assert!(
            typed_lines(&world)
                .iter()
                .any(|line| line.contains("Follow-up overtook this seal")
                    && line.contains("Check the missing gate"))
        );
        assert!(
            crate::inbox::unhandled(&project)
                .iter()
                .any(|item| item.kind == "follow-up-pending")
        );
    }

    #[test]
    fn an_event_read_before_its_line_is_typed_is_repaired() {
        let (world, project) = delivery_world();
        thread::allocate(&project, |t| {
            t.title = "Lane".into();
            t.status = Status::Open;
        })
        .unwrap();
        let event = sealed_done(&project, "t-0001");
        // The coordinator read the completion in the same window as the seal,
        // before the ticker had typed the wake-up line.
        crate::events::append_delivery(
            &project,
            &event.id,
            crate::contracts::DeliveryState::Acknowledged,
        )
        .unwrap();

        let ctx = world.ctx();
        deliver_events(&ctx, &project).unwrap();

        let lines = typed_lines(&world);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("DONE t-0001"), "{}", lines[0]);
        assert_eq!(
            crate::events::states(&project, &event.id).unwrap(),
            vec![
                crate::contracts::DeliveryState::Acknowledged,
                crate::contracts::DeliveryState::Queued,
                crate::contracts::DeliveryState::Submitted
            ]
        );

        // Once repaired, the wake-up is never typed a second time.
        deliver_events(&ctx, &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
    }

    /// A resolved lane is finished: its delivery journal is never replayed,
    /// even when the event was read before its wake-up line was typed.
    #[test]
    fn a_resolved_lane_event_is_never_replayed() {
        let (world, project) = delivery_world();
        let lane = thread::allocate(&project, |t| {
            t.title = "Lane".into();
            t.status = Status::Resolved;
            t.resolved_reason = "manual".into();
        })
        .unwrap();
        let event = sealed_done(&project, &lane.id);
        crate::events::append_delivery(
            &project,
            &event.id,
            crate::contracts::DeliveryState::Acknowledged,
        )
        .unwrap();

        let ctx = world.ctx();
        deliver_events(&ctx, &project).unwrap();

        assert!(
            typed_lines(&world).is_empty(),
            "no replay for a resolved lane"
        );
        assert_eq!(
            crate::events::states(&project, &event.id).unwrap(),
            vec![crate::contracts::DeliveryState::Acknowledged]
        );
    }

    #[test]
    fn typed_draft_holds_ordered_lane_notices_while_inbox_uses_notification() {
        let (world, project) = delivery_world();
        world
            .runner
            .on("notification show", crate::runner::fake::ok("{}"));
        let screen = std::rc::Rc::new(std::cell::RefCell::new("❯ Rolf is typing\n".to_string()));
        let read = screen.clone();
        world.runner.on_fn(
            |cmd| cmd.display().contains("pane read") && cmd.display().contains("--source visible"),
            move |_| Ok(crate::runner::fake::ok(&read.borrow())),
        );
        let first = thread::allocate(&project, |t| t.status = Status::Open).unwrap();
        let second = thread::allocate(&project, |t| t.status = Status::Open).unwrap();
        let a = sealed_done(&project, &first.id);
        let b = sealed_done(&project, &second.id);
        inbox::write(&project, "note", "test", "new inbox item", "").unwrap();
        let ctx = world.ctx();
        let herdr = Herdr::new(
            ctx.env.herdr_bin(),
            &project.coordinator().unwrap().socket,
            ctx.runner,
        );
        let mut state = State::default();
        announce_inbox(&project, &mut state, &herdr).unwrap();
        deliver_events(&ctx, &project).unwrap();
        assert!(typed_lines(&world).is_empty());
        assert!(!state.announced.is_empty());
        assert_eq!(world.runner.count("notification show"), 1);
        assert_eq!(
            events::states(&project, &a.id).unwrap(),
            [crate::contracts::DeliveryState::Queued]
        );
        assert_eq!(
            events::states(&project, &b.id).unwrap(),
            [crate::contracts::DeliveryState::Queued]
        );
        *screen.borrow_mut() = "❯ \n".into();
        deliver_events(&ctx, &project).unwrap();
        let lines = typed_lines(&world);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let first_at = lines[0].find(&first.id).unwrap();
        let second_at = lines[0].find(&second.id).unwrap();
        assert!(first_at < second_at, "{lines:?}");
        announce_inbox(&project, &mut state, &herdr).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
        assert_eq!(world.runner.count("notification show"), 1);
    }

    #[test]
    fn held_remote_seal_still_records_its_report_and_retries_notice() {
        let (world, project) = delivery_world();
        let (mut settings, body) = project.read_project_md().unwrap();
        settings.repos.push(crate::project::Repo {
            path: "/repo".into(),
            publish_url: Some("https://example.test/repo.git".into()),
            ..Default::default()
        });
        std::fs::write(
            project.project_md(),
            format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        world.runner.on(
            "git -C /repo remote get-url fork",
            crate::runner::fake::ok("https://example.test/repo.git\n"),
        );
        world
            .runner
            .on("git -C /repo remote", crate::runner::fake::ok("fork\n"));
        world
            .runner
            .on("git -C /repo fetch", crate::runner::fake::ok(""));
        world.runner.on(
            "git -C /repo merge-base --is-ancestor",
            crate::runner::fake::ok(""),
        );
        let screen = std::rc::Rc::new(std::cell::RefCell::new(
            "❯ Rolf's unfinished draft\n".to_string(),
        ));
        let read = screen.clone();
        world.runner.on_fn(
            |cmd| cmd.display().contains("pane read"),
            move |_| Ok(crate::runner::fake::ok(&read.borrow())),
        );
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Open;
            t.machine = "box".into();
            t.machine_id = "box".into();
            t.repo = "/repo".into();
            t.branch = "hp/demo/t-0001".into();
        })
        .unwrap();
        let event = sealed_done(&project, &lane.id);
        let next = thread::allocate(&project, |t| {
            t.status = Status::Open;
            t.machine = "box".into();
            t.machine_id = "box".into();
            t.repo = "/repo".into();
            t.branch = "hp/demo/t-0002".into();
        })
        .unwrap();
        let later = sealed_done(&project, &next.id);
        let artifact = crate::events::artifact_path(&project, "def");
        std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        std::fs::write(artifact, b"report").unwrap();
        deliver_events(&world.ctx(), &project).unwrap();
        assert_eq!(thread::load(&project, &lane.id).unwrap().report_hash, "def");
        assert_eq!(thread::load(&project, &next.id).unwrap().report_hash, "def");
        assert!(events::states(&project, &event.id).unwrap().is_empty());
        assert!(events::states(&project, &later.id).unwrap().is_empty());
        assert!(typed_lines(&world).is_empty());
        let digest = crate::coordinator::digest(&world.ctx(), &project, "ha")
            .unwrap()
            .0;
        assert!(digest.contains("done: abc"), "{digest}");
        *screen.borrow_mut() = "────────────────────\n❯ \n────────────────────\n  /home/agent/.herdr-ade/adeherdr > ctx\n  ⏵⏵ bypass permissions on · 1 shell · ← for agents\n  ● main\n  ◯ general-purpose  Verifying excluded files · 20m\n".into();
        deliver_events(&world.ctx(), &project).unwrap();
        assert!(typed_lines(&world).is_empty());
        assert!(events::states(&project, &event.id).unwrap().is_empty());
        assert!(events::states(&project, &later.id).unwrap().is_empty());
    }

    #[test]
    fn review_notice_survives_resolved_reviewer_and_busy_coordinator() {
        let (world, project) = delivery_world();
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Open;
            t.repo = "/repo".into();
            t.base = "base".into();
        })
        .unwrap();
        let member = sealed_done(&project, &lane.id);
        deliver_events(&world.ctx(), &project).unwrap();
        assert!(typed_lines(&world).is_empty());
        assert!(events::states(&project, &member.id).unwrap().is_empty());

        let reviewer = thread::allocate(&project, |t| {
            t.status = Status::Open;
            t.role = "reviewer".into();
            t.repo = "/repo".into();
        })
        .unwrap();
        let review_event = sealed_done(&project, &reviewer.id);
        let review = crate::review::Review {
            id: "review-1".into(),
            repo: "/repo".into(),
            integration: "main".into(),
            base: "base".into(),
            candidate_branch: "candidate".into(),
            members: vec![],
            gates: vec![],
            gates_note: String::new(),
            selected_gates: vec![],
            reviewer: Some(reviewer.id.clone()),
            phase: crate::review::Phase::Complete,
            verdict: None,
            verdict_event: String::new(),
            reviewer_after: String::new(),
            checked_event: String::new(),
            retry_attempt: None,
            retry_generation: 0,
            moved: 0,
            refresh_tip: None,
            push_remote: None,
            install_required: false,
            fast_forward: true,
            push: true,
            install: true,
            install_result: String::new(),
            close: true,
            prune: true,
            attention: String::new(),
            no_verdict_since: String::new(),
            notices: vec![Notice {
                line: "REVIEW review-1 merged t-0001 (abc, pushed)".into(),
                submitted: false,
            }],
        };
        crate::review::save(&project, &review).unwrap();
        thread::update(&project, &reviewer.id, |t| t.status = Status::Resolved).unwrap();
        let screen = std::rc::Rc::new(std::cell::RefCell::new("❯ busy draft\n".to_string()));
        let read = screen.clone();
        world.runner.on_fn(
            |cmd| cmd.display().contains("pane read") && cmd.display().contains("--source visible"),
            move |_| Ok(crate::runner::fake::ok(&read.borrow())),
        );
        deliver_events(&world.ctx(), &project).unwrap();
        assert!(typed_lines(&world).is_empty());
        *screen.borrow_mut() = "❯ \n".into();
        deliver_events(&world.ctx(), &project).unwrap();
        deliver_events(&world.ctx(), &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
        assert!(typed_lines(&world)[0].contains("REVIEW review-1 merged"));
        assert!(
            events::states(&project, &review_event.id)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn final_start_failure_wakes_once_but_recovery_pending_does_not() {
        let (world, project) = delivery_world();
        let lane = thread::allocate(&project, |t| {
            t.status = Status::Starting;
            t.launch_attempts = 0;
        })
        .unwrap();
        thread::update(&project, &lane.id, |t| {
            t.provider_wait_started = project::now();
        })
        .unwrap();
        deliver_events(&world.ctx(), &project).unwrap();
        assert!(typed_lines(&world).is_empty());
        crate::threads::fail_start(
            &world.ctx(),
            &project,
            &lane.id,
            "provider_wait_expired",
            crate::contracts::FailureClass::Provider,
            false,
        )
        .unwrap();
        deliver_events(&world.ctx(), &project).unwrap();
        deliver_events(&world.ctx(), &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
        assert!(typed_lines(&world)[0].contains(&format!(
            "FAILED {}: provider_wait_expired — next: ha thread retry demo {} --reason \"retry failed startup\"",
            lane.id, lane.id
        )));
    }

    #[test]
    fn a_normal_event_is_typed_once_and_not_retyped() {
        let (world, project) = delivery_world();
        thread::allocate(&project, |t| {
            t.title = "Lane".into();
            t.status = Status::Open;
        })
        .unwrap();
        let event = sealed_done(&project, "t-0001");

        let ctx = world.ctx();
        deliver_events(&ctx, &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
        assert_eq!(
            crate::events::states(&project, &event.id).unwrap(),
            vec![
                crate::contracts::DeliveryState::Queued,
                crate::contracts::DeliveryState::Submitted
            ]
        );

        deliver_events(&ctx, &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 1);
        assert!(inbox::unhandled(&project).is_empty());
        let digest = crate::coordinator::digest(&ctx, &project, "ha").unwrap().0;
        assert!(
            digest.contains("done: abc report=.state/artifacts/def (missing)"),
            "{digest}"
        );
        assert!(!project.state_dir().join("inbox-counter.json").exists());
    }

    #[test]
    fn courier_manifest_roundtrips_typed_progress_and_refuses_junk() {
        let mut manifest = CourierManifest::default();
        manifest.progress.push((
            ("demo".into(), "t-0001".into()),
            LaneProgress {
                pane: "w:p".into(),
                screen: "hash".into(),
                head: "sha".into(),
            },
        ));
        let text = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<CourierManifest>(&text).unwrap(),
            manifest
        );
        assert!(serde_json::from_str::<CourierManifest>("nonsense").is_err());
    }

    #[test]
    fn courier_helper_surfaces_box_recovery_failure() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let dir = project.record_dir_for_write("ops").unwrap();
        std::fs::write(
            dir.join("t-0001-1-1.toml"),
            r#"
op = "t-0001-1-1"
revision = 1
thread = "t-0001"
attempt = 1
kind = "waiting"
helper_pid = 0
event = "t-0001-1-1"
state = "staged"
created = "2026-10-03T00:00:00Z"
[recipient]
pane = "w:p"
coordinator_attempt = 1
[requested]
text = "waiting"
"#,
        )
        .unwrap();
        // Missing staged evidence cannot be mistaken for a completed pass.
        let error = box_manifest(&world.ctx(), "default", &[]).unwrap_err();
        assert!(format!("{error:#}").contains("box recovery failed"));
    }

    #[test]
    fn courier_helper_survives_a_hostile_box_root() {
        let world = World::new();
        let root = world.home.path().join("it's a $(box)");
        project::create(&root, "demo", "", vec![]).unwrap();
        let ctx = Ctx {
            root,
            ..world.ctx()
        };
        let manifest = box_manifest(&ctx, "default", &[]).unwrap();
        assert!(manifest.envelopes.is_empty());
        assert_eq!(world.runner.count("ssh"), 0);
    }

    #[test]
    fn courier_helper_answers_only_after_the_taken_cursor() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let dir = project.record_dir_for_write("events").unwrap();
        std::fs::write(
            dir.join("t-0001-1-1.toml"),
            box_event_bytes("t-0001-1-1", ""),
        )
        .unwrap();
        let skipped = box_manifest(
            &world.ctx(),
            "default",
            &[("demo".into(), "t-0001-1-1".into())],
        )
        .unwrap();
        assert!(skipped.envelopes.is_empty());
        let fresh = box_manifest(&world.ctx(), "default", &[]).unwrap();
        assert_eq!(fresh.envelopes[0].event, "t-0001-1-1");
    }

    #[test]
    fn courier_reads_box_lane_progress_in_the_existing_helper_call() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("ade");
        let bin = home.path().join("bin");
        let repo = home.path().join("repo");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        project.record_dir_for_write("lanes").unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let git = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .output()
            .unwrap();
        assert!(git.status.success());
        assert!(
            std::process::Command::new("git")
                .args([
                    "-C",
                    repo.to_str().unwrap(),
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=t@e",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "initial"
                ])
                .status()
                .unwrap()
                .success()
        );
        let head = std::process::Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "rev-parse", "HEAD"])
            .output()
            .unwrap();
        let sha = String::from_utf8(head.stdout).unwrap().trim().to_string();
        let branch = std::process::Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "symbolic-ref",
                "--short",
                "HEAD",
            ])
            .output()
            .unwrap();
        let branch = String::from_utf8(branch.stdout).unwrap().trim().to_string();
        std::fs::write(
            root.join("demo/.state/lanes/t-1.toml"),
            format!(
                "pane_id = \"w:p\"\nbox_worktree = \"{}\"\nbranch = \"{branch}\"\n",
                repo.display()
            ),
        )
        .unwrap();
        let fake = bin.join("herdr");
        std::fs::write(&fake, "#!/bin/sh\ncase \"$*\" in\n  *'agent list'*) echo '{\"result\":{\"agents\":[]}}';;\n  *'pane list'*) echo '{\"result\":{\"panes\":[{\"workspace_id\":\"w\",\"tab_id\":\"w:t\",\"pane_id\":\"w:p\",\"cwd\":\"/repo\"}]}}';;\n  *'pane read'*) printf 'working';;\nesac\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env =
            crate::paths::Env::for_test(home.path(), &[("HERDR_BIN_PATH", fake.to_str().unwrap())]);
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: home.path().join("cfg"),
            runner: &crate::runner::RealRunner,
            detached_ticker: false,
        };
        let manifest = box_manifest(&ctx, "default", &[]).unwrap();
        let progress = &manifest.progress[0].1;
        assert_eq!(manifest.progress[0].0, ("demo".into(), "t-1".into()));
        assert_eq!(progress.head, sha);
        assert_eq!(progress.screen, thread::sha256_hex(b"working"));
    }

    #[test]
    fn recovered_box_attempt_ignores_old_identity_until_its_start_timeout() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = thread::allocate(&project, |t| {
            t.status = thread::Status::Starting;
            t.machine = "buildbox".into();
            t.machine_id = "abc".into();
            t.attempt = 2;
            t.workspace_id = "w9".into();
            t.tab_id = "w9:t2".into();
            t.pane_id = "w9:p2".into();
            t.cwd = "/lane".into();
            t.startup_wait_started = "2026-10-03T00:00:00Z".into();
            t.launch.ready_timeout_ms = 60_000;
            t.identity.workspace_id = "w9".into();
            t.identity.tab_id = "w9:t1".into();
            t.identity.pane_id = "w9:p1".into();
            t.identity.process = Some(crate::contracts::ProcessIdentity {
                pid: 42,
                argv0: "pi".into(),
            });
            t.identity.agent_session = Some("saved-session".into());
        })
        .unwrap();
        let pane = Pane {
            workspace_id: lane.workspace_id.clone(),
            tab_id: lane.tab_id.clone(),
            pane_id: lane.pane_id.clone(),
            cwd: lane.cwd.clone(),
        };
        world.runner.on("pane list", crate::runner::fake::ok(
            r#"{"result":{"panes":[{"workspace_id":"w9","tab_id":"w9:t2","pane_id":"w9:p2","cwd":"/lane"}]}}"#,
        ));
        world.runner.on("pane process-info", crate::runner::fake::ok(
            r#"{"result":{"process_info":{"pane_id":"w9:p2","foreground_processes":[{"pid":5,"name":"bash"}]}}}"#,
        ));
        let ctx = world.ctx();
        let check = |now| {
            assert!(
                observation_pass(
                    &ctx,
                    &project,
                    ObservationView {
                        machine_id: "abc",
                        threads: &[thread::load(&project, &lane.id).unwrap()],
                        agents: &[],
                        panes: std::slice::from_ref(&pane),
                        boot_id: "boot-1",
                        now,
                    }
                )
                .is_empty()
            );
        };
        for _ in 0..2 {
            check(at("2026-10-03T00:00:59Z"));
        }
        let saved = thread::load(&project, &lane.id).unwrap();
        assert_eq!(saved.status, thread::Status::Starting);
        assert_eq!(saved.attempt, 2);
        assert_eq!(
            saved.identity.agent_session.as_deref(),
            Some("saved-session")
        );
        assert!(
            !events::remote_state(&project, "abc")
                .missing
                .contains_key(&lane.id)
        );
        assert_eq!(world.runner.count("pane process-info"), 0);
        for _ in 0..2 {
            check(at("2026-10-03T00:01:00Z"));
        }
        assert_eq!(
            thread::load(&project, &lane.id).unwrap().status,
            thread::Status::Starting
        );
        assert_eq!(
            world.runner.count("pane process-info"),
            0,
            "a pre-launch shell is not a dead agent"
        );
        thread::update(&project, &lane.id, |t| t.launch_attempts = 1).unwrap();
        for _ in 0..2 {
            check(at("2026-10-03T00:01:00Z"));
        }
        let failed = thread::load(&project, &lane.id).unwrap();
        assert_eq!(failed.status, thread::Status::Failed);
        assert_eq!(
            failed.failure_class,
            crate::contracts::FailureClass::ProcessGone
        );
        assert!(failed.start_notices.iter().any(|n| n.line.contains("GONE")));
    }

    #[test]
    fn courier_does_not_fail_a_placed_reviewer_before_agent_launch() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let lane = thread::allocate(&project, |t| {
            t.role = "reviewer".into();
            t.machine = "buildbox".into();
            t.machine_id = "abc".into();
            t.pane_id = "w9:p9".into();
            t.workspace_id = "w9".into();
            t.tab_id = "w9:t9".into();
            t.cwd = "/box/review".into();
            t.status = thread::Status::Starting;
            t.attempt = 2;
            t.launch.attempt = 2;
            t.launch.same_recipe_retries = 1;
            t.launch_attempts = 0;
            t.startup_wait_started.clear();
        })
        .unwrap();
        let runner = crate::runner::fake::FakeRunner::new();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let ctx = Ctx {
            env: &env,
            root: root.path().to_path_buf(),
            config_dir: root.path().join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let now = at("2026-09-19T00:00:00Z");
        let mut stale = events::remote_state(&project, "abc");
        stale.boot_id = "boot-1".into();
        stale.gone.insert(lane.id.clone());
        stale.missing.insert(lane.id.clone(), 2);
        events::save_remote_state(&project, "abc", &stale).unwrap();
        for _ in 0..1 {
            let errors = observation_pass(
                &ctx,
                &project,
                ObservationView {
                    machine_id: "abc",
                    threads: std::slice::from_ref(&lane),
                    agents: &[],
                    panes: &[],
                    boot_id: "boot-2",
                    now,
                },
            );
            assert!(errors.is_empty(), "{errors:?}");
        }
        let state = events::remote_state(&project, "abc");
        assert!(!state.gone.contains(&lane.id));
        assert!(!state.pending_gone.contains(&lane.id));
        assert_eq!(state.missing.get(&lane.id), Some(&1));
        let unchanged = thread::load(&project, &lane.id).unwrap();
        assert_eq!(unchanged.attempt, 2);
        assert_eq!(unchanged.launch.same_recipe_retries, 1);
        assert_eq!(unchanged.status, thread::Status::Starting);

        // A pane placed after the second snapshot must not spend a retry.
        runner.on(
            "--machine abc pane list",
            crate::runner::fake::ok(
                "{\"result\":{\"panes\":[{\"pane_id\":\"w9:p9\",\"tab_id\":\"w9:t9\",\"workspace_id\":\"w9\",\"cwd\":\"/box/review\"}]}}",
            ),
        );
        assert!(
            observation_pass(
                &ctx,
                &project,
                ObservationView {
                    machine_id: "abc",
                    threads: std::slice::from_ref(&lane),
                    agents: &[],
                    panes: &[],
                    boot_id: "boot-2",
                    now,
                }
            )
            .is_empty()
        );
        assert_eq!(thread::load(&project, &lane.id).unwrap().attempt, 2);
        assert!(
            !events::remote_state(&project, "abc")
                .missing
                .contains_key(&lane.id)
        );
    }

    #[test]
    fn missing_box_pane_retries_before_launch_then_sends_failed_after_second_loss() {
        let root = tempfile::tempdir().unwrap();
        let cfg = root.path().join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(
            cfg.join("config.toml"),
            "[routing]\ndefault = \"claude_coordinator_opus\"\nretries = 1\n",
        )
        .unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let lane = thread::allocate(&project, |t| {
            t.role = "reviewer".into();
            t.machine = "buildbox".into();
            t.machine_id = "abc".into();
            t.pane_id = "w9:p9".into();
            t.workspace_id = "w9".into();
            t.tab_id = "w9:t9".into();
            t.cwd = "/box/review".into();
            t.status = thread::Status::Open;
            t.attempt = 1;
            t.launch.attempt = 1;
            t.launch.recipe_id = "claude_coordinator_opus".into();
            t.launch.kind = "claude".into();
            t.prompt_pending = true;
            t.launch_attempts = 0;
        })
        .unwrap();
        std::fs::write(thread::task_path(&project, &lane.id), "Review this change.").unwrap();
        let runner = crate::runner::fake::FakeRunner::new();
        runner.on(
            "agent start --help",
            crate::runner::fake::ok("[possible values: pi, claude, agy]"),
        );
        runner.on(
            "--machine abc pane list",
            crate::runner::fake::ok("{\"result\":{\"panes\":[]}}"),
        );
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let ctx = Ctx {
            env: &env,
            root: root.path().into(),
            config_dir: cfg,
            runner: &runner,
            detached_ticker: false,
        };
        let snapshot = |record: &thread::Thread| {
            observation_pass(
                &ctx,
                &project,
                ObservationView {
                    machine_id: "abc",
                    threads: std::slice::from_ref(record),
                    agents: &[],
                    panes: &[],
                    boot_id: "boot-1",
                    now: at("2026-09-19T00:00:00Z"),
                },
            )
        };
        assert!(snapshot(&lane).is_empty());
        assert_eq!(thread::load(&project, &lane.id).unwrap().attempt, 1);
        assert_eq!(
            events::remote_state(&project, "abc").missing.get(&lane.id),
            Some(&1)
        );
        assert_eq!(runner.count("--machine abc pane list"), 0);
        assert!(snapshot(&lane).is_empty());
        let retried = thread::load(&project, &lane.id).unwrap();
        assert_eq!(retried.attempt, 2);
        assert_eq!(retried.launch.same_recipe_retries, 1);
        assert!(retried.recovery_pending);
        assert!(retried.start_notices.iter().any(|n| {
            n.line.contains("GONE")
                && n.line.contains(
                    "automatic same-recipe retry selected for attempt 2; wait for startup",
                )
        }));
        assert_eq!(runner.count("--machine abc pane list"), 1);

        // A stale first-attempt streak cannot count against the new pane.
        let mut stale = events::remote_state(&project, "abc");
        stale.missing.insert(lane.id.clone(), 2);
        stale
            .missing_identity
            .insert(lane.id.clone(), "1:w9:w9:t9:w9:p9".into());
        events::save_remote_state(&project, "abc", &stale).unwrap();
        let second = thread::update(&project, &lane.id, |t| {
            t.pane_id = "w10:p1".into();
            t.tab_id = "w10:t1".into();
            t.workspace_id = "w10".into();
            t.status = thread::Status::Open;
            t.recovery_pending = false;
            t.launch_attempts = 0;
        })
        .unwrap();
        assert!(snapshot(&second).is_empty());
        assert_eq!(
            events::remote_state(&project, "abc").missing.get(&lane.id),
            Some(&1)
        );
        assert_eq!(thread::load(&project, &lane.id).unwrap().attempt, 2);
        assert!(snapshot(&second).is_empty());
        let exhausted = thread::load(&project, &lane.id).unwrap();
        assert_eq!(exhausted.status, thread::Status::Failed);
        assert!(!exhausted.recovery_pending);
        assert!(exhausted.start_notices.iter().any(|n| {
            n.line.starts_with(&format!("GONE {} attempt 2:", lane.id))
                && n.line
                    .contains(&crate::threads::retry_command("demo", &lane.id))
        }));
    }

    #[test]
    fn a_box_lane_signal_waits_until_the_coordinator_can_receive_it() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let lane = thread::allocate(&project, |t| {
            t.machine = "buildbox".into();
            t.machine_id = "abc".into();
            t.launch_attempts = 1;
            t.pane_id = "w9:p9".into();
            t.workspace_id = "w9".into();
            t.tab_id = "w9:t9".into();
            t.cwd = "/box/wt".into();
            t.agent_name = "lane".into();
        })
        .unwrap();
        let runner = crate::runner::fake::FakeRunner::new();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let ctx = Ctx {
            env: &env,
            root: root.path().to_path_buf(),
            config_dir: root.path().join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let now = at("2026-09-19T00:00:00Z");
        let threads = [lane.clone()];
        observation_pass(
            &ctx,
            &project,
            ObservationView {
                machine_id: "abc",
                threads: &threads,
                agents: &[],
                panes: &[],
                boot_id: "boot-1",
                now,
            },
        );
        assert!(
            !events::remote_state(&project, "abc")
                .gone
                .contains(&lane.id)
        );
        observation_pass(
            &ctx,
            &project,
            ObservationView {
                machine_id: "abc",
                threads: &threads,
                agents: &[],
                panes: &[],
                boot_id: "boot-1",
                now,
            },
        );
        let state = events::remote_state(&project, "abc");
        assert!(
            !state.gone.contains(&lane.id),
            "a missing coordinator must not consume GONE: {state:?}"
        );

        // A reboot records the new boot but still leaves GONE pending while
        // there is no coordinator to receive it.
        observation_pass(
            &ctx,
            &project,
            ObservationView {
                machine_id: "abc",
                threads: &threads,
                agents: &[],
                panes: &[],
                boot_id: "boot-2",
                now,
            },
        );
        let state = events::remote_state(&project, "abc");
        assert_eq!(state.boot_id, "boot-2");
        assert!(!state.gone.contains(&lane.id));
        assert!(state.pending_gone.contains(&lane.id));
        observation_pass(
            &ctx,
            &project,
            ObservationView {
                machine_id: "abc",
                threads: &threads,
                agents: &[],
                panes: &[],
                boot_id: "boot-2",
                now,
            },
        );
        assert!(
            events::remote_state(&project, "abc")
                .pending_gone
                .contains(&lane.id)
        );
    }

    fn box_event_bytes(id: &str, artifact: &str) -> Vec<u8> {
        let event = crate::contracts::Event {
            id: id.into(),
            op: id.into(),
            usage: None,
            thread: "t-0001".into(),
            attempt: 1,
            recipient: crate::contracts::Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 1,
            },
            created: "2026-09-19T00:00:00Z".into(),
            payload: crate::contracts::EventPayload {
                done: Some(crate::contracts::DonePayload {
                    has_changes: None,
                    sha: "abc".into(),
                    report_path: ".reports/t-0001.md".into(),
                    artifact: artifact.into(),
                    attestation: None,
                    published_ref: None,
                }),
                waiting: None,
                failed: None,
            },
        };
        events::bytes(&event).unwrap()
    }

    fn courier_ctx<'a>(
        root: &'a std::path::Path,
        env: &'a crate::paths::Env,
        runner: &'a dyn crate::runner::Runner,
    ) -> Ctx<'a> {
        let config_dir = root.join("cfg");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            r#"[machines.box]
target = "me@box"
session = "default"
home = "/home/agent"
root = "/home/agent/.herdr-ade"
worktrees = "/home/agent/projects"
build = "/home/agent/build/lanes"
path = "/home/agent/.local/bin:/usr/bin:/bin"
ade_bin = "/home/agent/.local/bin/herdr-ade"
pi_bin = "/home/agent/.local/bin/herdr-pi"
"#,
        )
        .unwrap();
        Ctx {
            env,
            root: root.to_path_buf(),
            config_dir,
            runner,
            detached_ticker: false,
        }
    }

    #[test]
    fn courier_imports_every_project_on_the_machine_after_the_taken_cursor() {
        let root = tempfile::tempdir().unwrap();
        let alpha = project::create(root.path(), "alpha", "", vec![]).unwrap();
        let beta = project::create(root.path(), "beta", "", vec![]).unwrap();
        let gamma = project::create(root.path(), "gamma", "", vec![]).unwrap();
        for project in [&alpha, &beta, &gamma] {
            thread::allocate(project, |t| {
                t.machine = "box".into();
                t.machine_id = "1".into();
                t.pane_id = "w2:p1".into();
            })
            .unwrap();
        }
        let report = b"report body\n";
        let artifact_hash = thread::sha256_hex(report);
        let event = box_event_bytes("t-0001-1-1", &artifact_hash);
        let event_hash = thread::sha256_hex(&event);
        let runner = crate::runner::fake::FakeRunner::new();
        runner.on(
            "machine list --json",
            crate::runner::fake::ok(
                r#"[{"id":"1","label":"box","target":"me@box","session":"default","enabled":true}]"#,
            ),
        );
        let mut manifest = CourierManifest {
            boot_id: "boot-1".into(),
            agents: Some(Vec::new()),
            panes: Some(Vec::new()),
            ..Default::default()
        };
        for slug in ["alpha", "beta"] {
            manifest.envelopes.push(BoxEnvelope {
                slug: slug.into(),
                event: "t-0001-1-1".into(),
                event_path: format!("/box/{slug}/.state/events/t-0001-1-1.toml"),
                event_hash: event_hash.clone(),
                artifact_path: format!("/box/{slug}/.state/artifacts/{artifact_hash}"),
                artifact_hash: artifact_hash.clone(),
            });
            manifest.receipts.push(CompletionReceipt {
                slug: slug.into(),
                event: "t-0001-1-1".into(),
                event_hash: event_hash.clone(),
                artifact_hash: artifact_hash.clone(),
            });
        }
        manifest.bootstraps.push(BootstrapReceipt {
            slug: "alpha".into(),
            thread: "t-0001".into(),
            brief_hash: String::new(),
            pane: "w2:p1".into(),
        });
        let manifest = crate::box_helper::tests::ready(manifest);
        let manifest_for_ssh = manifest.clone();
        runner.on_fn(
            |cmd| cmd.program == "ssh",
            move |_| Ok(crate::runner::fake::ok(&manifest_for_ssh)),
        );
        let event_bytes = event.clone();
        let report_bytes = report.to_vec();
        let artifact_for_scp = artifact_hash.clone();
        runner.on_fn(
            |cmd| cmd.program == "scp",
            move |cmd| {
                let dir = std::path::PathBuf::from(cmd.args.last().unwrap().trim_end_matches('/'));
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join("t-0001-1-1.toml"), &event_bytes)?;
                std::fs::write(dir.join(&artifact_for_scp), &report_bytes)?;
                Ok(crate::runner::fake::ok(""))
            },
        );
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let ctx = courier_ctx(root.path(), &env, &runner);
        let outcome = courier(&ctx, &[&alpha, &beta, &gamma], "1").unwrap();
        assert_eq!(outcome.machine_id, "1");
        assert_eq!(outcome.boot_id, "boot-1");
        assert_eq!(events::list(&alpha).len(), 1);
        assert_eq!(events::list(&beta).len(), 1);
        assert!(
            events::remote_state(&alpha, "1")
                .taken
                .contains_key("t-0001-1-1")
        );
        assert!(
            events::remote_state(&beta, "1")
                .taken
                .contains_key("t-0001-1-1")
        );
        assert!(
            !events::remote_state(&gamma, "1").last_pass.is_empty(),
            "a project with no new envelope still heard from the box"
        );
        let imported = events::load(&alpha, "t-0001-1-1").unwrap();
        assert_eq!(
            imported.payload.done.unwrap().report_path,
            ".reports/t-0001.md"
        );
        assert_eq!(
            thread::load(&alpha, "t-0001").unwrap().bootstrap,
            "acknowledged"
        );

        // The cursor now skips the same envelopes: no second fetch.
        let before = runner.count("scp");
        courier(&ctx, &[&alpha, &beta, &gamma], "1").unwrap();
        assert_eq!(runner.count("scp"), before);
        // The imported events are the durable records; courier delivery does
        // not duplicate them in the inbox, including after a retry.
        for project in [&alpha, &beta, &gamma] {
            assert!(inbox::unhandled(project).is_empty());
        }
    }

    #[test]
    fn courier_refuses_a_receipt_that_disagrees_with_the_fetched_bytes() {
        let root = tempfile::tempdir().unwrap();
        let alpha = project::create(root.path(), "alpha", "", vec![]).unwrap();
        thread::allocate(&alpha, |t| {
            t.machine = "box".into();
            t.machine_id = "1".into();
        })
        .unwrap();
        let report = b"report body\n";
        let artifact_hash = thread::sha256_hex(report);
        let event = box_event_bytes("t-0001-1-1", &artifact_hash);
        let event_hash = thread::sha256_hex(&event);
        let runner = crate::runner::fake::FakeRunner::new();
        runner.on(
            "machine list --json",
            crate::runner::fake::ok(
                r#"[{"id":"1","label":"box","target":"me@box","session":"default","enabled":true}]"#,
            ),
        );
        let manifest = crate::box_helper::tests::ready(CourierManifest {
            boot_id: "boot-1".into(),
            envelopes: vec![BoxEnvelope {
                slug: "alpha".into(),
                event: "t-0001-1-1".into(),
                event_path: "/box/alpha/.state/events/t-0001-1-1.toml".into(),
                event_hash,
                artifact_path: format!("/box/alpha/.state/artifacts/{artifact_hash}"),
                artifact_hash: artifact_hash.clone(),
            }],
            receipts: vec![CompletionReceipt {
                slug: "alpha".into(),
                event: "t-0001-1-1".into(),
                event_hash: "deadbeef".into(),
                artifact_hash: artifact_hash.clone(),
            }],
            ..Default::default()
        });
        let manifest_for_ssh = manifest.clone();
        runner.on_fn(
            |cmd| cmd.program == "ssh",
            move |_| Ok(crate::runner::fake::ok(&manifest_for_ssh)),
        );
        let event_bytes = event.clone();
        let report_bytes = report.to_vec();
        let artifact_for_scp = artifact_hash.clone();
        runner.on_fn(
            |cmd| cmd.program == "scp",
            move |cmd| {
                let dir = std::path::PathBuf::from(cmd.args.last().unwrap().trim_end_matches('/'));
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join("t-0001-1-1.toml"), &event_bytes)?;
                std::fs::write(dir.join(&artifact_for_scp), &report_bytes)?;
                Ok(crate::runner::fake::ok(""))
            },
        );
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let ctx = courier_ctx(root.path(), &env, &runner);
        let error = courier(&ctx, &[&alpha], "box").unwrap_err().to_string();
        assert!(error.contains("receipt_mismatch"), "{error}");
        assert!(events::list(&alpha).is_empty());
        assert!(
            events::remote_state(&alpha, "1").taken.is_empty(),
            "the cursor must not move on a receipt mismatch"
        );
    }

    #[test]
    fn a_done_event_is_not_delivered_until_its_sha_is_on_the_publish_remote() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(
            root.path(),
            "demo",
            "",
            vec![crate::project::Repo {
                path: "/repo".into(),
                machine: None,
                box_path: Some("/box/repo".into()),
                publish_url: Some("https://github.com/uguryildirim24/herdr-ade.git".into()),
                ..crate::project::Repo::default()
            }],
        )
        .unwrap();
        let lane = thread::allocate(&project, |t| {
            t.machine = "box".into();
            t.machine_id = "1".into();
            t.repo = "/repo".into();
            t.branch = "hp/demo/t-0001".into();
        })
        .unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);

        let fetch_fails = crate::runner::fake::FakeRunner::new();
        fetch_fails.on(
            "git -C /repo remote get-url fork",
            crate::runner::fake::ok("https://github.com/uguryildirim24/herdr-ade.git\n"),
        );
        fetch_fails.on("git -C /repo remote", crate::runner::fake::ok("fork\n"));
        fetch_fails.on(
            "git -C /repo fetch",
            crate::runner::fake::fail(1, "could not fetch"),
        );
        let ctx = courier_ctx(root.path(), &env, &fetch_fails);
        let error = verify_published_sha(&ctx, &project, &lane, "abc", None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("published_fetch_failed"), "{error}");

        let sha_absent = crate::runner::fake::FakeRunner::new();
        sha_absent.on(
            "git -C /repo remote get-url fork",
            crate::runner::fake::ok("https://github.com/uguryildirim24/herdr-ade.git\n"),
        );
        sha_absent.on("git -C /repo remote", crate::runner::fake::ok("fork\n"));
        sha_absent.on("git -C /repo fetch", crate::runner::fake::ok(""));
        sha_absent.on(
            "git -C /repo merge-base --is-ancestor",
            crate::runner::fake::fail(1, ""),
        );
        let runner = sha_absent;
        let ctx = courier_ctx(root.path(), &env, &runner);
        let error = verify_published_sha(&ctx, &project, &lane, "abc", None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("published_sha_missing"), "{error}");

        for output in [
            crate::runner::fake::ok("abc\n"),
            crate::runner::Output {
                stdout: "abc\n".into(),
                ..crate::runner::fake::fail(128, "broken ref")
            },
            crate::runner::fake::timeout(),
        ] {
            let success = output.success();
            let second = crate::runner::fake::FakeRunner::new();
            second.on(
                "git -C /repo remote get-url fork",
                crate::runner::fake::ok("https://github.com/uguryildirim24/herdr-ade.git\n"),
            );
            second.on("git -C /repo remote", crate::runner::fake::ok("fork\n"));
            second.on("git -C /repo fetch", crate::runner::fake::ok(""));
            second.on("git -C /repo rev-parse FETCH_HEAD", output);
            let ctx = courier_ctx(root.path(), &env, &second);
            assert_eq!(
                verify_published_sha(
                    &ctx,
                    &project,
                    &lane,
                    "abc",
                    Some("seals/hp/demo/t-0001/abc"),
                )
                .is_ok(),
                success
            );
            assert!(second.calls.borrow().iter().any(|call| {
                call.args
                    .iter()
                    .any(|arg| arg == "refs/heads/seals/hp/demo/t-0001/abc")
            }));
        }
    }
}
