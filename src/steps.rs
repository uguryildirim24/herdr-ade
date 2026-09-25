//! The ticker's per-project steps: delivery, messages and pull requests.
//! Thread facts update their owning records; only messages
//! without a thread or round home enter the inbox.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::herdr::{Agent, Herdr, Pane};
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::thread::{self, CopyOutcome, Status, Thread};
use crate::threads;
use crate::{events, inbox, pr};

pub(crate) const TICKER_PROMPT_PREFIX: &str =
    "[herdr-ade ticker: automated, not the user, approves nothing]";
const PR_INTERVAL_SECS: i64 = 120;
pub(crate) const DONE_RETENTION_DAYS: u64 = 30;
const DEFAULT_OUTAGE_SECS: i64 = 600;

/// `.state/ticker.json`: what the ticker compared against last time.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub(crate) struct State {
    pub(crate) last_pr_check: String,
    /// Hashes of files a `config-error` item was already written for.
    pub(crate) config_errors: BTreeSet<String>,
    /// Hash of the unseen inbox item ids last announced.
    pub(crate) announced: String,
    pub(crate) session_item_written: bool,
    /// Last automatic coordinator relaunch attempt, including failed starts.
    pub(crate) coordinator_relaunch_last: String,
    /// Coordinator pane whose live lanes were last reconciled by pickup.
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

/// Delivers every sealed event whose transport submission is not yet in its
/// journal. The event, not the typed line or report hash, is authoritative.
/// A held notice holds later notices, preserving their seal order. An event whose journal already holds `submitted` is not typed
/// again; one read before its line went out (`acknowledged` or `handled` with
/// no `submitted`) is still typed once, so the wake-up always happens. An
/// event for a superseded lane attempt is left as it is, sealed and
/// undelivered.
pub(crate) fn deliver_events(ctx: &Ctx, project: &Project) -> Result<()> {
    let mut first: Option<anyhow::Error> = None;
    let sealed = crate::events::list(project);
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
    for event in sealed {
        if event.payload.failed.is_some() {
            continue;
        }
        // A resolved lane is finished: its delivery journal is never replayed.
        if thread::load(project, &event.thread).is_ok_and(|lane| lane.status == Status::Resolved) {
            continue;
        }
        let states = match crate::events::states(project, &event.id) {
            Ok(states) => states,
            Err(error) => {
                first.get_or_insert(error);
                continue;
            }
        };
        // Context reads sealed work directly, including seals submitted to a
        // previous coordinator. Never replay a line already typed once.
        if states.contains(&crate::contracts::DeliveryState::Submitted) {
            continue;
        }
        // No `submitted` yet: fresh, or read before its wake-up line was
        // typed. Both still owe exactly one typed line.
        if thread::load(project, &event.thread)
            .is_ok_and(|lane| lane.attempt.max(1) != event.attempt)
        {
            continue;
        }
        if let Err(error) = deliver_notice(ctx, project, &event) {
            first.get_or_insert(error.context(format!("event {}", event.id)));
            break;
        }
        // A held notice must not be overtaken by a later one.
        if !crate::events::states(project, &event.id)?
            .contains(&crate::contracts::DeliveryState::Submitted)
        {
            break;
        }
    }
    first.map_or(Ok(()), Err)
}

pub(crate) fn deliver_event(
    ctx: &Ctx,
    project: &Project,
    event: &crate::contracts::Event,
) -> Result<()> {
    adopt_report(project, event)?;
    deliver_notice(ctx, project, event)
}

fn deliver_notice(ctx: &Ctx, project: &Project, event: &crate::contracts::Event) -> Result<()> {
    let coordinator = project
        .coordinator()
        .ok_or_else(|| anyhow::anyhow!("recipient_unavailable: project has no coordinator"))?;
    if coordinator.pane_id != event.recipient.pane
        || coordinator.attempt() != event.recipient.coordinator_attempt
    {
        // The new binding sees this seal in context; it must not receive a
        // second inbox projection or a line addressed to the old binding.
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
    // A box lane's DONE is not typed until the Mac itself holds the pinned code
    // commit: fetch the lane branch from the configured GitHub URL and check
    // the event's sha is reachable there (SPEC-remote §4.3, gate R5).
    if lane.is_remote()
        && let Some(done) = &event.payload.done
    {
        verify_published_sha(ctx, project, &lane, &done.sha)?;
    }

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

    // A seal can overtake a queued correction. Store a stable event-bound
    // notice even when the coordinator is offline, and say it in the wake-up.
    let queued: Vec<_> = if event.payload.done.is_some() {
        lane.follow_ups
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                f.attempt == event.attempt
                    && (f.state == crate::thread::FollowUpState::Queued || f.after_seal == event.id)
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
                "{} sealed before follow-up {} landed; keep the lane open for its next seal",
                event.thread,
                queued.join("; ")
            ),
        )?;
    }
    // Serialize prompts sent to the coordinator pane.
    let _writer = crate::talk::writer_lock(project)?;
    let agent = herdr.agent_list()?.into_iter().find(|agent| {
        agent.pane_id == event.recipient.pane
            && agent.name == coordinator.agent_name
            && agent.ready()
    });
    let Some(_agent) = agent else {
        return Ok(());
    };
    if !crate::talk::coordinator_prompt_clear(project, &herdr, &event.recipient.pane)? {
        return Ok(());
    }
    let mut line = crate::events::typed_line(event)?;
    if !queued.is_empty() {
        line.push_str(&format!(
            " Follow-up overtook this seal: {}. Keep this lane open until it seals again.",
            queued.join("; ")
        ));
    }
    crate::talk::mark_automated_prompt(project, &event.recipient.pane, &line)?;
    herdr.agent_prompt(&event.recipient.pane, &line)?;
    crate::events::append_delivery(
        project,
        &event.id,
        crate::contracts::DeliveryState::Submitted,
    )
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
fn verify_published_sha(ctx: &Ctx, project: &Project, lane: &Thread, sha: &str) -> Result<()> {
    if sha.is_empty() {
        bail!("published_sha_missing: the done event has no sha");
    }
    if lane.branch.is_empty() {
        bail!("published_branch_missing: {} has no lane branch", lane.id);
    }
    let url = publish_url_for(ctx, project, lane.machine_route(), &lane.repo)?
        .with_context(|| format!("box_repo_unmapped: {} has no publish URL", lane.repo))?;
    let remote = crate::remote::remote_for_url(ctx.runner, &lane.repo, &url)?;
    let git = |args: &[&str]| {
        ctx.runner.run(
            &crate::runner::Cmd::new("git", Duration::from_secs(60))
                .args(["-C", lane.repo.as_str()])
                .args(args.iter().copied()),
        )
    };
    let out = git(&[
        "fetch",
        "--quiet",
        remote.as_str(),
        &format!("refs/heads/{}", lane.branch),
    ])?;
    if !out.success() {
        bail!("published_fetch_failed: {url}: {}", out.error_text());
    }
    if !crate::git::is_ancestor(ctx.runner, &lane.repo, sha, "FETCH_HEAD")? {
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
        if !self.reported && now.as_second() - since.as_second() >= threshold_secs {
            self.reported = true;
            return Some(OutageEvent::Down);
        }
        None
    }
}

const REMOTE_EVERY_TICKS: u64 = 4;
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
    pub(crate) gh: Outage,
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
            gh: Outage::default(),
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

    /// Remote machines are polled every fourth tick (about a minute), and not
    /// at all for eight ticks after a failure.
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
const COURIER_TIMEOUT: Duration = Duration::from_secs(30);

/// One envelope the box helper reported (SPEC-remote §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionReceipt {
    pub(crate) slug: String,
    pub(crate) event: String,
    pub(crate) event_hash: String,
    /// Empty for a `waiting` receipt.
    pub(crate) artifact_hash: String,
}

/// One bootstrap receipt the box wrote when its lane ran `skill lane` (D14).
/// The courier carries it to the Mac, which marks the thread's receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BootstrapReceipt {
    pub(crate) slug: String,
    pub(crate) thread: String,
    pub(crate) brief_hash: String,
    pub(crate) pane: String,
}

/// What one box helper call returned, after the taken cursor it was asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CourierManifest {
    pub(crate) boot_id: String,
    pub(crate) free_bytes: u64,
    /// Box-local `herdr agent list` JSON; `None` when the box server did not
    /// answer this pass.
    pub(crate) agents: Option<String>,
    /// Box-local `herdr pane list` JSON; `None` when it did not answer.
    pub(crate) panes: Option<String>,
    pub(crate) envelopes: Vec<BoxEnvelope>,
    pub(crate) receipts: Vec<CompletionReceipt>,
    pub(crate) bootstraps: Vec<BootstrapReceipt>,
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
}

/// The box-local helper: it recovers an interrupted box D5 operation, reads
/// the box server's own live lists, and prints one tab-separated record per
/// fact after the taken cursor it reads on stdin (SPEC-remote §4.3). It makes
/// no `herdr --machine` bridge call; the courier's one SSH trip carries both
/// the helper and the batched `scp`.
const COURIER_HELPER: &str = r#"set -u
root=__ROOT__
PATH=__PATH__; export PATH
herdr_bin=$(command -v herdr 2>/dev/null || true)
ade_bin=__ADE_BIN__
cursor=$(mktemp)
trap 'rm -f "$cursor"' EXIT
cat > "$cursor"
printf 'boot\t%s\n' "$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true)"
avail=$(df -B1 --output=avail / 2>/dev/null | tail -n1 | tr -d ' ')
printf 'free\t%s\n' "${avail:-0}"
"$ade_bin" --root "$root" recover >/dev/null 2>&1 || true
if [ -x "$herdr_bin" ]; then
  a=$("$herdr_bin" --session __SESSION__ agent list 2>/dev/null | tr -d '\n')
  p=$("$herdr_bin" --session __SESSION__ pane list 2>/dev/null | tr -d '\n')
  if [ -n "$a" ]; then printf 'agents\t%s\n' "$a"; else printf 'agents\t-\n'; fi
  if [ -n "$p" ]; then printf 'panes\t%s\n' "$p"; else printf 'panes\t-\n'; fi
else
  printf 'agents\t-\n'
  printf 'panes\t-\n'
fi
[ -d "$root" ] || exit 0
for dir in "$root"/*/.state/events; do
  [ -d "$dir" ] || continue
  slug=${dir%/.state/events}; slug=${slug##*/}
  for f in "$dir"/*.toml; do
    [ -f "$f" ] || continue
    id=${f##*/}; id=${id%.toml}
    case "$id" in .*) continue;; esac
    key=$(printf '%s\t%s' "$slug" "$id")
    if grep -Fqx "$key" "$cursor" 2>/dev/null; then continue; fi
    h=$(sha256sum "$f" | cut -d' ' -f1)
    a=$(sed -n 's/^artifact = "\([^"]*\)"/\1/p' "$f" | head -n1)
    if [ -n "$a" ]; then
      printf 'event\t%s\t%s\t%s\t%s\t%s\t%s\n' "$slug" "$id" "$f" "$h" "$root/$slug/.state/artifacts/$a" "$a"
    else
      printf 'event\t%s\t%s\t%s\t%s\t-\t-\n' "$slug" "$id" "$f" "$h"
    fi
    rec="$root/$slug/.state/receipts/$id.toml"
    if [ -f "$rec" ]; then
      eh=$(sed -n 's/^event_hash = "\([^"]*\)"/\1/p' "$rec" | head -n1)
      ah=$(sed -n 's/^artifact_hash = "\([^"]*\)"/\1/p' "$rec" | head -n1)
      printf 'receipt\t%s\t%s\t%s\t%s\n' "$slug" "$id" "$eh" "$ah"
    fi
  done
done
for f in "$root"/*/.state/bootstrap/*.json; do
  [ -f "$f" ] || continue
  slug=${f%%/.state/bootstrap/*}; slug=${slug##*/}
  thread=${f##*/}; thread=${thread%.json}
  brief=$(sed -n 's/.*"brief_hash"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$f" | head -n1)
  pane=$(sed -n 's/.*"pane"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$f" | head -n1)
  printf 'bootstrap\t%s\t%s\t%s\t%s\n' "$slug" "$thread" "$brief" "$pane"
done
"#;

fn courier_helper(machine: &crate::remote::MachineDeclaration, session: &str) -> String {
    COURIER_HELPER
        .replace("__ROOT__", &crate::remote::quote(&machine.root))
        .replace("__PATH__", &crate::remote::quote(&machine.path))
        .replace("__ADE_BIN__", &crate::remote::quote(&machine.ade_bin))
        .replace("__SESSION__", &crate::remote::quote(session))
}

/// Parses the helper's tab-separated output. An unknown record is refused so a
/// helper version mismatch is loud, not silently empty.
fn parse_courier_manifest(text: &str) -> Result<CourierManifest> {
    let mut manifest = CourierManifest::default();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["boot", id] => manifest.boot_id = (*id).to_string(),
            ["free", bytes] => manifest.free_bytes = bytes.trim().parse().unwrap_or(0),
            ["agents", json] => {
                if *json != "-" && !json.is_empty() {
                    manifest.agents = Some((*json).to_string());
                }
            }
            ["panes", json] => {
                if *json != "-" && !json.is_empty() {
                    manifest.panes = Some((*json).to_string());
                }
            }
            ["receipt", slug, event, event_hash, artifact_hash] => {
                manifest.receipts.push(CompletionReceipt {
                    slug: (*slug).to_string(),
                    event: (*event).to_string(),
                    event_hash: (*event_hash).to_string(),
                    artifact_hash: (*artifact_hash).to_string(),
                });
            }
            ["bootstrap", slug, thread, brief_hash, pane] => {
                manifest.bootstraps.push(BootstrapReceipt {
                    slug: (*slug).to_string(),
                    thread: (*thread).to_string(),
                    brief_hash: (*brief_hash).to_string(),
                    pane: (*pane).to_string(),
                });
            }
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

/// Parses one box-local `herdr <list> --json` reply into its list. A reply
/// carrying `error` is a refusal; a missing list is an empty one.
fn parse_box_list<T: serde::de::DeserializeOwned>(json: &str, field: &str) -> Result<Vec<T>> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("the box herdr list is not JSON")?;
    if let Some(error) = value.get("error") {
        bail!(
            "box herdr {field} failed: {}",
            error.get("message").and_then(|m| m.as_str()).unwrap_or("")
        );
    }
    let list = value
        .get("result")
        .and_then(|result| result.get(field))
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    serde_json::from_value(list)
        .map_err(|error| anyhow::anyhow!("the box herdr {field} reply changed: {error}"))
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
/// Mac's canonical ledger. The box keeps its copies; only the taken cursor
/// advances, and only after the import is durable. Returns the box's live
/// facts, so the caller can key lane state and push GONE after a reboot.
pub(crate) fn courier(ctx: &Ctx, projects: &[&Project], machine: &str) -> Result<CourierOutcome> {
    let _scope = crate::ledger::Scope::new(projects);
    let result = courier_inner(ctx, projects, machine);
    if let Err(error) = &result {
        for project in projects {
            crate::ledger::observe(project, "courier-failed", machine, &format!("{error:#}"));
        }
    } else {
        for project in projects {
            crate::ledger::recovered(project, "courier-failed", machine);
        }
    }
    result
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
    let mut cursor = String::new();
    for project in projects {
        let state = events::remote_state(project, &profile.id);
        for id in state.taken.keys() {
            cursor.push_str(&project.slug);
            cursor.push('\t');
            cursor.push_str(id);
            cursor.push('\n');
        }
        states.insert(project.slug.clone(), state);
    }

    let machine_paths = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)
        .map_err(|error| MachineLookupError(format!("machine declaration failed: {error:#}")))?;
    let script = courier_helper(&machine_paths, &profile.session);
    let out = crate::remote::ssh_courier(
        ctx.runner,
        &target,
        &control,
        &script,
        &cursor,
        COURIER_TIMEOUT,
    )?;
    if !out.success() {
        bail!("courier helper on {target}: {}", out.error_text());
    }
    let manifest = parse_courier_manifest(&out.stdout)
        .with_context(|| format!("courier helper on {target}"))?;

    let agents = match &manifest.agents {
        Some(json) => Some(parse_box_list::<Agent>(json, "agents")?),
        None => None,
    };
    let panes = match &manifest.panes {
        Some(json) => Some(parse_box_list::<Pane>(json, "panes")?),
        None => None,
    };
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
        // new. The boot id is left for `remote_attention` to advance: it must
        // see the change to type GONE for the pre-reboot lanes.
        state.last_pass = project::now();
        events::save_remote_state(project, &profile.id, &state)?;
    }

    apply_bootstraps(projects, &manifest.bootstraps);

    Ok(CourierOutcome {
        machine_id: profile.id,
        boot_id: manifest.boot_id,
        agents,
        panes,
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

/// One successful pass's live facts for a machine's box lanes.
pub(crate) struct RemoteView<'a> {
    pub(crate) machine_id: &'a str,
    pub(crate) threads: &'a [Thread],
    pub(crate) agents: &'a [Agent],
    pub(crate) panes: &'a [Pane],
    pub(crate) boot_id: &'a str,
    pub(crate) now: jiff::Timestamp,
}

/// The D8 amendment for box lanes: the fork has no push across machines, so
/// the plugin's one serialized writer types `BLOCKED` and `GONE` exactly once
/// per transition. A boot-id change types GONE for every open box lane; a pane
/// or agent absent on two consecutive successful passes does the same.
pub(crate) fn remote_attention(
    ctx: &Ctx,
    project: &Project,
    view: RemoteView<'_>,
) -> Vec<anyhow::Error> {
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
        state.pending_gone = threads.iter().map(|lane| lane.id.clone()).collect();
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
        // Placement creates the pane before the ticker has launched an
        // agent. A missing agent (or even a reboot) at that boundary is not
        // evidence of a gone process and must not spend recovery retries.
        // Also discard a prior attempt's gone marker so this attempt can be
        // observed independently once its launch begins.
        if lane.parked || crate::threads::parkable(project, lane) {
            state.blocked.remove(&lane.id);
            state.missing.remove(&lane.id);
            state.gone.remove(&lane.id);
            state.pending_gone.remove(&lane.id);
            continue;
        }
        if lane.launch_attempts == 0 && lane.startup_wait_started.is_empty() {
            state.missing.remove(&lane.id);
            state.gone.remove(&lane.id);
            state.pending_gone.remove(&lane.id);
            continue;
        }
        let live = thread::live_state(lane, agents, panes, now);
        let blocked = live.agent_state.as_deref() == Some("blocked");
        if blocked {
            if !state.blocked.contains(&lane.id) {
                signals.push(Signal::Blocked(lane.id.clone()));
            }
        } else {
            state.blocked.remove(&lane.id);
        }
        let process_present = live.pane_exists
            && (live.agent_state.is_some() || lane.prompt_pending || lane.last_state.is_empty());
        if process_present {
            state.missing.insert(lane.id.clone(), 0);
        } else {
            let count = state.missing.entry(lane.id.clone()).or_insert(0);
            *count += 1;
        }
        let missing = *state.missing.get(&lane.id).unwrap_or(&0);
        if (state.pending_gone.contains(&lane.id) || missing >= 2) && !state.gone.contains(&lane.id)
        {
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
                    state.pending_gone.remove(&lane);
                    state.gone.insert(lane.clone());
                    if let Some(record) = threads.iter().find(|record| record.id == lane) {
                        let recover = !record.launch.recipe_id.is_empty();
                        if let Err(error) = crate::threads::fail_start(
                            ctx,
                            project,
                            &lane,
                            "the pane or agent is gone without a report",
                            crate::contracts::FailureClass::ProcessGone,
                            recover,
                        ) {
                            errors.push(error.context(format!("{lane}: process recovery")));
                        }
                    }
                }
            },
            // A suspended or busy writer has not consumed the transition. Do
            // not mark it: the next successful pass must try again.
            Ok(false) => break,
            Err(error) => {
                errors.push(error.context(format!("remote line `{line}`")));
                break;
            }
        }
    }
    if let Err(error) = events::save_remote_state(project, machine_id, &state) {
        errors.push(error.context("remote state"));
    }
    errors
}

/// Types one line into the coordinator through the same serialized writer D5
/// uses (SPEC-remote §4.3). Returns whether it was typed.
pub(crate) fn type_remote_line(ctx: &Ctx, project: &Project, text: &str) -> Result<bool> {
    let _scope = crate::ledger::Scope::new(&[project]);
    let Some(record) = project.coordinator() else {
        return Ok(false);
    };
    if record.pane_id.is_empty() {
        return Ok(false);
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, ctx.runner);
    let _writer = crate::talk::writer_lock(project)?;
    let ready = herdr.agent_list()?.into_iter().any(|agent| {
        agent.pane_id == record.pane_id && agent.name == record.agent_name && agent.ready()
    });
    if !ready {
        return Ok(false);
    }
    if !crate::talk::coordinator_prompt_clear(project, &herdr, &record.pane_id)? {
        return Ok(false);
    }
    crate::talk::mark_automated_prompt(project, &record.pane_id, text)?;
    herdr.agent_prompt(&record.pane_id, text)?;
    Ok(true)
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

/// Step 2, every two minutes.
pub(crate) fn pull_requests(
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
        // During work the `PR:` line is in the lane's draft. After cleanup,
        // read the one final artifact (or an unmatched historical home copy).
        let draft = std::path::Path::new(&t.thread_dir).join("report.md");
        let report = std::fs::read_to_string(&draft).unwrap_or_else(|_| {
            thread::final_report_path(project, &t)
                .and_then(|path| std::fs::read_to_string(path).ok())
                .unwrap_or_default()
        });
        let (url, note) = match pr::pr_line(&report) {
            Ok(url) => (url.unwrap_or_default(), String::new()),
            Err(note) => (String::new(), note),
        };
        if url != t.pr || (url.is_empty() && note != t.pr_note) {
            errors.extend(
                thread::update(project, &t.id, |t| {
                    t.pr = url.clone();
                    t.pr_note = note;
                    t.pr_summary = None;
                    t.pr_state.clear();
                    t.pr_review.clear();
                })
                .err(),
            );
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
                errors.extend(
                    thread::update(project, &t.id, |t| {
                        t.pr_note = format!("pull request ignored: {reason}");
                        t.pr_summary = None;
                        t.pr_state.clear();
                        t.pr_review.clear();
                    })
                    .err(),
                );
            }
            Ok(pr::Checked::Summary(summary)) => {
                let old = t.pr_summary.clone();
                if t.pr == url && t.pr_note.is_empty() && old.as_ref() == Some(&summary) {
                    continue;
                }
                let (pr_state, pr_review) =
                    (summary.state.clone(), summary.review_decision.clone());
                errors.extend(
                    thread::update(project, &t.id, |t| {
                        t.pr_note.clear();
                        t.pr_state = pr_state;
                        t.pr_review = pr_review;
                        t.pr_summary = Some(summary.clone());
                    })
                    .err(),
                );
                let merged = summary.state == "MERGED";
                if merged {
                    errors.extend(resolve_after_copy(ctx, project, &t, "merged").err());
                }
            }
        }
    }
    errors
}

/// Resolve-on-pull-request-merge: the final copy first; if it fails the
/// thread is not resolved and the next pull-request check tries again.
fn resolve_after_copy(ctx: &Ctx, project: &Project, t: &Thread, reason: &str) -> Result<bool> {
    crate::round::require_resolvable(project, &t.id)?;
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
    threads::remove_scratch_session(ctx, &resolved)?;
    Ok(true)
}

/// Step 3, plus `config-error` items for files that do not parse.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Runner as _;
    use crate::scenarios::{World, agent_json};

    fn at(text: &str) -> jiff::Timestamp {
        text.parse().unwrap()
    }

    fn test_machine(root: &str) -> crate::remote::MachineDeclaration {
        crate::remote::MachineDeclaration {
            root: root.into(),
            path: "/bin:/usr/bin".into(),
            ade_bin: "/missing/herdr-ade".into(),
            ..Default::default()
        }
    }

    /// A project with a ready coordinator in its bound pane, and the fake
    /// runner answering `agent prompt`.
    fn delivery_world() -> (World, Project) {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        world
            .runner
            .on("agent prompt", crate::runner::fake::ok(r#"{"result":{}}"#));
        let cwd = project.canonical_dir().to_string_lossy().into_owned();
        *world.agents.borrow_mut() = format!(
            "[{}]",
            agent_json("w1", "w1:t1", "w1:p1", &cwd, "hp-demo-coordinator", "idle")
        );
        (world, project)
    }

    /// A lane with one sealed `done` event bound to the project's coordinator.
    fn sealed_done(project: &Project, id: &str) -> crate::contracts::Event {
        let coordinator = project.coordinator().unwrap();
        let event = crate::contracts::Event {
            id: format!("{id}-1-1"),
            op: format!("{id}-1-1"),
            thread: id.into(),
            attempt: 1,
            round: None,
            recipient: crate::contracts::Recipient {
                pane: coordinator.pane_id.clone(),
                coordinator_attempt: coordinator.attempt(),
            },
            created: "2026-09-19T00:00:00Z".into(),
            payload: crate::contracts::EventPayload {
                done: Some(crate::contracts::DonePayload {
                    sha: "abc".into(),
                    report_path: ".reports/lane.md".into(),
                    artifact: "def".into(),
                    attestation: None,
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
        assert!(events::states(&project, &a.id).unwrap().is_empty());
        assert!(events::states(&project, &b.id).unwrap().is_empty());
        *screen.borrow_mut() = "❯ \n".into();
        deliver_events(&ctx, &project).unwrap();
        let lines = typed_lines(&world);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains(&first.id));
        assert!(lines[1].contains(&second.id));
        announce_inbox(&project, &mut state, &herdr).unwrap();
        assert_eq!(typed_lines(&world).len(), 2);
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
        *screen.borrow_mut() = "────────────────────\n❯ \n────────────────────\n  /Users/rolfie/.herdr-ade/adeherdr > ctx\n  ⏵⏵ bypass permissions on · 1 shell · ← for agents\n  ● main\n  ◯ general-purpose  Verifying excluded files · 20m\n".into();
        deliver_events(&world.ctx(), &project).unwrap();
        assert_eq!(typed_lines(&world).len(), 2);
        assert!(
            events::states(&project, &event.id)
                .unwrap()
                .contains(&crate::contracts::DeliveryState::Submitted)
        );
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
            vec![crate::contracts::DeliveryState::Submitted]
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
    fn courier_manifest_parses_records_and_refuses_junk() {
        let text = "boot\tboot-1\nfree\t1234\nagents\t{\"result\":{\"agents\":[]}}\npanes\t-\n\
                    receipt\tdemo\tt-0001-1-1\tabc\tdef\n\
                    bootstrap\tdemo\tt-0001\tabcd\tw1:p2\n\
                    event\tdemo\tt-0001-1-1\t/r/demo/.state/events/t-0001-1-1.toml\tabc\t/r/demo/.state/artifacts/def\tdef\n\
                    event\tdemo\tt-0002-1-1\t/r/demo/.state/events/t-0002-1-1.toml\tabc\t-\t-\n";
        let manifest = parse_courier_manifest(text).unwrap();
        assert_eq!(manifest.boot_id, "boot-1");
        assert_eq!(manifest.free_bytes, 1234);
        assert!(manifest.agents.is_some());
        assert!(manifest.panes.is_none());
        assert_eq!(manifest.receipts.len(), 1);
        assert_eq!(manifest.receipts[0].artifact_hash, "def");
        assert_eq!(manifest.bootstraps.len(), 1);
        assert_eq!(manifest.bootstraps[0].pane, "w1:p2");
        assert_eq!(manifest.envelopes.len(), 2);
        assert_eq!(manifest.envelopes[0].artifact_hash, "def");
        assert!(manifest.envelopes[1].artifact_path.is_empty());
        assert!(parse_courier_manifest("nonsense\n").is_err());
    }

    #[test]
    fn courier_helper_survives_a_hostile_box_root() {
        let script = courier_helper(&test_machine("/home/it's a $(box)"), "default");
        let command = format!("sh -c {}", crate::remote::quote(&script));
        // A throwaway HOME so the script's box recovery never touches the real
        // ADE root; the box binaries are absent there.
        let home = tempfile::tempdir().unwrap();
        let out = crate::runner::RealRunner
            .run(
                &crate::runner::Cmd::new("sh", Duration::from_secs(120))
                    .args(["-c", &command])
                    .env("HOME", home.path().display().to_string()),
            )
            .unwrap();
        assert!(out.success(), "{}", out.error_text());
        assert!(out.stdout.contains("boot\t"), "{}", out.stdout);
        assert!(out.stdout.contains("free\t"), "{}", out.stdout);
        assert!(out.stdout.contains("agents\t-\n"), "{}", out.stdout);
        assert!(out.stdout.contains("panes\t-\n"), "{}", out.stdout);
    }

    #[test]
    fn courier_helper_answers_only_after_the_taken_cursor() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("ade");
        let dir = root.join("demo/.state/events");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t-0001-1-1.toml"), "id = \"t-0001-1-1\"\n").unwrap();
        let script = courier_helper(&test_machine(&root.to_string_lossy()), "default");
        let command = format!("sh -c {}", crate::remote::quote(&script));
        let run = |stdin: &str| {
            crate::runner::RealRunner
                .run(
                    &crate::runner::Cmd::new("sh", Duration::from_secs(120))
                        .args(["-c", &command])
                        .env("HOME", home.path().display().to_string())
                        .stdin(stdin.to_string()),
                )
                .unwrap()
        };
        let skipped = run("demo\tt-0001-1-1\n");
        assert!(skipped.success(), "{}", skipped.error_text());
        assert!(
            !skipped.stdout.contains("event\tdemo"),
            "{}",
            skipped.stdout
        );
        let fresh = run("");
        assert!(
            fresh.stdout.contains("event\tdemo\tt-0001-1-1"),
            "{}",
            fresh.stdout
        );
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
        for _ in 0..3 {
            let errors = remote_attention(
                &ctx,
                &project,
                RemoteView {
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
        assert!(!state.missing.contains_key(&lane.id));
        let unchanged = thread::load(&project, &lane.id).unwrap();
        assert_eq!(unchanged.attempt, 2);
        assert_eq!(unchanged.launch.same_recipe_retries, 1);
        assert_eq!(unchanged.status, thread::Status::Starting);

        // After the ticker starts the agent, absence is evidence again.
        let mut launched = lane;
        launched.launch_attempts = 1;
        let _ = remote_attention(
            &ctx,
            &project,
            RemoteView {
                machine_id: "abc",
                threads: &[launched],
                agents: &[],
                panes: &[],
                boot_id: "boot-2",
                now,
            },
        );
        assert_eq!(
            events::remote_state(&project, "abc")
                .missing
                .values()
                .next(),
            Some(&1)
        );
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
        assert!(state.pending_gone.contains(&lane.id));
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
        assert!(
            events::remote_state(&project, "abc")
                .pending_gone
                .contains(&lane.id)
        );
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

    fn box_event_bytes(id: &str, artifact: &str) -> Vec<u8> {
        let event = crate::contracts::Event {
            id: id.into(),
            op: id.into(),
            thread: "t-0001".into(),
            attempt: 1,
            round: None,
            recipient: crate::contracts::Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 1,
            },
            created: "2026-09-19T00:00:00Z".into(),
            payload: crate::contracts::EventPayload {
                done: Some(crate::contracts::DonePayload {
                    sha: "abc".into(),
                    report_path: ".reports/t-0001.md".into(),
                    artifact: artifact.into(),
                    attestation: None,
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
        let manifest = format!(
            "boot\tboot-1\nfree\t100\nagents\t{{\"result\":{{\"agents\":[]}}}}\npanes\t{{\"result\":{{\"panes\":[]}}}}\n\
             event\talpha\tt-0001-1-1\t/box/alpha/.state/events/t-0001-1-1.toml\t{event_hash}\t/box/alpha/.state/artifacts/{artifact_hash}\t{artifact_hash}\n\
             receipt\talpha\tt-0001-1-1\t{event_hash}\t{artifact_hash}\n\
             bootstrap\talpha\tt-0001\t\tw2:p1\n\
             event\tbeta\tt-0001-1-1\t/box/beta/.state/events/t-0001-1-1.toml\t{event_hash}\t/box/beta/.state/artifacts/{artifact_hash}\t{artifact_hash}\n\
             receipt\tbeta\tt-0001-1-1\t{event_hash}\t{artifact_hash}\n"
        );
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
        let manifest = format!(
            "boot\tboot-1\nfree\t100\nagents\t-\npanes\t-\n\
             event\talpha\tt-0001-1-1\t/box/alpha/.state/events/t-0001-1-1.toml\t{event_hash}\t/box/alpha/.state/artifacts/{artifact_hash}\t{artifact_hash}\n\
             receipt\talpha\tt-0001-1-1\tdeadbeef\t{artifact_hash}\n"
        );
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
        let failures = crate::ledger::list(&alpha).unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, "courier-failed");
        assert_eq!(failures[0].subject, "box");
        assert!(failures[0].detail.contains("receipt_mismatch"));
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
        let error = verify_published_sha(&ctx, &project, &lane, "abc")
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
        let _scope = crate::ledger::Scope::new(&[&project]);
        let runner = crate::ledger::RecordingRunner(&sha_absent);
        let ctx = courier_ctx(root.path(), &env, &runner);
        let error = verify_published_sha(&ctx, &project, &lane, "abc")
            .unwrap_err()
            .to_string();
        assert!(error.contains("published_sha_missing"), "{error}");
        assert!(crate::ledger::list(&project).unwrap().is_empty());
    }
}
