//! Immutable completion events, their append-only delivery journals, and the
//! Mac-side import of box envelopes (SPEC-remote §4.3).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::{DeliveryLine, DeliveryState, Event, EventPayload};
use crate::project::{self, Project};
use crate::thread::sha256_hex as hash_bytes;

pub(crate) fn dir(project: &Project) -> PathBuf {
    project.record_dir("events")
}

fn deliveries_dir(project: &Project) -> PathBuf {
    project.record_dir("deliveries")
}

pub(crate) fn event_path(project: &Project, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(dir(project).join(format!("{id}.toml")))
}

fn journal_path(project: &Project, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(deliveries_dir(project).join(format!("{id}.jsonl")))
}

fn validate_id(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && !id.starts_with('.')
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if !valid {
        bail!("`{id}` is not an event id");
    }
    Ok(())
}

fn validate_machine(machine: &str) -> Result<()> {
    let valid = !machine.is_empty()
        && !machine.starts_with('.')
        && !machine.contains("..")
        && machine
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !valid {
        bail!("`{machine}` is not a machine profile id");
    }
    Ok(())
}

/// The Mac's content-addressed artifact folder (SPEC-remote §4.3).
fn artifact_dir(project: &Project) -> PathBuf {
    project.state_dir().join("artifacts")
}

/// The Mac path of one artifact, named by its own hash.
pub(crate) fn artifact_path(project: &Project, hash: &str) -> PathBuf {
    artifact_dir(project).join(hash)
}

/// The source tuple recorded for every box event imported on the Mac
/// (SPEC-remote §4.3, D5/D10). It is the taken cursor's entry: the box event
/// id and the hash of the box's own bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub(crate) struct ImportSource {
    /// Stable saved-profile id the envelope came from.
    pub(crate) machine: String,
    /// Box project slug (the Mac's slug).
    pub(crate) project: String,
    pub(crate) event: String,
    pub(crate) event_hash: String,
    /// Empty for a `waiting` envelope.
    pub(crate) artifact_hash: String,
    pub(crate) imported: String,
}

fn imports_dir(project: &Project) -> PathBuf {
    project.record_dir("imports")
}

fn import_path(project: &Project, machine: &str, event: &str) -> Result<PathBuf> {
    validate_machine(machine)?;
    validate_id(event)?;
    Ok(imports_dir(project)
        .join(machine)
        .join(format!("{event}.toml")))
}

fn load_import(project: &Project, machine: &str, event: &str) -> Result<Option<ImportSource>> {
    let path = import_path(project, machine, event)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    match std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| toml::from_str::<ImportSource>(text).ok())
    {
        Some(source)
            if !source.event.is_empty()
                && !source.event_hash.is_empty()
                && !source.machine.is_empty()
                && !source.project.is_empty()
                && !source.imported.is_empty() =>
        {
            Ok(Some(source))
        }
        _ => {
            eprintln!("interrupted import marker: {}", path.display());
            Ok(None)
        }
    }
}

/// What an [`import_box_event`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportOutcome {
    /// The event and its artifact were written to the Mac event records.
    New,
    /// The exact `(machine, event id, hash)` was already imported; nothing
    /// was rewritten.
    Replay,
}

/// Imports one box envelope into the Mac's canonical event records, create-only
/// (SPEC-remote §4.3). The box's own bytes must hash to `event_hash`; the
/// artifact must hash to the event's `artifact`. The sealed event bytes remain
/// historical evidence; file lookup uses the artifact hash. The same event id
/// with different bytes is corruption and refuses.
pub(crate) fn import_box_event(
    project: &Project,
    machine: &str,
    box_bytes: &[u8],
    artifact: Option<&[u8]>,
) -> Result<ImportOutcome> {
    validate_machine(machine)?;
    let text = std::str::from_utf8(box_bytes).context("box event is not UTF-8")?;
    let event: Event = toml::from_str(text).context("box event does not parse")?;
    validate_id(&event.id)?;
    // Use the same boundary as local sealing so a fresh merge intent cannot
    // race an imported completion into existence after its final check.
    let _lock = project.lock()?;
    let event_hash = hash_bytes(box_bytes);
    if let Some(existing) = load_import(project, machine, &event.id)? {
        if existing.event_hash == event_hash {
            return Ok(ImportOutcome::Replay);
        }
        bail!(
            "event_conflict: box event {} already imported with hash {}, now {event_hash}",
            event.id,
            existing.event_hash
        );
    }

    let mut artifact_hash = String::new();
    if let Some(done) = &event.payload.done {
        let bytes = artifact.with_context(|| {
            format!(
                "artifact_missing: event {} names artifact {}",
                event.id, done.artifact
            )
        })?;
        let got = hash_bytes(bytes);
        if got != done.artifact {
            bail!(
                "artifact_mismatch: event {} artifact is {}, fetched {got}",
                event.id,
                done.artifact
            );
        }
        store_artifact(project, bytes)?;
        artifact_hash = done.artifact.clone();
    }

    seal_create_if_absent(project, &event)
        .with_context(|| format!("could not import event {}", event.id))?;

    let source = ImportSource {
        machine: machine.to_string(),
        project: project.slug.clone(),
        event: event.id.clone(),
        event_hash,
        artifact_hash,
        imported: project::now(),
    };
    write_import_create_only(project, &source)?;
    Ok(ImportOutcome::New)
}

fn write_import_create_only(project: &Project, source: &ImportSource) -> Result<()> {
    project.record_dir_for_write("imports")?;
    let path = import_path(project, &source.machine, &source.event)?;
    let mut text = toml::to_string(source)?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() && load_import(project, &source.machine, &source.event)?.is_none() {
        // Historical in-place writes could leave a partial marker. Its source
        // is reconstructible from the hash-checked immutable box event.
        project::write_atomic(&path, text.as_bytes())?;
        return sync_parent(&path);
    }
    project::write_create_only(&path, text.as_bytes())
        .with_context(|| format!("import_conflict: {}", path.display()))
}

/// One immutable artifact store for briefs, operations and imported reports.
/// Publish complete bytes create-only, after file sync, then sync the directory.
pub(crate) fn store_artifact(project: &Project, bytes: &[u8]) -> Result<String> {
    let hash = hash_bytes(bytes);
    std::fs::create_dir_all(artifact_dir(project))?;
    let path = artifact_path(project, &hash);
    project::write_create_only(&path, bytes)
        .with_context(|| format!("artifact_conflict: {}", path.display()))?;
    Ok(hash)
}

/// A box lane's Mac-side courier state, one file per (profile id, project)
/// (SPEC-remote §4.3). `taken` is the cursor: box event id -> the hash of the
/// box's bytes. `missing` counts consecutive successful pane-list snapshots
/// without the lane's pane; `gone` and `blocked` stop a line being typed twice.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub(crate) struct RemoteState {
    pub(crate) boot_id: String,
    pub(crate) last_pass: String,
    pub(crate) taken: BTreeMap<String, String>,
    pub(crate) missing: BTreeMap<String, u32>,
    /// Attempt/pane that owns each consecutive absence streak.
    pub(crate) missing_identity: BTreeMap<String, String>,
    /// Attempt and placement owning blocked/gone observations.
    pub(crate) attention_identity: BTreeMap<String, String>,
    pub(crate) gone: BTreeSet<String>,
    /// Boot-change GONE notices that still owe a coordinator wake-up.
    pub(crate) pending_gone: BTreeSet<String>,
    pub(crate) blocked: BTreeSet<String>,
}

fn remote_state_path(project: &Project, machine: &str) -> Result<PathBuf> {
    validate_machine(machine)?;
    Ok(project
        .state_dir()
        .join("remote")
        .join(format!("{machine}.json")))
}

pub(crate) fn remote_state(project: &Project, machine: &str) -> RemoteState {
    remote_state_path(project, machine)
        .ok()
        .and_then(|path| project::read_json(&path))
        .unwrap_or_default()
}

pub(crate) fn save_remote_state(
    project: &Project,
    machine: &str,
    state: &RemoteState,
) -> Result<()> {
    let path = remote_state_path(project, machine)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = project.lock()?;
    project::write_json(&path, state)
}

/// Canonical bytes used both for the create-if-absent write and the X2b
/// equality check. The event's field order is fixed by the contract type.
pub(crate) fn bytes(event: &Event) -> Result<Vec<u8>> {
    let mut text = toml::to_string(event)?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text.into_bytes())
}

/// Creates an immutable event. If another helper already created it, only
/// exact byte equality is accepted.
pub(crate) fn seal_create_if_absent(project: &Project, event: &Event) -> Result<()> {
    project.record_dir_for_write("events")?;
    let path = event_path(project, &event.id)?;
    let expected = bytes(event)?;
    project::write_create_only(&path, &expected)
        .with_context(|| format!("event_conflict: sealed event {}", event.id))
}

/// The receipt written when a `done` or `waiting` event is sealed (D5): the
/// bytes the sealer itself hashed. The courier carries it to the Mac, which
/// compares it with the fetched bytes before the taken cursor advances.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub(crate) struct Receipt {
    pub(crate) event: String,
    pub(crate) event_hash: String,
    /// The content-addressed artifact name; empty for `waiting`.
    pub(crate) artifact: String,
    pub(crate) artifact_hash: String,
    pub(crate) created: String,
}

fn receipts_dir(project: &Project) -> PathBuf {
    project.record_dir("receipts")
}

pub(crate) fn receipt_path(project: &Project, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(receipts_dir(project).join(format!("{id}.toml")))
}

/// Writes the receipt create-only. A retry with the same bytes is a no-op;
/// different bytes for a sealed event are corruption.
pub(crate) fn write_receipt(project: &Project, event: &Event) -> Result<()> {
    project.record_dir_for_write("receipts")?;
    let event_hash = hash_bytes(&bytes(event)?);
    let (artifact, artifact_hash) = match &event.payload.done {
        Some(done) => (done.artifact.clone(), done.artifact.clone()),
        None => (String::new(), String::new()),
    };
    let receipt = Receipt {
        event: event.id.clone(),
        event_hash,
        artifact,
        artifact_hash,
        // Derive this from the immutable event. If sealing crashes after the
        // receipt write but before the op marker, X2b must reproduce exactly
        // the same receipt on a later courier pass.
        created: event.created.clone(),
    };
    let path = receipt_path(project, &event.id)?;
    let mut text = toml::to_string(&receipt)?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if path.exists() {
        let actual =
            std::fs::read(&path).with_context(|| format!("could not read {}", path.display()))?;
        let complete = std::str::from_utf8(&actual)
            .ok()
            .and_then(|text| toml::from_str::<Receipt>(text).ok())
            .is_some_and(|receipt| {
                !receipt.event.is_empty()
                    && !receipt.event_hash.is_empty()
                    && !receipt.created.is_empty()
                    && (event.payload.done.is_none()
                        || (!receipt.artifact.is_empty() && !receipt.artifact_hash.is_empty()))
            });
        if !complete {
            // Rebuild only incomplete historical receipts, from the event
            // whose immutable bytes have already passed the seal equality check.
            eprintln!(
                "interrupted receipt: {}; rebuilding from sealed event {}",
                path.display(),
                event.id
            );
            project::write_atomic(&path, text.as_bytes())?;
            return sync_parent(&path);
        }
    }
    project::write_create_only(&path, text.as_bytes())
        .with_context(|| format!("receipt_conflict: {}", path.display()))
}

pub(crate) fn load(project: &Project, id: &str) -> Result<Event> {
    let path = event_path(project, id)?;
    #[cfg(test)]
    EVENT_READS.with(|count| count.set(count.get() + 1));
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read event {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("{} does not parse", path.display()))
}

#[cfg(test)]
thread_local! {
    static EVENT_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn count_event_reads(f: impl FnOnce()) -> usize {
    EVENT_READS.with(|count| {
        let previous = count.replace(0);
        f();
        count.replace(previous)
    })
}

/// Load the event records once, retaining the unreadable-evidence signal used
/// by task projections. Missing event directories represent an empty set.
pub(crate) fn list_checked(project: &Project) -> (Vec<Event>, bool) {
    if let Some(snapshot) = cached_events(project, None) {
        return snapshot;
    }
    let entries = match std::fs::read_dir(dir(project)) {
        Ok(entries) => entries,
        Err(error) => return (Vec::new(), error.kind() == std::io::ErrorKind::NotFound),
    };
    let mut events = Vec::new();
    let mut readable = true;
    for entry in entries {
        let Ok(entry) = entry else {
            readable = false;
            continue;
        };
        if entry.path().extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let Some(id) = entry
            .file_name()
            .into_string()
            .ok()
            .and_then(|name| name.strip_suffix(".toml").map(str::to_owned))
        else {
            readable = false;
            continue;
        };
        match load(project, &id) {
            Ok(event) => events.push(event),
            Err(_) => readable = false,
        }
    }
    events.sort_by(|a, b| a.id.cmp(&b.id));
    (events, readable)
}

pub(crate) fn list(project: &Project) -> Vec<Event> {
    list_checked(project).0
}

#[derive(Default)]
struct EventLists {
    records: crate::record_cache::Records<Event>,
    indexes: BTreeMap<PathBuf, EventIndex>,
    unresolved: BTreeMap<PathBuf, UnresolvedEvents>,
}

struct UnresolvedEvents {
    events: std::rc::Rc<Vec<Event>>,
    threads: std::rc::Rc<Vec<crate::thread::Thread>>,
    rows: Vec<Event>,
}

struct EventIndex {
    rows: std::rc::Rc<Vec<Event>>,
    threads: BTreeMap<String, Vec<usize>>,
}

thread_local! {
    static TICKER_LISTS: std::cell::RefCell<Option<EventLists>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn set_cache(enabled: bool) {
    TICKER_LISTS.with(|cache| *cache.borrow_mut() = enabled.then(EventLists::default));
}

fn cached_events(project: &Project, thread: Option<&str>) -> Option<(Vec<Event>, bool)> {
    TICKER_LISTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let cache = cache.as_mut()?;
        let dir = dir(project);
        let (rows, errors) = cache.records.read(dir.clone(), |id| load(project, id));
        let index = cache.indexes.entry(dir).or_insert_with(|| EventIndex {
            rows: std::rc::Rc::new(Vec::new()),
            threads: BTreeMap::new(),
        });
        if !std::rc::Rc::ptr_eq(&index.rows, &rows) {
            index.threads.clear();
            for (position, event) in rows.iter().enumerate() {
                index
                    .threads
                    .entry(event.thread.clone())
                    .or_default()
                    .push(position);
            }
            index.rows = rows;
        }
        let events = match thread {
            Some(thread) => index
                .threads
                .get(thread)
                .into_iter()
                .flatten()
                .map(|position| index.rows[*position].clone())
                .collect(),
            None => index.rows.as_ref().clone(),
        };
        Some((events, errors.is_empty()))
    })
}

/// Per-lane lookups never rescan or clone the entire project's event log in
/// the ticker. Ordinary commands still read all historical evidence afresh.
pub(crate) fn for_thread(project: &Project, thread: &str) -> Vec<Event> {
    for_thread_snapshot(project, thread).0
}

fn for_thread_snapshot(project: &Project, thread: &str) -> (Vec<Event>, bool) {
    cached_events(project, Some(thread)).unwrap_or_else(|| {
        let (events, readable) = list_checked(project);
        (
            events
                .into_iter()
                .filter(|event| event.thread == thread)
                .collect(),
            readable,
        )
    })
}

/// Keep the whole-log unreadable-evidence check without cloning its history
/// for each lane or open reviewer.
pub(crate) fn checked_for_thread(project: &Project, thread: &str) -> Result<Vec<Event>> {
    let (events, readable) = for_thread_snapshot(project, thread);
    if !readable {
        bail!("event records are unreadable");
    }
    Ok(events)
}

/// Delivery/recovery has no obligations on resolved lanes. Keep orphaned
/// evidence in this projection so missing/corrupt lane errors are not hidden.
/// Rebuild only when the event or thread snapshot changes.
pub(crate) fn for_unresolved_threads(project: &Project) -> Vec<Event> {
    let threads = crate::thread::snapshot(project);
    let select = |events: &[Event]| {
        let resolved: BTreeSet<_> = threads
            .iter()
            .filter(|t| t.status == crate::thread::Status::Resolved)
            .map(|t| t.id.as_str())
            .collect();
        events
            .iter()
            .filter(|e| {
                !resolved.contains(e.thread.as_str())
                    // Recovery rejects a multi-tag failure before checking lane
                    // status. Keep that existing corruption signal visible.
                    || e.payload.failed.is_some()
                        && (e.payload.done.is_some() || e.payload.waiting.is_some())
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    TICKER_LISTS
        .with(|cache| {
            let mut cache = cache.borrow_mut();
            let cache = cache.as_mut()?;
            let dir = dir(project);
            let (events, _) = cache.records.read(dir.clone(), |id| load(project, id));
            let entry = cache
                .unresolved
                .entry(dir)
                .or_insert_with(|| UnresolvedEvents {
                    rows: select(&events),
                    events: events.clone(),
                    threads: threads.clone(),
                });
            if !std::rc::Rc::ptr_eq(&entry.events, &events)
                || !std::rc::Rc::ptr_eq(&entry.threads, &threads)
            {
                entry.rows = select(&events);
                entry.events = events;
                entry.threads = threads.clone();
            }
            Some(entry.rows.clone())
        })
        .unwrap_or_else(|| select(&list(project)))
}

/// Incident input only: both failed seals and dependency waits are evidence,
/// but neither is a diagnosis. A caller must explicitly confirm a common cause.
pub(crate) fn incident_text(event: &Event) -> Option<&str> {
    match (
        &event.payload.failed,
        &event.payload.waiting,
        &event.payload.done,
    ) {
        (Some(failed), None, None) => Some(&failed.text),
        (None, Some(waiting), None) => Some(&waiting.text),
        _ => None,
    }
}

/// Read the existing recovery journal, not a second incident writer. Missing
/// or corrupt input is surfaced in the local projection, never in delivery.
pub(crate) fn recovery_facts(project: &Project, event: &str) -> Vec<String> {
    let path = project.state_dir().join("dispatch.jsonl");
    let (rows, error) = project::read_jsonl::<serde_json::Value>(&path)
        .unwrap_or_else(|error| (vec![], Some(error.to_string().into_bytes())));
    let mut facts: Vec<_> = rows
        .into_iter()
        .filter(|row| row["event"].as_str() == Some(event))
        .map(|row| {
            format!(
                "{}: {}",
                row["kind"].as_str().unwrap_or("unknown"),
                row["error"].as_str().unwrap_or("details unknown")
            )
        })
        .collect();
    if error.is_some() {
        facts.push("recovery journal evidence unknown".into());
    }
    facts
}

/// Appends a fact once. Re-running acknowledgement or handling is idempotent;
/// `submitted` may still be duplicated when the transport succeeded before a
/// crash, which is the intentional X4 at-least-once boundary.
pub(crate) fn append_delivery(project: &Project, event: &str, state: DeliveryState) -> Result<()> {
    let _lock = project.lock()?;
    append_delivery_locked(project, event, state)
}

pub(crate) fn append_delivery_locked(
    project: &Project,
    event: &str,
    state: DeliveryState,
) -> Result<()> {
    project.record_dir_for_write("deliveries")?;
    let rows = delivery_lines(project, event)?;
    if state != DeliveryState::Submitted && rows.iter().any(|line| line.state == state) {
        return Ok(());
    }
    let dir = deliveries_dir(project).join(event);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{:08}.json", rows.len() + 1));
    let line = DeliveryLine {
        event: event.to_string(),
        state,
    };
    project::write_create_only(&path, &serde_json::to_vec(&line)?)
}

fn delivery_lines(project: &Project, event: &str) -> Result<Vec<DeliveryLine>> {
    let path = journal_path(project, event)?;
    let (mut rows, _) = project::read_jsonl::<DeliveryLine>(&path)?;
    let dir = deliveries_dir(project).join(event);
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
            let line: DeliveryLine = serde_json::from_slice(&std::fs::read(&path)?)
                .with_context(|| format!("corrupt delivery record: {}", path.display()))?;
            if line.event != event {
                bail!("delivery event mismatch: {}", path.display());
            }
            rows.push(line);
        }
    }
    Ok(rows)
}

pub(crate) fn states(project: &Project, event: &str) -> Result<Vec<DeliveryState>> {
    Ok(delivery_lines(project, event)?
        .into_iter()
        .map(|line| line.state)
        .collect())
}

pub(crate) fn typed_line(project: &Project, event: &Event) -> Result<String> {
    match &event.payload {
        EventPayload {
            done: Some(done),
            waiting: None,
            failed: None,
        } => Ok(format!(
            "DONE {} {} commit {} · {}",
            event.thread,
            std::path::absolute(artifact_path(project, &done.artifact))?.display(),
            done.sha,
            crate::usage::summary(event.usage.as_ref())
        )),
        EventPayload {
            done: None,
            waiting: Some(waiting),
            failed: None,
        } => Ok(format!(
            "WAITING {} {}: {}",
            event.thread,
            waiting.class.plain(),
            waiting.text
        )),
        EventPayload {
            done: None,
            waiting: None,
            failed: Some(failure),
        } => Ok(format!(
            "FAILED {} {}: {}",
            event.thread,
            failure.class.plain(),
            failure.text
        )),
        _ => bail!(
            "event_payload_invalid: {} has no single tagged payload",
            event.id
        ),
    }
}

fn sync_parent(path: &std::path::Path) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub(crate) fn latest_event<'e>(
    events: &'e [Event],
    thread: &str,
    attempt: u32,
) -> Option<&'e Event> {
    events
        .iter()
        .filter(|e| e.thread == thread && e.attempt == attempt)
        .max_by(|a, b| {
            a.created
                .cmp(&b.created)
                .then_with(|| crate::ops::submission_id_order(&a.id, &b.id))
        })
}

/// The seal before a follow-up may be followed by a waiting event. Keep
/// tracking that completion until a newer completion replaces it.
pub(crate) fn latest_done_event<'e>(
    events: &'e [Event],
    thread: &str,
    attempt: u32,
) -> Option<&'e Event> {
    events
        .iter()
        .filter(|e| e.thread == thread && e.attempt == attempt && e.payload.done.is_some())
        .max_by(|a, b| {
            a.created
                .cmp(&b.created)
                .then_with(|| crate::ops::submission_id_order(&a.id, &b.id))
        })
}

pub(crate) fn checked(project: &Project) -> Result<Vec<Event>> {
    let (events, readable) = list_checked(project);
    if !readable {
        bail!("event records are unreadable");
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{DonePayload, Recipient};
    use crate::project;

    fn fixture() -> (tempfile::TempDir, Project, Event) {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let event = Event {
            id: "t-0001-1-1".into(),
            op: "t-0001-1-1".into(),
            thread: "t-0001".into(),
            attempt: 1,
            recipient: Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 1,
            },
            created: "2026-09-18T00:00:00Z".into(),
            usage: None,
            payload: EventPayload {
                done: Some(DonePayload {
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
        (root, project, event)
    }

    #[test]
    fn artifact_store_keeps_historical_names_and_never_replaces_existing_bytes() {
        let (_root, project, _) = fixture();
        let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        std::fs::create_dir_all(artifact_dir(&project)).unwrap();
        let path = artifact_path(&project, hash);
        std::fs::write(&path, b"abc").unwrap();
        let witness = project.state_dir().join("same-inode");
        std::fs::hard_link(&path, &witness).unwrap();
        assert_eq!(store_artifact(&project, b"abc").unwrap(), hash);
        assert_eq!(
            crate::thread::store_artifact(&project, b"abc").unwrap(),
            hash
        );
        assert_eq!(crate::thread::artifact(&project, hash).unwrap(), b"abc");
        // A replay must not replace the historical inode. Corrupt it through
        // the witness and require both callers to refuse, leaving it intact.
        std::fs::write(&witness, b"corrupt").unwrap();
        assert!(
            store_artifact(&project, b"abc")
                .unwrap_err()
                .to_string()
                .starts_with("artifact_conflict:")
        );
        assert!(crate::thread::store_artifact(&project, b"abc").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
    }

    #[test]
    fn concurrent_artifact_replays_publish_complete_bytes_despite_crash_leftovers() {
        let (_root, project, _) = fixture();
        let bytes = vec![b'x'; 128 * 1024];
        let hash = hash_bytes(&bytes);
        std::fs::create_dir_all(artifact_dir(&project)).unwrap();
        let stale = artifact_dir(&project).join(format!(".{hash}.{}.tmp", std::process::id()));
        std::fs::write(&stale, b"old interrupted write").unwrap();
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| store_artifact(&project, &bytes).unwrap()))
                .collect();
            for handle in handles {
                assert_eq!(handle.join().unwrap(), hash);
            }
        });
        assert_eq!(
            std::fs::read(artifact_path(&project, &hash)).unwrap(),
            bytes
        );
        assert_eq!(std::fs::read(&stale).unwrap(), b"old interrupted write");
        assert_eq!(
            std::fs::read_dir(artifact_dir(&project)).unwrap().count(),
            2
        );
    }

    #[test]
    fn historical_event_without_usage_loads_and_new_usage_is_one_optional_table() {
        // Real adeherdr event t-0058-1-1, sealed before usage existed.
        let old = r#"id = "t-0058-1-1"
op = "t-0058-1-1"
thread = "t-0058"
attempt = 1
created = "2026-09-19T22:53:42Z"
[recipient]
pane = "w1G:p1"
coordinator_attempt = 1
[payload.done]
sha = "4f93bfb9d2f103186523577957852a5d1cc4d590"
report_path = ".herdr-project/adeherdr-t-0058/report.md"
artifact = "ff2346a2702021221a52567a733cc60301ac507dc2da3c6a0629e2c6ca58f75b"
"#;
        let event: Event = toml::from_str(old).unwrap();
        assert!(event.usage.is_none());
        let (_root, project, mut event) = fixture();
        assert!(
            !String::from_utf8(bytes(&event).unwrap())
                .unwrap()
                .contains("[usage]")
        );
        event.usage = Some(crate::usage::Usage {
            input: 200000,
            cache_read: 4900000,
            total: 5100000,
            ..Default::default()
        });
        let text = String::from_utf8(bytes(&event).unwrap()).unwrap();
        assert_eq!(text.matches("[usage]").count(), 1);
        assert_eq!(toml::from_str::<Event>(&text).unwrap(), event);
        assert!(
            typed_line(&project, &event)
                .unwrap()
                .ends_with(" · 5.1M tokens (4.9M cached)")
        );
    }

    #[test]
    fn ticker_indexes_keep_resolved_evidence_and_notice_reopen_cleanup_and_new_seals() {
        let (_root, project, mut event) = fixture();
        let lane = crate::thread::allocate(&project, |t| {
            t.status = crate::thread::Status::Resolved;
            t.attempt = 1;
        })
        .unwrap();
        event.payload.done.as_mut().unwrap().artifact =
            store_artifact(&project, b"final report").unwrap();
        seal_create_if_absent(&project, &event).unwrap();
        let _cache = crate::record_cache::Cache::new();
        assert!(for_unresolved_threads(&project).is_empty());
        assert!(crate::thread::list_live(&project).is_empty());
        assert_eq!(for_thread(&project, &lane.id), vec![event.clone()]);
        assert_eq!(
            checked_for_thread(&project, &lane.id).unwrap(),
            vec![event.clone()]
        );
        assert!(crate::thread::sealed_report_path(&project, &lane).is_some());
        assert_eq!(crate::thread::list_with_errors(&project).0.len(), 1);

        crate::thread::update(&project, &lane.id, |t| t.cleanup_pending = true).unwrap();
        assert_eq!(crate::thread::list_live(&project).len(), 1);
        // Cleanup can read the seal even though delivery never replays it.
        assert!(for_unresolved_threads(&project).is_empty());
        assert_eq!(for_thread(&project, &lane.id).len(), 1);
        crate::thread::update(&project, &lane.id, |t| {
            t.cleanup_pending = false;
            t.status = crate::thread::Status::Open;
        })
        .unwrap();
        assert_eq!(for_unresolved_threads(&project), vec![event.clone()]);
        let mut newer = event.clone();
        newer.id = "t-0001-1-2".into();
        newer.created = "2026-10-01T00:00:00Z".into();
        assert_eq!(
            count_event_reads(|| seal_create_if_absent(&project, &newer).unwrap()),
            0
        );
        assert_eq!(
            count_event_reads(|| {
                let events = for_thread(&project, &lane.id);
                assert_eq!(latest_done_event(&events, &lane.id, 1), Some(&newer));
            }),
            1
        );
        assert_eq!(for_unresolved_threads(&project).len(), 2);
        std::fs::remove_file(event_path(&project, &newer.id).unwrap()).unwrap();
        assert_eq!(for_thread(&project, &lane.id), vec![event]);
    }

    #[test]
    fn ticker_event_cache_retries_corrupt_evidence_and_does_not_hide_orphans() {
        let (_root, project, event) = fixture();
        seal_create_if_absent(&project, &event).unwrap();
        let path = event_path(&project, &event.id).unwrap();
        crate::project::write_atomic(&path, b"invalid = [").unwrap();
        let cache = crate::record_cache::Cache::new();
        assert!(!list_checked(&project).1);
        assert!(checked_for_thread(&project, "t-9999").is_err());
        // In-place repair does not change the directory stamp.
        std::fs::write(&path, bytes(&event).unwrap()).unwrap();
        assert!(list_checked(&project).1);
        assert!(checked_for_thread(&project, "t-9999").unwrap().is_empty());
        assert_eq!(for_unresolved_threads(&project), vec![event.clone()]);
        // A fresh CLI read must not inherit the ticker's snapshot.
        std::fs::write(&path, b"invalid = [").unwrap();
        drop(cache);
        assert!(!list_checked(&project).1);
    }

    #[test]
    fn seal_is_create_only_and_byte_equal() {
        let (_root, project, event) = fixture();
        seal_create_if_absent(&project, &event).unwrap();
        seal_create_if_absent(&project, &event).unwrap();
        let mut changed = event.clone();
        changed.created.push('x');
        assert!(
            seal_create_if_absent(&project, &changed)
                .unwrap_err()
                .to_string()
                .contains("event_conflict")
        );
    }

    fn box_event(artifact: &str) -> Event {
        Event {
            id: "t-0001-1-1".into(),
            op: "t-0001-1-1".into(),
            thread: "t-0001".into(),
            attempt: 1,
            recipient: Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 1,
            },
            created: "2026-09-19T00:00:00Z".into(),
            usage: None,
            payload: EventPayload {
                done: Some(DonePayload {
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
        }
    }

    #[test]
    fn box_import_is_create_only_hash_checked_and_preserves_the_report_path() {
        let (_root, project, _event) = fixture();
        let report = b"report body";
        let hash = hash_bytes(report);
        let event = box_event(&hash);
        let box_bytes = bytes(&event).unwrap();

        assert_eq!(
            import_box_event(&project, "buildbox", &box_bytes, Some(report)).unwrap(),
            ImportOutcome::New
        );
        let imported = load(&project, &event.id).unwrap();
        let written = imported.payload.done.clone().unwrap().report_path;
        assert_eq!(written, ".reports/t-0001.md");
        assert_eq!(
            std::fs::read(artifact_path(&project, &hash)).unwrap(),
            report
        );
        let notice = typed_line(&project, &imported).unwrap();
        let durable = std::path::absolute(artifact_path(&project, &hash)).unwrap();
        assert!(durable.is_absolute() && durable.is_file());
        assert_eq!(
            notice,
            format!(
                "DONE t-0001 {} commit abc · usage unknown",
                durable.display()
            )
        );
        assert!(!notice.contains(".reports/t-0001.md"));

        // The exact replay is a no-op; changed bytes under the same id refuse.
        assert_eq!(
            import_box_event(&project, "buildbox", &box_bytes, Some(report)).unwrap(),
            ImportOutcome::Replay
        );
        let mut changed = event.clone();
        changed.created.push('x');
        assert!(
            import_box_event(
                &project,
                "buildbox",
                &bytes(&changed).unwrap(),
                Some(report)
            )
            .unwrap_err()
            .to_string()
            .contains("event_conflict")
        );
    }

    #[test]
    fn box_import_refuses_an_artifact_that_does_not_hash_to_its_name() {
        let (_root, project, _event) = fixture();
        let event = box_event("deadbeef");
        let box_bytes = bytes(&event).unwrap();
        assert!(
            import_box_event(&project, "buildbox", &box_bytes, Some(b"other"))
                .unwrap_err()
                .to_string()
                .contains("artifact_mismatch")
        );
    }

    #[test]
    fn remote_state_roundtrips_per_profile_and_project() {
        let (_root, project, _event) = fixture();
        let mut state = remote_state(&project, "abc");
        state.boot_id = "boot-1".into();
        state.taken.insert("t-0001-1-1".into(), "hash".into());
        state.missing.insert("t-0001".into(), 2);
        state.gone.insert("t-0001".into());
        save_remote_state(&project, "abc", &state).unwrap();
        assert_eq!(remote_state(&project, "abc"), state);
        assert!(remote_state(&project, "other").taken.is_empty());
        assert!(save_remote_state(&project, "../escape", &state).is_err());
    }

    #[test]
    fn a_completion_receipt_is_create_only_and_records_the_hashes() {
        let (_root, project, event) = fixture();
        write_receipt(&project, &event).unwrap();
        let path = receipt_path(&project, &event.id).unwrap();
        let receipt: Receipt = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(receipt.event, event.id);
        assert_eq!(receipt.event_hash, hash_bytes(&bytes(&event).unwrap()));
        assert_eq!(receipt.artifact_hash, "def");
        assert_eq!(receipt.created, event.created);
        write_receipt(&project, &event).unwrap();
        let mut changed = event.clone();
        changed.payload.done.as_mut().unwrap().artifact = "other".into();
        assert!(
            write_receipt(&project, &changed)
                .unwrap_err()
                .to_string()
                .contains("receipt_conflict")
        );
    }

    #[test]
    fn torn_delivery_tail_does_not_block_acknowledgement() {
        let (_root, project, event) = fixture();
        project.record_dir_for_write("deliveries").unwrap();
        let path = journal_path(&project, &event.id).unwrap();
        let submitted = DeliveryLine {
            event: event.id.clone(),
            state: DeliveryState::Submitted,
        };
        let old = format!(
            "{{\"event\":\"{}\",\"state\":\"queued\"}}\n{}\n{{\"event\":\"{}\",\"state\":",
            event.id,
            serde_json::to_string(&submitted).unwrap(),
            event.id
        );
        std::fs::write(&path, &old).unwrap();
        append_delivery(&project, &event.id, DeliveryState::Queued).unwrap();
        append_delivery(&project, &event.id, DeliveryState::Acknowledged).unwrap();
        append_delivery(&project, &event.id, DeliveryState::Acknowledged).unwrap();
        assert_eq!(
            states(&project, &event.id).unwrap(),
            vec![
                DeliveryState::Queued,
                DeliveryState::Submitted,
                DeliveryState::Acknowledged
            ]
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
        let bad = deliveries_dir(&project)
            .join(&event.id)
            .join("00000002.json");
        std::fs::write(&bad, b"{").unwrap();
        let error = append_delivery(&project, &event.id, DeliveryState::Handled).unwrap_err();
        assert!(format!("{error:#}").contains(&bad.display().to_string()));
    }

    #[test]
    fn interrupted_import_marker_is_rebuilt_then_replays() {
        let (_root, project, _event) = fixture();
        let report = b"report body";
        let event = box_event(&hash_bytes(report));
        let path = import_path(&project, "buildbox", &event.id).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"event = \"").unwrap();
        let box_bytes = bytes(&event).unwrap();
        assert_eq!(
            import_box_event(&project, "buildbox", &box_bytes, Some(report)).unwrap(),
            ImportOutcome::New
        );
        assert_eq!(
            import_box_event(&project, "buildbox", &box_bytes, Some(report)).unwrap(),
            ImportOutcome::Replay
        );
        assert_eq!(
            toml::from_str::<ImportSource>(&std::fs::read_to_string(&path).unwrap())
                .unwrap()
                .event_hash,
            hash_bytes(&box_bytes)
        );
    }

    #[test]
    fn delivery_journal_is_append_only_and_ack_is_idempotent() {
        let (_root, project, event) = fixture();
        append_delivery(&project, &event.id, DeliveryState::Submitted).unwrap();
        append_delivery(&project, &event.id, DeliveryState::Submitted).unwrap();
        append_delivery(&project, &event.id, DeliveryState::Acknowledged).unwrap();
        append_delivery(&project, &event.id, DeliveryState::Acknowledged).unwrap();
        let states = states(&project, &event.id).unwrap();
        assert_eq!(
            states,
            vec![
                DeliveryState::Submitted,
                DeliveryState::Submitted,
                DeliveryState::Acknowledged
            ]
        );
    }
}
