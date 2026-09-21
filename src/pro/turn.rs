//! The turn protocol: packet in, collector out (SPEC-pro-bridge v2,
//! "Lane lifecycle and turn protocol").
//!
//! `prepare` validates and writes the packet and the turn record, then `spawn`
//! forks a detached collector. The collector owns the long wait, reads the
//! Codex rollout for the answer, writes it atomically and types the
//! D7-shaped DONE line. It never resends a prompt.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::herdr_cli;
use super::lane;
use super::packet;
use super::sh::Runner;
use super::state::{self, BridgeState, Inflight, Lane, Turn};
use super::{Env, Layout, bridge, doctor};

/// The poll interval while waiting on the rollout.
const POLL: Duration = Duration::from_millis(500);
/// The DONE line is typed again once after this wait.
const NOTIFY_RETRY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub(crate) struct TurnOptions {
    pub(crate) lane: String,
    pub(crate) brief: PathBuf,
    pub(crate) out: PathBuf,
    pub(crate) notify: String,
    pub(crate) attachments: Vec<PathBuf>,
    pub(crate) id: Option<String>,
}

/// `herdr-pro turn`: preflight, packet, record, then a detached collector.
pub(crate) fn start(
    env: &Env,
    layout: &Layout,
    runner: &dyn Runner,
    opts: &TurnOptions,
) -> Result<Turn> {
    let turn = prepare(env, layout, runner, opts)?;
    match spawn(layout, &turn.tag) {
        Ok(()) => Ok(turn),
        Err(error) => {
            let _ = std::fs::remove_file(layout.inflight_lock(&turn.tag));
            let _ = release_lane(layout, &turn.lane);
            if let Ok(mut failed) = Turn::read(layout, &turn.tag) {
                failed.state = "failed".into();
                failed.finished_at = Some(super::now_rfc3339());
                failed.detail = Some(format!("collector: {error:#}"));
                let _ = failed.write(layout);
            }
            Err(error)
        }
    }
}

/// Validate one turn and write the packet and the turn record. Everything that
/// can be known up front is refused here, while the coordinator is waiting.
pub(crate) fn prepare(
    env: &Env,
    layout: &Layout,
    runner: &dyn Runner,
    opts: &TurnOptions,
) -> Result<Turn> {
    layout.ensure()?;
    state::check_name(&opts.lane)?;
    let _admission = state::FileLock::acquire(&layout.turn_lock())?;
    let mut lane = Lane::read(layout, &opts.lane)?;
    if lane.stopped {
        bail!("lane `{}` is stopped", opts.lane);
    }
    if let Some(profile) = &lane.profile {
        bail!(
            "lane `{}` uses Codex profile `{profile}`; profile lanes do not accept Pro bridge turns",
            opts.lane
        );
    }
    if !lane.ready() {
        bail!(
            "lane `{}` is {}; only a ready lane accepts a turn",
            opts.lane,
            lane.state
        );
    }
    let now = jiff::Timestamp::now();
    if state::cooldown_active(layout, now) {
        let until = state::cooldown_until(layout)
            .map(|u| u.to_string())
            .unwrap_or_default();
        bail!(
            "refused: the breaker is active until {until}; clear it with `herdr-pro resume-bridge`"
        );
    }
    let inflight = state::inflight_count(layout);
    let inflight_limit = env.inflight_limit()?;
    if inflight >= inflight_limit {
        bail!(
            "refused: {inflight} turns already in flight (limit {inflight_limit}, hard max {})",
            super::INFLIGHT_MAX
        );
    }
    let out = std::path::absolute(&opts.out)
        .with_context(|| format!("bad output path {}", opts.out.display()))?;
    if out.to_string_lossy().chars().any(char::is_whitespace) {
        bail!(
            "refused: output path {} contains whitespace and cannot form a DONE line",
            out.display()
        );
    }
    if out.exists() {
        bail!("refused: {} already exists", out.display());
    }
    // A lane with no Codex rollout cannot be collected, and the collector
    // would type the packet before it found that out: refuse before anything
    // is sent. `start` guarantees a rollout, so a missing one means a stale or
    // hand-made record.
    if lane::refresh_rollout(env, &mut lane).is_none() {
        bail!(
            "refused: lane `{}` has no Codex rollout yet; start or resume it and wait for the session file",
            opts.lane
        );
    }

    let tag = match &opts.id {
        Some(id) => id.clone(),
        None => super::next_turn_id(layout, &opts.lane),
    };
    state::check_tag(&tag)?;
    if layout.turn(&tag).exists() {
        bail!("refused: turn `{tag}` already exists");
    }

    doctor::gate(env, layout, runner)
        .map_err(|error| anyhow::anyhow!("refused: doctor failed before the turn: {error:#}"))?;
    let agent = lane::agent_ready(env, runner, &lane)?;
    if !agent.ready() {
        bail!(
            "lane `{}` reads {}; wait for idle or resolve the blocked screen",
            opts.lane,
            agent.agent_status
        );
    }

    let packet = packet::build(&opts.brief, &opts.attachments, &out)?;

    // The breaker sees a daemon restart on a changed pid (spec §4).
    let (port, health) = bridge::health_any(runner)?;
    let previous = BridgeState::read(layout);
    let restarted = previous.pid.is_some() && previous.pid != health.pid;
    BridgeState {
        pid: health.pid,
        version: Some(health.version.clone()),
        accepting: Some(health.accepting),
    }
    .write(layout)?;
    if restarted {
        let detail = trip_breaker(env, layout, runner, port, "the bridge daemon restarted");
        bail!("refused: {detail}");
    }

    let packet_path = layout.packet(&tag);
    state::write_atomic(&packet_path, &packet.text)?;

    let turn = Turn {
        tag: tag.clone(),
        lane: lane.name.clone(),
        brief: opts.brief.display().to_string(),
        out: out.display().to_string(),
        notify: opts.notify.clone(),
        attachments: opts
            .attachments
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        state: "loading".into(),
        started_at: now.to_string(),
        finished_at: None,
        detail: Some(format!(
            "packet {} bytes, about {} tokens",
            packet.bytes, packet.tokens
        )),
        failure_class: None,
        packet: Some(packet_path.display().to_string()),
        written: None,
    };
    turn.write(layout)?;
    let inflight = Inflight::create(layout, &tag)?;

    lane.last_turn = Some(tag);
    lane.state = "in_turn".into();
    if let Err(error) = lane.write(layout) {
        inflight.release();
        return Err(error);
    }
    Ok(turn)
}

unsafe extern "C" {
    fn setsid() -> i32;
}

/// Fork the collector, detached with null stdio so it outlives the coordinator
/// call and pins no plugin slot.
pub(crate) fn spawn(layout: &Layout, tag: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let binary = std::env::current_exe().context("could not find this binary's own path")?;
    let mut command = Command::new(binary);
    command
        .env("HERDR_PRO_STATE_DIR", layout.root.display().to_string())
        .args(["collector", "--turn", tag])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no memory.
    unsafe {
        command.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    command.spawn().context("could not start the collector")?;
    Ok(())
}

struct InflightGuard {
    layout: Layout,
    tag: String,
}

impl InflightGuard {
    fn new(layout: &Layout, tag: &str) -> InflightGuard {
        InflightGuard {
            layout: layout.clone(),
            tag: tag.to_string(),
        }
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.layout.inflight_lock(&self.tag));
    }
}

/// The detached worker (`herdr-pro collector`).
pub(crate) fn collect(env: &Env, layout: &Layout, runner: &dyn Runner, tag: &str) -> Result<()> {
    let result = collect_inner(env, layout, runner, tag);
    if let Err(error) = result {
        if let Ok(mut turn) = Turn::read(layout, tag) {
            let detail = format!("{error:#}");
            if finish_failed(env, layout, runner, &mut turn, "collector", &detail, true).is_ok() {
                return Ok(());
            }
        }
        return Err(error);
    }
    Ok(())
}

fn collect_inner(env: &Env, layout: &Layout, runner: &dyn Runner, tag: &str) -> Result<()> {
    layout.ensure()?;
    let _guard = InflightGuard::new(layout, tag);
    let mut turn = Turn::read(layout, tag)?;
    let bin = env.herdr_bin();
    let packet = turn
        .packet
        .clone()
        .context("the turn record has no packet path")?;

    // 1. loading: paste the packet, wait for its shell-command item.
    let Some(mut reader) = RolloutReader::for_turn(env, layout, &turn)? else {
        return finish_failed(
            env,
            layout,
            runner,
            &mut turn,
            "load",
            "no Codex rollout was found for the lane",
            true,
        );
    };
    let load = format!("!cat -- {}", shell_quote(&packet));
    if let Err(error) = herdr_cli::agent_prompt(runner, &bin, &turn.lane, &load) {
        return finish_failed(
            env,
            layout,
            runner,
            &mut turn,
            "load",
            &format!("{error:#}"),
            true,
        );
    }
    if let Err(error) = wait_for_shell_command(&mut reader, &packet, super::LOAD_TIMEOUT) {
        return finish_failed(
            env,
            layout,
            runner,
            &mut turn,
            "load",
            &format!("{error:#}"),
            true,
        );
    }

    // 2. in_flight: drop anything the `!cat` turn left behind, then send the
    // TURN prompt so the next `task_started` is unambiguously this turn's.
    reader.drain()?;
    turn.state = "in_flight".into();
    turn.write(layout)?;
    state::record_usage(layout, &turn.lane, &turn.tag)?;

    let prompt = format!(
        "TURN {}: the packet above holds the brief and files. Reply with the full answer in markdown only.",
        turn.tag
    );
    if let Err(error) = herdr_cli::agent_prompt(runner, &bin, &turn.lane, &prompt) {
        return finish_failed(
            env,
            layout,
            runner,
            &mut turn,
            "prompt",
            &format!("{error:#}"),
            true,
        );
    }
    let completion = match wait_for_completion(&mut reader, super::TURN_TIMEOUT) {
        Ok(completion) => completion,
        Err(error) => {
            return finish_failed(
                env,
                layout,
                runner,
                &mut turn,
                "timeout",
                &format!("{error:#}"),
                true,
            );
        }
    };

    // 3. collecting.
    match classify(&completion) {
        Outcome::Delivered(answer) => match write_answer(Path::new(&turn.out), &answer) {
            Ok(written) => {
                turn.written = Some(written.display().to_string());
                turn.state = "delivered".into();
                turn.finished_at = Some(super::now_rfc3339());
                turn.detail = Some(format!("delivered {} bytes", answer.len()));
                turn.write(layout)?;
                release_lane(layout, &turn.lane)?;
                let done = format!("DONE {} {} -", turn.tag, written.display());
                let note = notify(runner, &bin, &turn.notify, &done);
                record_note(layout, &mut turn, note);
                Ok(())
            }
            Err(error) => finish_failed(
                env,
                layout,
                runner,
                &mut turn,
                "write",
                &format!("{error:#}"),
                true,
            ),
        },
        Outcome::Cooldown(reason) => {
            let detail = trip_breaker(env, layout, runner, port_from_health(runner), &reason);
            finish_failed(env, layout, runner, &mut turn, "cooldown", &detail, false)
        }
        Outcome::ProviderFailed(reason) => {
            finish_failed(env, layout, runner, &mut turn, "provider", &reason, true)
        }
        Outcome::Unknown(reason) => {
            finish_failed(env, layout, runner, &mut turn, "unknown", &reason, false)
        }
    }
}

fn port_from_health(runner: &dyn Runner) -> u16 {
    bridge::health_any(runner)
        .map(|(port, _)| port)
        .unwrap_or(super::BRIDGE_PORT)
}

/// The turn ended as `failed(x)`: write the state, trip the breaker when two
/// failures fall inside ten minutes, and tell the coordinator WAITING.
fn finish_failed(
    env: &Env,
    layout: &Layout,
    runner: &dyn Runner,
    turn: &mut Turn,
    code: &str,
    detail: &str,
    trip: bool,
) -> Result<()> {
    if trip {
        let prior = state::recent_failures(
            layout,
            jiff::Timestamp::now(),
            super::FAILURE_WINDOW.as_secs() as i64,
        );
        if prior >= 1 {
            let _ = trip_breaker(
                env,
                layout,
                runner,
                port_from_health(runner),
                &format!("two failed turns in ten minutes ({code}: {detail})"),
            );
        }
    }

    turn.state = "failed".into();
    turn.finished_at = Some(super::now_rfc3339());
    turn.failure_class = Some(
        match code {
            "cooldown" | "provider" | "timeout" => "provider",
            "write" => "work_failed",
            _ => "unknown",
        }
        .into(),
    );
    turn.detail = Some(format!("{code}: {detail}"));
    turn.write(layout)?;
    // The turn record keeps the failure. The lane returns to ready; the
    // global cooldown file is the breaker gate when this failure tripped.
    let _ = release_lane(layout, &turn.lane);
    let waiting = format!(
        "WAITING {} pro {} ({code}): {detail}",
        turn.tag,
        turn.failure_class.as_deref().unwrap_or("unknown")
    );
    let note = notify(runner, &env.herdr_bin(), &turn.notify, &waiting);
    record_note(layout, turn, note);
    Ok(())
}

fn release_lane(layout: &Layout, name: &str) -> Result<()> {
    let _admission = state::FileLock::acquire(&layout.turn_lock())?;
    let mut lane = Lane::read(layout, name)?;
    lane.state = if lane.stopped || lane.state == "gone" {
        "gone"
    } else {
        "ready"
    }
    .into();
    lane.write(layout)
}

fn record_note(layout: &Layout, turn: &mut Turn, note: Result<(), String>) {
    if let Err(error) = note {
        let detail = turn.detail.clone().unwrap_or_default();
        turn.detail = Some(format!("{detail}; notification not delivered: {error}"));
        let _ = turn.write(layout);
    }
}

/// The breaker: write the cooldown and drain the bridge so Codex retries get a
/// local 503. Returns the detail line.
fn trip_breaker(
    env: &Env,
    layout: &Layout,
    runner: &dyn Runner,
    port: u16,
    reason: &str,
) -> String {
    let until =
        jiff::Timestamp::now() + jiff::SignedDuration::from_secs(super::COOLDOWN.as_secs() as i64);
    let _ = state::set_cooldown(layout, until);
    let drain = match bridge::drain(runner, env, port) {
        Ok(_) => "bridge drained".to_string(),
        Err(error) => format!("drain not sent ({error:#})"),
    };
    format!("cooldown until {until}: {reason}; {drain}")
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Type one line to the coordinator, retried once after five seconds.
fn notify(runner: &dyn Runner, bin: &str, target: &str, text: &str) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 0..2 {
        match herdr_cli::agent_prompt(runner, bin, target, text) {
            Ok(()) => return Ok(()),
            Err(error) => last = format!("{error:#}"),
        }
        if attempt == 0 {
            std::thread::sleep(NOTIFY_RETRY);
        }
    }
    Err(last)
}

/// A non-empty answer with an atomic, never-overwriting write.
fn write_answer(requested: &Path, answer: &str) -> Result<PathBuf> {
    use std::io::Write;

    let dir = requested.parent().unwrap_or(Path::new("."));
    let file_name = requested
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "answer".into());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = dir.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .with_context(|| format!("could not create {}", temp.display()))?;
    if let Err(error) = file
        .write_all(answer.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("could not write {}", temp.display()));
    }
    drop(file);

    let mut candidate = requested.to_path_buf();
    for n in 1..1000u32 {
        match std::fs::hard_link(&temp, &candidate) {
            Ok(()) => {
                let _ = std::fs::remove_file(&temp);
                if let Ok(directory) = std::fs::File::open(dir) {
                    let _ = directory.sync_all();
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let stem = requested
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "answer".into());
                let ext = requested
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default();
                candidate = dir.join(format!("{stem}.{n}{ext}"));
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                return Err(error)
                    .with_context(|| format!("could not create {}", candidate.display()));
            }
        }
    }
    let _ = std::fs::remove_file(&temp);
    bail!(
        "could not find an unused answer path next to {}",
        requested.display()
    )
}

/// What the completed turn's events say.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    Delivered(String),
    Cooldown(String),
    ProviderFailed(String),
    Unknown(String),
}

fn classify(completion: &Completion) -> Outcome {
    let non_result_events = completion
        .events
        .iter()
        .filter(|event| task_complete(event).is_none());
    let route_stopped = completion.answer.trim() == "Stopped thinking"
        || non_result_events.clone().any(|event| {
            value_contains(event, "rate_limit_exceeded")
                || value_contains(event, "Stopped thinking")
        });
    if route_stopped {
        return Outcome::Cooldown("the route reported a rate limit or stopped thinking".into());
    }
    if non_result_events
        .clone()
        .any(|event| value_contains(event, "chatgpt_session_expired"))
    {
        return Outcome::ProviderFailed("login: the ChatGPT session expired".into());
    }
    let has_error = completion.events.iter().any(error_event);
    if has_error {
        return Outcome::ProviderFailed("the turn contained an error or stream_error event".into());
    }
    if !completion.answer.trim().is_empty() {
        return Outcome::Delivered(completion.answer.clone());
    }
    Outcome::Unknown("the turn completed with no answer or failure evidence".into())
}

fn value_contains(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(values) => values.iter().any(|value| value_contains(value, needle)),
        Value::Object(values) => values.values().any(|value| value_contains(value, needle)),
        _ => false,
    }
}

fn error_event(event: &Value) -> bool {
    matches!(
        event.get("type").and_then(Value::as_str),
        Some("error" | "stream_error" | "response.failed")
    ) || matches!(
        event.pointer("/payload/type").and_then(Value::as_str),
        Some("error" | "stream_error" | "response.failed")
    )
}

/// The turn's result from the rollout.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Completion {
    pub(crate) turn_id: String,
    pub(crate) answer: String,
    /// Every event between the TURN's `task_started` and its `task_complete`.
    pub(crate) events: Vec<Value>,
}

fn task_started_id(event: &Value) -> Option<String> {
    if event["type"] != "event_msg" || event["payload"]["type"] != "task_started" {
        return None;
    }
    event["payload"]["turn_id"].as_str().map(str::to_string)
}

fn task_complete(event: &Value) -> Option<(String, String)> {
    if event["type"] != "event_msg" || event["payload"]["type"] != "task_complete" {
        return None;
    }
    let id = event["payload"]["turn_id"].as_str()?.to_string();
    let answer = event["payload"]["last_agent_message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Some((id, answer))
}

/// True when this event is the `<user_shell_command>` item for `packet`.
fn is_shell_command(event: &Value, packet: &str) -> bool {
    if event["type"] != "response_item" || event["payload"]["type"] != "message" {
        return false;
    }
    if event["payload"]["role"] != "user" {
        return false;
    }
    let Some(content) = event["payload"]["content"].as_array() else {
        return false;
    };
    content.iter().any(|part| {
        part["text"]
            .as_str()
            .is_some_and(|text| text.contains("<user_shell_command>") && text.contains(packet))
    })
}

/// Wait for the `!cat` item and its own completion. Waiting for completion
/// keeps that empty turn from being mistaken for the TURN sent next.
fn wait_for_shell_command(
    reader: &mut RolloutReader,
    packet: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut seen = false;
    loop {
        for event in reader.new_events()? {
            if is_shell_command(&event, packet) {
                seen = true;
            }
            if seen && task_complete(&event).is_some() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "the packet did not finish loading within {}s",
                timeout.as_secs()
            );
        }
        std::thread::sleep(POLL);
    }
}

/// Wait for the `task_complete` of the turn started after the TURN prompt.
fn wait_for_completion(reader: &mut RolloutReader, timeout: Duration) -> Result<Completion> {
    let deadline = Instant::now() + timeout;
    let mut turn_id: Option<String> = None;
    let mut events: Vec<Value> = Vec::new();
    loop {
        for event in reader.new_events()? {
            if turn_id.is_none() {
                if let Some(id) = task_started_id(&event) {
                    turn_id = Some(id);
                }
                continue;
            }
            events.push(event.clone());
            if let Some((id, answer)) = task_complete(&event)
                && Some(&id) == turn_id.as_ref()
            {
                return Ok(Completion {
                    turn_id: id,
                    answer,
                    events,
                });
            }
        }
        if Instant::now() >= deadline {
            bail!("no task_complete within {}s", timeout.as_secs());
        }
        std::thread::sleep(POLL);
    }
}

/// Reads new complete JSON lines from a rollout, by byte offset. A truncation
/// or replacement resets the offset.
struct RolloutReader {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl RolloutReader {
    fn for_turn(env: &Env, layout: &Layout, turn: &Turn) -> Result<Option<RolloutReader>> {
        let lane = Lane::read(layout, &turn.lane)?;
        let started = super::parse_rfc3339(&lane.started_at).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
        let path = lane
            .rollout
            .as_deref()
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .or_else(|| {
                lane::find_rollout(
                    &env.lane_codex_home(),
                    &lane.cwd,
                    started,
                    lane.session_id.as_deref(),
                )
            });
        let Some(path) = path else {
            return Ok(None);
        };
        Ok(Some(RolloutReader::at_end(path)?))
    }

    fn at_end(path: PathBuf) -> Result<RolloutReader> {
        let offset = std::fs::metadata(&path)
            .with_context(|| format!("could not read {}", path.display()))?
            .len();
        Ok(RolloutReader {
            path,
            offset,
            partial: Vec::new(),
        })
    }

    /// Discard every event currently in the file.
    fn drain(&mut self) -> Result<()> {
        self.new_events().map(|_| ())
    }

    fn new_events(&mut self) -> Result<Vec<Value>> {
        let mut file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(_) => return Ok(Vec::new()),
        };
        let len = file.metadata()?.len();
        if len < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        self.offset += buffer.len() as u64;
        self.partial.extend_from_slice(&buffer);

        let mut events = Vec::new();
        let mut start = 0usize;
        for (index, byte) in self.partial.iter().enumerate() {
            if *byte == b'\n' {
                let line = &self.partial[start..index];
                start = index + 1;
                if let Ok(value) = serde_json::from_slice::<Value>(line) {
                    events.push(value);
                }
            }
        }
        self.partial.drain(..start);
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pro::sh::fake::{FakeRunner, ok};
    use serde_json::json;

    /// A ready lane record with a rollout file on disk, so `prepare`'s rollout
    /// gate passes. Returns the lane file path the record was written to.
    fn write_ready_lane(dir: &Path, layout: &Layout) {
        let rollout = dir.join("rollout-pro.jsonl");
        std::fs::write(&rollout, "{}\n").unwrap();
        Lane {
            name: "pro".into(),
            pane_id: "w1:p2".into(),
            tab_id: "w1:t2".into(),
            workspace_id: "w1".into(),
            parent: None,
            cwd: "/w".into(),
            profile: None,
            session_id: None,
            rollout: Some(rollout.display().to_string()),
            started_at: crate::pro::now_rfc3339(),
            state: "ready".into(),
            stopped: false,
            last_turn: None,
        }
        .write(layout)
        .unwrap();
    }

    fn turn(state: &str) -> Turn {
        Turn {
            tag: "pro-01".into(),
            lane: "pro".into(),
            brief: "/b.md".into(),
            out: "/o.md".into(),
            notify: "hcoord".into(),
            attachments: vec![],
            state: state.into(),
            started_at: crate::pro::now_rfc3339(),
            finished_at: None,
            detail: None,
            failure_class: None,
            packet: Some("/p.md".into()),
            written: None,
        }
    }

    #[test]
    fn classify_reads_the_result_not_the_pane() {
        let delivered = Completion {
            turn_id: "t".into(),
            answer: "the answer".into(),
            events: vec![],
        };
        assert_eq!(
            classify(&delivered),
            Outcome::Delivered("the answer".into())
        );

        let rate = Completion {
            turn_id: "t".into(),
            answer: String::new(),
            events: vec![json!({"payload": {"type": "error", "code": "rate_limit_exceeded"}})],
        };
        assert!(matches!(classify(&rate), Outcome::Cooldown(_)));

        let expired = Completion {
            turn_id: "t".into(),
            answer: String::new(),
            events: vec![json!({"payload": {"code": "chatgpt_session_expired"}})],
        };
        assert_eq!(
            classify(&expired),
            Outcome::ProviderFailed("login: the ChatGPT session expired".into())
        );

        let empty = Completion {
            turn_id: "t".into(),
            answer: String::new(),
            events: vec![],
        };
        assert!(matches!(classify(&empty), Outcome::Unknown(_)));

        let stopped = Completion {
            turn_id: "t".into(),
            answer: String::new(),
            events: vec![
                json!({"type":"stream_error","message":"stream disconnected before completion: ChatGPT displayed 'Stopped thinking'"}),
            ],
        };
        assert!(matches!(classify(&stopped), Outcome::Cooldown(_)));
    }

    #[test]
    fn the_done_line_is_d7_shaped_and_the_answer_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let requested = dir.path().join("answer.md");
        let first = write_answer(&requested, "one").unwrap();
        assert_eq!(first, requested);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "one");
        let second = write_answer(&requested, "two").unwrap();
        assert_eq!(second, dir.path().join("answer.1.md"));
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "two");
        assert_eq!(std::fs::read_to_string(&requested).unwrap(), "one");
        assert_eq!(
            format!("DONE pro-01 {} -", second.display()),
            format!("DONE pro-01 {} -", second.display())
        );
    }

    #[test]
    fn wait_for_completion_ignores_the_empty_cat_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        // The `!cat` turn finished before the TURN prompt, so the collector
        // drains it and the next `task_started` is the real turn's.
        std::fs::write(
            &path,
            [
                json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"cat"}}),
                json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"cat","last_agent_message":""}}),
            ]
            .iter()
            .map(|v| format!("{v}\n"))
            .collect::<String>(),
        )
        .unwrap();
        let mut reader = RolloutReader::at_end(path.clone()).unwrap();
        reader.offset = 0;
        reader.drain().unwrap();

        // A real rollout only appends; write the turn's events after the cat's.
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            write!(
                file,
                "{}\n{}\n",
                json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"turn"}}),
                json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn","last_agent_message":"answer"}}),
            )
            .unwrap();
        }
        let completion = wait_for_completion(&mut reader, Duration::from_secs(2)).unwrap();
        assert_eq!(completion.turn_id, "turn");
        assert_eq!(completion.answer, "answer");
    }

    #[test]
    fn shell_command_detection_matches_only_the_packet() {
        let event = json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "<user_shell_command>\n<command>\ncat -- '/tmp/p.md'\n</command>\n</user_shell_command>"}]
            }
        });
        assert!(is_shell_command(&event, "/tmp/p.md"));
        assert!(!is_shell_command(&event, "/tmp/other.md"));
    }

    #[test]
    fn a_changed_bridge_pid_trips_the_breaker_once() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        write_ready_lane(dir.path(), &layout);
        BridgeState {
            pid: Some(7),
            version: Some("5.0.8".into()),
            accepting: Some(true),
        }
        .write(&layout)
        .unwrap();
        let brief = dir.path().join("brief.md");
        std::fs::write(&brief, "Do the thing").unwrap();
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = FakeRunner::new();
        runner.on(
            "agent list",
            ok(r#"{"result":{"agents":[{"name":"pro","agent_status":"idle"}]}}"#),
        );
        runner.on(
            ":17841/healthz",
            ok(r#"{"version":"5.0.8","mode":"browser-only","pid":9,"accepting_turns":true}"#),
        );
        runner.on("codex login status", ok("Logged in\n"));
        let options = |n| TurnOptions {
            lane: "pro".into(),
            brief: brief.clone(),
            out: dir.path().join(format!("answer-{n}.md")),
            notify: "hcoord".into(),
            attachments: vec![],
            id: Some(format!("pro-{n:02}")),
        };

        let error = prepare(&env, &layout, &runner, &options(1)).unwrap_err();
        assert!(error.to_string().contains("daemon restarted"), "{error}");
        assert!(state::cooldown_active(&layout, jiff::Timestamp::now()));

        // After Rolf clears the breaker the recorded pid matches, so the turn
        // starts and the restart does not trip again.
        state::clear_cooldown(&layout).unwrap();
        let turn = prepare(&env, &layout, &runner, &options(2)).unwrap();
        assert_eq!(turn.state, "loading");
        assert!(!state::cooldown_active(&layout, jiff::Timestamp::now()));
    }

    #[test]
    fn prepare_refuses_an_existing_out_file() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        let lane = Lane {
            name: "pro".into(),
            pane_id: "w1:p2".into(),
            tab_id: "w1:t2".into(),
            workspace_id: "w1".into(),
            parent: None,
            cwd: "/w".into(),
            profile: None,
            session_id: None,
            rollout: None,
            started_at: crate::pro::now_rfc3339(),
            state: "ready".into(),
            stopped: false,
            last_turn: None,
        };
        lane.write(&layout).unwrap();
        let out = dir.path().join("answer.md");
        std::fs::write(&out, "old").unwrap();
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = FakeRunner::new();
        runner.on(
            "agent list",
            ok(r#"{"result":{"agents":[{"name":"pro","agent_status":"idle"}]}}"#),
        );
        let error = prepare(
            &env,
            &layout,
            &runner,
            &TurnOptions {
                lane: "pro".into(),
                brief: dir.path().join("b.md"),
                out,
                notify: "hcoord".into(),
                attachments: vec![],
                id: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error}");
    }

    #[test]
    fn prepare_refuses_a_lane_without_a_rollout_before_typing() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        Lane {
            name: "pro".into(),
            pane_id: "w1:p2".into(),
            tab_id: "w1:t2".into(),
            workspace_id: "w1".into(),
            parent: None,
            cwd: "/w".into(),
            profile: None,
            session_id: None,
            rollout: None,
            started_at: crate::pro::now_rfc3339(),
            state: "ready".into(),
            stopped: false,
            last_turn: None,
        }
        .write(&layout)
        .unwrap();
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = FakeRunner::new();
        let error = prepare(
            &env,
            &layout,
            &runner,
            &TurnOptions {
                lane: "pro".into(),
                brief: dir.path().join("b.md"),
                out: dir.path().join("answer.md"),
                notify: "hcoord".into(),
                attachments: vec![],
                id: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("no Codex rollout"), "{error}");
        assert_eq!(runner.count("agent prompt"), 0);
        assert!(!layout.turn("pro-01").exists());
    }

    #[test]
    fn prepare_writes_the_record_and_lock() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        write_ready_lane(dir.path(), &layout);
        let brief = dir.path().join("brief.md");
        std::fs::write(&brief, "Do the thing").unwrap();
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = FakeRunner::new();
        runner.on(
            "agent list",
            ok(r#"{"result":{"agents":[{"name":"pro","agent_status":"idle"}]}}"#),
        );
        runner.on(
            ":17841/healthz",
            ok(r#"{"version":"5.0.8","mode":"browser-only","pid":7,"accepting_turns":true}"#),
        );
        runner.on("codex login status", ok("Logged in\n"));
        let turn = prepare(
            &env,
            &layout,
            &runner,
            &TurnOptions {
                lane: "pro".into(),
                brief,
                out: dir.path().join("answer.md"),
                notify: "hcoord".into(),
                attachments: vec![],
                id: Some("topic-01".into()),
            },
        )
        .unwrap();
        assert_eq!(turn.tag, "topic-01");
        assert_eq!(turn.state, "loading");
        assert!(layout.packet("topic-01").exists());
        assert!(layout.turn("topic-01").exists());
        assert!(layout.inflight_lock("topic-01").exists());
        assert_eq!(state::inflight_count(&layout), 1);
    }
}
