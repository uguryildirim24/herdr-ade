//! The ticker's per-project steps beyond thread state: inbox items, the nudge,
//! pull requests, routines, auto-resolve. Each is "compare with last time,
//! write an inbox item when it changed".

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::herdr::{Agent, Herdr, Pane};
use crate::paths::Ctx;
use crate::project::{self, Project, Settings};
use crate::thread::{self, CopyOutcome, Group, Status, Thread};
use crate::threads;
use crate::{events, inbox, pr, routine};

pub const NUDGE_TEXT: &str =
    "[herdr-ade ticker: automated, not the user, approves nothing] New inbox items. Run context.";
pub const PR_INTERVAL_SECS: i64 = 120;
pub const DONE_RETENTION_DAYS: u64 = 30;
const DEFAULT_OUTAGE_SECS: i64 = 600;

/// `.state/ticker.json`: what the ticker compared against last time.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct State {
    pub last_pr_check: String,
    pub prs: BTreeMap<String, pr::Summary>,
    /// thread id -> the pull request URL an "ignored" item was written for.
    pub pr_ignored: BTreeMap<String, String>,
    /// thread id -> report hash a "bad PR: line" note was written for.
    pub pr_line_noted: BTreeMap<String, String>,
    pub routines: routine::States,
    /// Hashes of files a `config-error` item was already written for.
    pub config_errors: BTreeSet<String>,
    /// Hash of the set of unseen item ids that was last nudged.
    pub nudged: String,
    pub session_item_written: bool,
}

pub fn load_state(project: &Project) -> State {
    project::read_json(&project.state_dir().join("ticker.json")).unwrap_or_default()
}

/// Only the ticker writes this file, so its own read-modify-write is safe; the
/// write still happens under the project lock, like every `.state/` write.
pub fn save_state(project: &Project, state: &State) -> Result<()> {
    let _lock = project.lock()?;
    project::write_json(&project.state_dir().join("ticker.json"), state)
}

/// Delivers every sealed event whose first transport submission is not yet in
/// its journal. The event, not the typed line or report hash, is authoritative.
/// Each event is independent: one that cannot be delivered never holds back
/// the others. An event with any journal line (submitted, acknowledged or
/// handled) is not typed again, and one for a superseded lane attempt is
/// left as it is, sealed and undelivered.
pub fn deliver_events(ctx: &Ctx, project: &Project) -> Result<()> {
    let mut first: Option<anyhow::Error> = None;
    for event in crate::events::list(project) {
        let states = match crate::events::states(project, &event.id) {
            Ok(states) => states,
            Err(error) => {
                first.get_or_insert(error);
                continue;
            }
        };
        if !states.is_empty() {
            // Typed to a coordinator that has since been replaced and never
            // acknowledged: the current one gets a recipient-changed item.
            let settled = states.iter().any(|s| {
                matches!(
                    s,
                    crate::contracts::DeliveryState::Acknowledged
                        | crate::contracts::DeliveryState::Handled
                )
            });
            if !settled
                && let Some(current) = project.coordinator()
                && (current.pane_id != event.recipient.pane
                    || current.attempt() != event.recipient.coordinator_attempt)
                && let Err(error) = inbox::write_event(
                    project,
                    &event,
                    "recipient-changed",
                    "a lane event was typed to an earlier coordinator binding",
                )
            {
                first.get_or_insert(error);
            }
            continue;
        }
        if thread::load(project, &event.thread)
            .is_ok_and(|lane| lane.attempt.max(1) != event.attempt)
        {
            continue;
        }
        if let Err(error) = deliver_event(ctx, project, &event) {
            first.get_or_insert(error.context(format!("event {}", event.id)));
        }
    }
    first.map_or(Ok(()), Err)
}

pub fn deliver_event(ctx: &Ctx, project: &Project, event: &crate::contracts::Event) -> Result<()> {
    let coordinator = project
        .coordinator()
        .ok_or_else(|| anyhow::anyhow!("recipient_unavailable: project has no coordinator"))?;
    if coordinator.pane_id != event.recipient.pane
        || coordinator.attempt() != event.recipient.coordinator_attempt
    {
        inbox::write_event(
            project,
            event,
            "recipient-changed",
            "a sealed lane event belongs to an earlier coordinator binding",
        )?;
        return Ok(());
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &coordinator.socket, ctx.runner);
    let lane = thread::load(project, &event.thread)?;
    if lane.attempt.max(1) != event.attempt {
        bail!(
            "stale_attempt: event {} is not for the current lane attempt",
            event.id
        );
    }
    let kind = if event.payload.done.is_some() {
        "done"
    } else {
        "waiting"
    };
    let summary = match (&event.payload.done, &event.payload.waiting) {
        (Some(done), None) => format!(
            "{} completed with report {} at {}",
            event.thread, done.report_path, done.sha
        ),
        (None, Some(waiting)) => format!("{} is waiting: {}", event.thread, waiting.text),
        _ => bail!("event_payload_invalid: {}", event.id),
    };
    inbox::write_event(project, event, kind, &summary)?;

    // A box lane's tokens were set by the box's own `ha done`; the Mac has no
    // socket into that server, so delivery never projects them again
    // (SPEC-remote §4.3).
    if !lane.is_remote() && !lane.pane_id.is_empty() {
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

    // One writer types into the coordinator's pane; `!native` in talk
    // suspends it and the event waits for the next tick (D18).
    let _writer = crate::talk::writer_lock(project)?;
    if crate::talk::writer_suspended(project) {
        return Ok(());
    }
    let agent = herdr.agent_list()?.into_iter().find(|agent| {
        agent.pane_id == event.recipient.pane
            && agent.name == coordinator.agent_name
            && agent.ready()
    });
    let Some(_agent) = agent else {
        return Ok(());
    };
    herdr.agent_prompt(&event.recipient.pane, &crate::events::typed_line(event)?)?;
    crate::events::append_delivery(
        project,
        &event.id,
        crate::contracts::DeliveryState::Submitted,
    )
}

/// D11: records the digest of `config.toml` and `RULES.md`; when it moved
/// since the last tick, one `config-changed` item. Best-effort, not tamper
/// evidence.
pub fn config_changed(project: &Project, digest: &str) -> Result<()> {
    let path = project.state_dir().join("policy_hash");
    let recorded = std::fs::read_to_string(&path).ok();
    if recorded.as_deref().map(str::trim) == Some(digest) {
        return Ok(());
    }
    if recorded.is_some() {
        inbox::write(
            project,
            "config-changed",
            "policy",
            &format!("config.toml or RULES.md changed; the policy hash is now {digest}"),
            "",
        )?;
    }
    project::write_atomic(&path, digest.as_bytes())
}

/// Continuous-failure tracking for `gh` or a machine: one item when it has
/// failed for the threshold, one more when it recovers, nothing for blips.
#[derive(Debug, Clone, Default)]
pub struct Outage {
    failing_since: Option<jiff::Timestamp>,
    reported: bool,
    pub last_error: String,
}

#[derive(Debug, PartialEq)]
pub enum OutageEvent {
    Down,
    Recovered,
}

impl Outage {
    pub fn record(
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
        if !self.reported && now.as_second() - since.as_second() >= threshold_secs {
            self.reported = true;
            return Some(OutageEvent::Down);
        }
        None
    }
}

pub const REMOTE_EVERY_TICKS: u64 = 4;
pub const SKIP_TICKS_AFTER_FAILURE: u64 = 8;

#[derive(Debug, Clone, Default)]
pub struct MachineMemory {
    pub outage: Outage,
    /// Not polled again before this tick: one sleeping machine must not slow
    /// the other projects' ticks.
    pub skip_until_tick: u64,
    pub last_poll_tick: u64,
}

/// What the ticker process remembers between ticks (not persisted).
pub struct Memory {
    pub started: jiff::Timestamp,
    pub gh: Outage,
    pub outage_secs: i64,
    pub tick: u64,
    pub machines: BTreeMap<String, MachineMemory>,
}

impl Memory {
    pub fn new(ctx: &Ctx) -> Memory {
        Memory {
            started: jiff::Timestamp::now(),
            gh: Outage::default(),
            // Overridable so an outage can be exercised without waiting ten minutes.
            outage_secs: ctx
                .env
                .var("HERDR_ADE_OUTAGE_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_OUTAGE_SECS),
            tick: 0,
            machines: BTreeMap::new(),
        }
    }

    /// Remote machines are polled every fourth tick (about a minute), and not
    /// at all for eight ticks after a failure.
    pub fn machine_is_due(&mut self, machine: &str) -> bool {
        let tick = self.tick;
        let entry = self.machines.entry(machine.to_string()).or_default();
        let due = tick >= entry.skip_until_tick
            && (entry.last_poll_tick == 0 || tick >= entry.last_poll_tick + REMOTE_EVERY_TICKS);
        if due {
            entry.last_poll_tick = tick;
        }
        due
    }

    pub fn record_machine(
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
pub fn write_machine_outage(
    project: &Project,
    machine: &str,
    event: Option<OutageEvent>,
    memory: &Memory,
) -> Result<()> {
    match event {
        Some(OutageEvent::Down) => {
            let error = memory
                .machines
                .get(machine)
                .map(|m| pr::sanitize(&m.outage.last_error))
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
pub const COURIER_TIMEOUT: Duration = Duration::from_secs(30);

/// One envelope the box helper reported (SPEC-remote §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxEnvelope {
    pub slug: String,
    pub event: String,
    pub event_path: String,
    pub event_hash: String,
    /// Empty for a `waiting` envelope.
    pub artifact_path: String,
    pub artifact_hash: String,
}

/// What one box helper call returned, after the taken cursor it was asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CourierManifest {
    pub boot_id: String,
    pub free_bytes: u64,
    pub envelopes: Vec<BoxEnvelope>,
}

/// The stable identity a courier pass resolved and the boot it saw, so the
/// caller can key its lane state and detect a reboot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourierOutcome {
    pub machine_id: String,
    pub boot_id: String,
}

/// The box-local helper: it reads the box ADE root after the taken cursor and
/// prints one tab-separated record per fact (SPEC-remote §4.3). It makes no
/// bridge call; the courier's `herdr --machine` lists are the live view.
pub fn courier_helper(box_root: &str) -> String {
    let root = crate::remote::quote(box_root);
    format!(
        "set -u\n\
         root={root}\n\
         printf 'boot\\t%s\\n' \"$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true)\"\n\
         avail=$(df -B1 --output=avail / 2>/dev/null | tail -n1 | tr -d ' ')\n\
         printf 'free\\t%s\\n' \"${{avail:-0}}\"\n\
         [ -d \"$root\" ] || exit 0\n\
         for dir in \"$root\"/*/events; do\n\
           [ -d \"$dir\" ] || continue\n\
           slug=${{dir%/events}}; slug=${{slug##*/}}\n\
           for f in \"$dir\"/*.toml; do\n\
             [ -f \"$f\" ] || continue\n\
             id=${{f##*/}}; id=${{id%.toml}}\n\
             case \"$id\" in .*) continue;; esac\n\
             h=$(sha256sum \"$f\" | cut -d' ' -f1)\n\
             a=$(sed -n 's/^artifact = \"\\([^\"]*\\)\"/\\1/p' \"$f\" | head -n1)\n\
             if [ -n \"$a\" ]; then\n\
               printf 'event\\t%s\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"$slug\" \"$id\" \"$f\" \"$h\" \"$root/$slug/artifacts/$a\" \"$a\"\n\
             else\n\
               printf 'event\\t%s\\t%s\\t%s\\t%s\\t-\\t-\\n' \"$slug\" \"$id\" \"$f\" \"$h\"\n\
             fi\n\
           done\n\
         done\n"
    )
}

/// Parses the helper's tab-separated output. An unknown record is refused so a
/// helper version mismatch is loud, not silently empty.
pub fn parse_courier_manifest(text: &str) -> Result<CourierManifest> {
    let mut manifest = CourierManifest::default();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["boot", id] => manifest.boot_id = (*id).to_string(),
            ["free", bytes] => manifest.free_bytes = bytes.trim().parse().unwrap_or(0),
            [
                "event",
                slug,
                event,
                path,
                hash,
                artifact_path,
                artifact_hash,
            ] => {
                manifest.envelopes.push(BoxEnvelope {
                    slug: (*slug).to_string(),
                    event: (*event).to_string(),
                    event_path: (*path).to_string(),
                    event_hash: (*hash).to_string(),
                    artifact_path: if *artifact_path == "-" {
                        String::new()
                    } else {
                        (*artifact_path).to_string()
                    },
                    artifact_hash: if *artifact_hash == "-" {
                        String::new()
                    } else {
                        (*artifact_hash).to_string()
                    },
                });
            }
            _ => bail!("courier_manifest_invalid: {line}"),
        }
    }
    Ok(manifest)
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// One courier pass for one project on one machine (SPEC-remote §4.3): one
/// multiplexed helper call, one batched `scp`, hash checks, and a create-only
/// import into the Mac's canonical ledger. Returns the box's boot id, so the
/// caller can push GONE after a reboot. The box keeps its copies; only the
/// taken cursor advances, and only after the import is durable.
pub fn courier(ctx: &Ctx, project: &Project, machine: &str) -> Result<CourierOutcome> {
    let profile =
        crate::remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, machine)?;
    if profile.is_local() {
        bail!("courier called for the local machine");
    }
    if profile.target.is_empty() {
        bail!("machine `{machine}` has no SSH target");
    }
    let target = profile.target.clone();
    let control = ctx.root.join(".state/remote");
    let script = courier_helper(crate::contracts::BOX_ROOT);
    let out = crate::remote::ssh_courier(ctx.runner, &target, &control, &script, COURIER_TIMEOUT)?;
    if !out.success() {
        bail!("courier helper on {target}: {}", out.error_text());
    }
    let manifest = parse_courier_manifest(&out.stdout)
        .with_context(|| format!("courier helper on {target}"))?;

    let mut state = events::remote_state(project, &profile.id);
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
                Some(bytes)
            };
            events::import_box_event(project, &profile.id, &bytes, artifact.as_deref())?;
            state
                .taken
                .insert(env.event.clone(), env.event_hash.clone());
        }
        let _ = std::fs::remove_dir_all(&staging);
    }
    // The boot id is left for `remote_attention` to advance: it must see the
    // change to type GONE for the pre-reboot lanes.
    state.last_pass = project::now();
    events::save_remote_state(project, &profile.id, &state)?;
    Ok(CourierOutcome {
        machine_id: profile.id,
        boot_id: manifest.boot_id,
    })
}

/// One successful pass's live facts for a machine's box lanes.
pub struct RemoteView<'a> {
    pub machine_id: &'a str,
    pub threads: &'a [Thread],
    pub agents: &'a [Agent],
    pub panes: &'a [Pane],
    pub boot_id: &'a str,
    pub now: jiff::Timestamp,
}

/// The D8 amendment for box lanes: the fork has no push across machines, so
/// the plugin's one serialized writer types `BLOCKED` and `GONE` exactly once
/// per transition. A boot-id change types GONE for every open box lane; a pane
/// or agent absent on two consecutive successful passes does the same.
pub fn remote_attention(ctx: &Ctx, project: &Project, view: RemoteView<'_>) -> Vec<anyhow::Error> {
    let RemoteView {
        machine_id,
        threads,
        agents,
        panes,
        boot_id,
        now,
    } = view;
    let mut errors = Vec::new();
    let mut state = events::remote_state(project, machine_id);
    let boot_changed = !state.boot_id.is_empty() && !boot_id.is_empty() && state.boot_id != boot_id;
    if !boot_id.is_empty() {
        state.boot_id = boot_id.to_string();
    }
    if boot_changed {
        state.gone.clear();
        state.missing.clear();
    }
    enum Signal {
        Blocked(String),
        Gone(String),
    }
    impl Signal {
        fn line(&self) -> String {
            match self {
                Signal::Blocked(lane) => format!("BLOCKED {lane}"),
                Signal::Gone(lane) => format!("GONE {lane}"),
            }
        }
    }

    let mut signals = Vec::new();
    for lane in threads {
        let live = thread::live_state(lane, agents, panes, now);
        let blocked = live.agent_state.as_deref() == Some("blocked");
        if blocked {
            if !state.blocked.contains(&lane.id) {
                signals.push(Signal::Blocked(lane.id.clone()));
            }
        } else {
            state.blocked.remove(&lane.id);
        }
        if live.pane_exists {
            state.missing.insert(lane.id.clone(), 0);
        } else {
            let count = state.missing.entry(lane.id.clone()).or_insert(0);
            *count += 1;
        }
        let missing = *state.missing.get(&lane.id).unwrap_or(&0);
        if (boot_changed || missing >= 2) && !state.gone.contains(&lane.id) {
            signals.push(Signal::Gone(lane.id.clone()));
        }
    }
    for signal in signals {
        let line = signal.line();
        match type_remote_line(ctx, project, &line) {
            Ok(true) => match signal {
                Signal::Blocked(lane) => {
                    state.blocked.insert(lane);
                }
                Signal::Gone(lane) => {
                    state.gone.insert(lane);
                }
            },
            // A suspended or busy writer has not consumed the transition. Do
            // not mark it: the next successful pass must try again.
            Ok(false) => {}
            Err(error) => errors.push(error.context(format!("remote line `{line}`"))),
        }
    }
    if let Err(error) = events::save_remote_state(project, machine_id, &state) {
        errors.push(error.context("remote state"));
    }
    errors
}

/// Types one line into the coordinator through the same serialized writer D5
/// uses (SPEC-remote §4.3). Returns whether it was typed.
pub fn type_remote_line(ctx: &Ctx, project: &Project, text: &str) -> Result<bool> {
    let Some(record) = project.coordinator() else {
        return Ok(false);
    };
    if record.pane_id.is_empty() {
        return Ok(false);
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, ctx.runner);
    let _writer = crate::talk::writer_lock(project)?;
    if crate::talk::writer_suspended(project) {
        return Ok(false);
    }
    let ready = herdr.agent_list()?.into_iter().any(|agent| {
        agent.pane_id == record.pane_id && agent.name == record.agent_name && agent.ready()
    });
    if !ready {
        return Ok(false);
    }
    herdr.agent_prompt(&record.pane_id, text)?;
    Ok(true)
}

/// A group change seen in the cheap pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub id: String,
    pub to: Group,
    pub note: String,
}

fn thread_label(t: &Thread) -> String {
    format!("{} \"{}\"", t.id, t.title)
}

/// Step 1's inbox items, written after the copies so a Ready for review item
/// always points at a home copy that exists.
pub fn write_thread_items(
    project: &Project,
    state: &mut State,
    transitions: &[Transition],
    session_lost: bool,
    copy_notes: &BTreeMap<String, Vec<String>>,
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
                    "herdr session restarted; {open} threads need `thread restart`, and the coordinator needs `open`"
                ),
                "",
            )?;
            state.session_item_written = true;
        }
        return Ok(());
    }
    state.session_item_written = false;

    for change in transitions {
        if !matches!(
            change.to,
            Group::WaitingOnYou | Group::Landing | Group::Idle
        ) {
            continue;
        }
        let Ok(t) = thread::load(project, &change.id) else {
            continue;
        };
        let mut summary = format!(
            "{} is now {} ({})",
            thread_label(&t),
            change.to.label(),
            change.note
        );
        if change.to == Group::WaitingOnYou && !t.pane_id.is_empty() {
            summary.push_str(&format!("; it needs the user in pane {}", t.pane_id));
            if t.is_remote() {
                summary.push_str(&format!(" on machine `{}` (reach it with `herdr --remote <ssh target>`, or select the machine in herdr's sidebar)", t.machine));
            }
        }
        inbox::write(project, "thread-state", &t.id, &summary, "")?;
    }

    // Ready for review: once per report hash, so an agent that goes back and
    // forth between working and idle on an unchanged report produces nothing.
    for t in thread::list(project) {
        if t.status != Status::Open
            || t.report_hash.is_empty()
            || t.report_hash == t.last_review_item_hash
        {
            continue;
        }
        if t.last_group != Group::ReadyForReview.token() && t.last_group != Group::Landing.token() {
            continue;
        }
        let mut summary = format!(
            "{} has a new report: threads/{}.md; report bytes are not a completion",
            thread_label(&t),
            t.id
        );
        if let Some(notes) = copy_notes.get(&t.id) {
            summary.push_str(&format!(
                "; not everything was copied: {}",
                notes.join("; ")
            ));
        }
        inbox::write(project, "report-available", &t.id, &summary, "")?;
        let hash = t.report_hash.clone();
        thread::update(project, &t.id, |t| t.last_review_item_hash = hash)?;
    }
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

/// Step 6. A given set of unseen items is announced once; there is no timed
/// re-nudge. With `nudge = false` (the default) the user gets a herdr
/// notification instead of a prompt in the coordinator.
pub fn nudge(
    project: &Project,
    state: &mut State,
    settings: &Settings,
    herdr: &Herdr,
    coordinator_ready: Option<&str>,
) -> Result<()> {
    let seen = inbox::seen(project);
    let unseen: BTreeSet<String> = inbox::unhandled(project)
        .into_iter()
        .map(|i| i.id)
        .filter(|id| !seen.contains(id))
        .collect();
    if unseen.is_empty() {
        return Ok(());
    }
    let hash = hash_ids(&unseen);
    if hash == state.nudged {
        return Ok(());
    }
    if settings.nudge {
        let Some(pane) = coordinator_ready else {
            return Ok(()); // not idle or done: try again on a later tick
        };
        // `agent_blocked` and other errors are returned, logged by the caller,
        // and the nudge is retried on a later tick.
        let _writer = crate::talk::writer_lock(project)?;
        if crate::talk::writer_suspended(project) {
            return Ok(());
        }
        herdr.agent_prompt(pane, NUDGE_TEXT)?;
    } else {
        let body = format!(
            "{} new inbox item(s). The coordinator reads them at its next turn.",
            unseen.len()
        );
        let _ = herdr.notification_show(&format!("herdr-ade: {}", project.slug), &body);
    }
    state.nudged = hash;
    Ok(())
}

/// Step 2, every two minutes.
pub fn pull_requests(
    ctx: &Ctx,
    project: &Project,
    state: &mut State,
    memory: &mut Memory,
    now: jiff::Timestamp,
) -> Vec<anyhow::Error> {
    let mut errors = Vec::new();
    if thread::seconds_since(&state.last_pr_check, now) < PR_INTERVAL_SECS
        && !state.last_pr_check.is_empty()
    {
        return errors;
    }
    state.last_pr_check = now.to_string();

    for t in thread::list(project) {
        if t.status != Status::Open {
            continue;
        }
        // The `PR:` line of the home copy of the report.
        let report =
            std::fs::read_to_string(thread::home_report_path(project, &t.id)).unwrap_or_default();
        let url = match pr::pr_line(&report) {
            Ok(url) => url.unwrap_or_default(),
            Err(note) => {
                if state.pr_line_noted.get(&t.id) != Some(&t.report_hash) {
                    state
                        .pr_line_noted
                        .insert(t.id.clone(), t.report_hash.clone());
                    errors.extend(
                        inbox::write(
                            project,
                            "pr",
                            &t.id,
                            &format!("{}: {note}", thread_label(&t)),
                            "",
                        )
                        .err(),
                    );
                }
                String::new()
            }
        };
        if url != t.pr {
            let new_url = url.clone();
            errors.extend(thread::update(project, &t.id, |t| t.pr = new_url).err());
        }
        if url.is_empty() {
            continue;
        }

        let json = match pr::view(ctx.runner, &url) {
            Ok(json) => {
                if memory.gh.record(true, "", now, memory.outage_secs)
                    == Some(OutageEvent::Recovered)
                {
                    errors.extend(
                        inbox::write(
                            project,
                            "outage",
                            "gh",
                            "`gh` is working again; pull request follow-up has resumed",
                            "",
                        )
                        .err(),
                    );
                }
                json
            }
            Err(error) => {
                let text = pr::sanitize(&format!("{error:#}"));
                if memory.gh.record(false, &text, now, memory.outage_secs)
                    == Some(OutageEvent::Down)
                {
                    let summary = format!(
                        "`gh` has been failing for {} minutes; pull requests are not being followed. Last error: {text}",
                        memory.outage_secs / 60
                    );
                    errors.extend(inbox::write(project, "outage", "gh", &summary, "").err());
                }
                continue;
            }
        };
        match pr::reduce(&json, &t.branch, &t.origin) {
            Err(error) => errors.push(error.context(format!("{}: gh output", t.id))),
            Ok(pr::Checked::Ignored(reason)) => {
                if state.pr_ignored.get(&t.id) != Some(&url) {
                    state.pr_ignored.insert(t.id.clone(), url.clone());
                    errors.extend(
                        inbox::write(
                            project,
                            "pr",
                            &t.id,
                            &format!(
                                "{}: the pull request in its report is ignored: {reason}",
                                thread_label(&t)
                            ),
                            "",
                        )
                        .err(),
                    );
                }
            }
            Ok(pr::Checked::Summary(summary)) => {
                let old = state.prs.get(&t.id).cloned();
                if old.as_ref() == Some(&summary) {
                    continue;
                }
                let (pr_state, pr_review) =
                    (summary.state.clone(), summary.review_decision.clone());
                errors.extend(
                    thread::update(project, &t.id, |t| {
                        t.pr_state = pr_state;
                        t.pr_review = pr_review;
                    })
                    .err(),
                );
                let change = pr::describe_change(old.as_ref(), &summary);
                let merged = summary.state == "MERGED";
                state.prs.insert(t.id.clone(), summary);
                errors.extend(
                    inbox::write(
                        project,
                        "pr",
                        &t.id,
                        &format!("{}: pull request {change}", thread_label(&t)),
                        "",
                    )
                    .err(),
                );
                if merged {
                    errors.extend(resolve_after_copy(ctx, project, &t, "merged").err());
                }
            }
        }
    }
    errors
}

/// Auto-resolve and resolve-on-merge: the final copy first; if it fails the
/// thread is not resolved and the next tick tries again.
fn resolve_after_copy(ctx: &Ctx, project: &Project, t: &Thread, reason: &str) -> Result<bool> {
    let copied = threads::final_copy(ctx, project, t);
    if let CopyOutcome::Failed(error) = copied.outcome {
        anyhow::bail!(
            "{}: not resolved ({reason}) because the final copy failed: {error}",
            t.id
        );
    }
    let resolved = thread::update(project, &t.id, |t| {
        t.status = Status::Resolved;
        t.resolved_reason = reason.to_string();
        t.prompt_pending = false;
    })?;
    threads::close_pane(ctx, project, &resolved)?;
    Ok(true)
}

/// Step 4. Measured from the later of the last state change, the last report
/// change and the time this ticker process started, so a ticker that was down
/// for a week does not resolve everything at once.
pub fn auto_resolve(
    ctx: &Ctx,
    project: &Project,
    settings: &Settings,
    memory: &Memory,
    now: jiff::Timestamp,
) -> Vec<anyhow::Error> {
    let mut errors = Vec::new();
    let limit = i64::from(settings.auto_resolve_days) * 86_400;
    if limit == 0 {
        return errors;
    }
    for t in thread::list(project) {
        if t.status != Status::Open || t.last_group != Group::Idle.token() {
            continue;
        }
        // The later of the three reference times is the smallest elapsed time.
        // A thread with neither timestamp has no clock to measure from.
        let elapsed = |stamp: &str| {
            stamp
                .parse::<jiff::Timestamp>()
                .ok()
                .map(|then| now.as_second() - then.as_second())
        };
        let since_ticker_start = now.as_second() - memory.started.as_second();
        let Some(since_thread) = [
            elapsed(&t.last_state_change),
            elapsed(&t.last_report_change),
        ]
        .into_iter()
        .flatten()
        .min() else {
            continue;
        };
        if since_thread.min(since_ticker_start) < limit {
            continue;
        }
        match resolve_after_copy(ctx, project, &t, "auto") {
            Ok(_) => errors.extend(inbox::write(project, "thread-state", &t.id, &format!("{} was idle for {} days and was resolved automatically; `thread resolve --reopen` undoes it", thread_label(&t), settings.auto_resolve_days), "").err()),
            Err(error) => errors.push(error),
        }
    }
    errors
}

/// Step 3, plus `config-error` items for files that do not parse.
pub fn routines(
    ctx: &Ctx,
    project: &Project,
    state: &mut State,
    routine_commands: bool,
    project_md_error: Option<(String, String)>,
    now: &jiff::Zoned,
) -> Vec<anyhow::Error> {
    let mut errors = Vec::new();
    let (routines, broken) = routine::load_all(project);

    let mut problems: Vec<(String, String, String)> = broken
        .into_iter()
        .map(|b| (b.file, b.hash, b.error))
        .collect();
    if let Some((hash, error)) = project_md_error {
        problems.push(("PROJECT.md".into(), hash, error));
    }
    for (file, hash, error) in problems {
        // One item per distinct file hash, so an unfixed file does not repeat.
        if state.config_errors.insert(hash) {
            let stem = file.trim_start_matches("routines/").trim_end_matches(".md");
            errors.extend(
                inbox::write(
                    project,
                    "config-error",
                    stem,
                    &format!("{file} is not usable: {}", pr::sanitize(&error)),
                    "",
                )
                .err(),
            );
        }
    }

    let prefix = crate::coordinator::current_prefix(&ctx.root).unwrap_or_default();
    for r in routines.iter().filter(|r| r.enabled) {
        let entry = state.routines.entry(r.name.clone()).or_default();
        let Ok(last_run) = entry.last_run.parse::<jiff::Timestamp>() else {
            // First seen counts as the last run: nothing fires the moment a
            // routine file appears.
            entry.last_run = now.timestamp().to_string();
            continue;
        };
        if !routine::is_due(&r.schedule, last_run, now) {
            continue;
        }
        entry.last_run = now.timestamp().to_string();

        if r.command.is_empty() {
            errors.extend(
                inbox::write(
                    project,
                    "routine",
                    &r.name,
                    &format!("routine `{}` is due", r.name),
                    &r.prompt,
                )
                .err(),
            );
            continue;
        }
        if !routine_commands || !routine::is_approved(&ctx.config_dir, project, r) {
            let hash = r.command_hash();
            if entry.approval_item_for != hash {
                entry.approval_item_for = hash;
                let why = if routine_commands {
                    "its command is not approved (or was edited since approval)"
                } else {
                    "routine commands are not enabled for this project"
                };
                let summary = format!(
                    "routine `{}` did not run: {why}. The user enables them with `routine_commands = true` (see `{prefix} safety show {}`) and approves with `{prefix} routine approve {} {}` in a terminal",
                    r.name, project.slug, project.slug, r.name
                );
                errors
                    .extend(inbox::write(project, "routine-approval", &r.name, &summary, "").err());
            }
            continue;
        }
        match routine::run_command(ctx.runner, project, r) {
            Ok(ran) => {
                if ran.output_hash != entry.output_hash {
                    entry.output_hash = ran.output_hash;
                    let body = format!("{}\n\n{}", r.prompt, ran.block);
                    errors.extend(
                        inbox::write(
                            project,
                            "routine",
                            &r.name,
                            &format!(
                                "routine `{}` ran ({}) and its output changed",
                                r.name, ran.exit
                            ),
                            body.trim(),
                        )
                        .err(),
                    );
                }
            }
            Err(error) => errors.push(error.context(format!("routine {}", r.name))),
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Runner as _;

    fn at(text: &str) -> jiff::Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn courier_manifest_parses_records_and_refuses_junk() {
        let text = "boot\tboot-1\nfree\t1234\n\
                    event\tdemo\tt-0001-1-1\t/r/demo/events/t-0001-1-1.toml\tabc\t/r/demo/artifacts/def\tdef\n\
                    event\tdemo\tt-0002-1-1\t/r/demo/events/t-0002-1-1.toml\tabc\t-\t-\n";
        let manifest = parse_courier_manifest(text).unwrap();
        assert_eq!(manifest.boot_id, "boot-1");
        assert_eq!(manifest.free_bytes, 1234);
        assert_eq!(manifest.envelopes.len(), 2);
        assert_eq!(manifest.envelopes[0].artifact_hash, "def");
        assert!(manifest.envelopes[1].artifact_path.is_empty());
        assert!(parse_courier_manifest("nonsense\n").is_err());
    }

    #[test]
    fn courier_helper_survives_a_hostile_box_root() {
        let script = courier_helper("/home/it's a $(box)");
        let command = format!("sh -c {}", crate::remote::quote(&script));
        let out = crate::runner::RealRunner
            .run(&crate::runner::Cmd::new("sh", Duration::from_secs(5)).args(["-c", &command]))
            .unwrap();
        assert!(out.success(), "{}", out.error_text());
        assert!(out.stdout.contains("boot\t"), "{}", out.stdout);
        assert!(out.stdout.contains("free\t"), "{}", out.stdout);
    }

    #[test]
    fn a_box_lane_signal_waits_until_the_coordinator_can_receive_it() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let lane = thread::allocate(&project, |t| {
            t.machine = "oci".into();
            t.machine_id = "abc".into();
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
        remote_attention(
            &ctx,
            &project,
            RemoteView {
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
        remote_attention(
            &ctx,
            &project,
            RemoteView {
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
        remote_attention(
            &ctx,
            &project,
            RemoteView {
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
    }

    #[test]
    fn short_outages_write_nothing_and_long_ones_write_one_item_each_way() {
        let mut outage = Outage::default();
        assert_eq!(
            outage.record(false, "e", at("2026-09-17T10:00:00Z"), 600),
            None
        );
        assert_eq!(
            outage.record(false, "e", at("2026-09-17T10:05:00Z"), 600),
            None
        );
        // A blip that ends before the threshold reports nothing at all.
        assert_eq!(
            outage.record(true, "", at("2026-09-17T10:06:00Z"), 600),
            None
        );

        assert_eq!(
            outage.record(false, "e", at("2026-09-17T11:00:00Z"), 600),
            None
        );
        assert_eq!(
            outage.record(false, "e", at("2026-09-17T11:10:00Z"), 600),
            Some(OutageEvent::Down)
        );
        assert_eq!(
            outage.record(false, "e", at("2026-09-17T11:30:00Z"), 600),
            None
        );
        assert_eq!(
            outage.record(true, "", at("2026-09-17T11:31:00Z"), 600),
            Some(OutageEvent::Recovered)
        );
        assert_eq!(
            outage.record(true, "", at("2026-09-17T11:32:00Z"), 600),
            None
        );
    }
}
