//! Immutable completion events, their append-only delivery journals, and the
//! Mac-side import of box envelopes (SPEC-remote §4.3).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contracts::{DeliveryLine, DeliveryState, Event, EventPayload};
use crate::project::{self, Project};

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

fn load_import(project: &Project, machine: &str, event: &str) -> Option<ImportSource> {
    let path = import_path(project, machine, event).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
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
    if let Some(existing) = load_import(project, machine, &event.id) {
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
        write_artifact_create_only(project, &done.artifact, bytes)?;
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
    let mut file = match OpenOptions::new().create_new(true).write(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let actual = std::fs::read_to_string(&path).unwrap_or_default();
            let expected = String::from_utf8_lossy(text.as_bytes()).into_owned();
            if actual == expected {
                return Ok(());
            }
            bail!(
                "import_conflict: {} already exists with different bytes",
                source.event
            )
        }
        Err(error) => {
            return Err(error).with_context(|| format!("could not write {}", path.display()));
        }
    };
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// Content-addressed artifact write; a retry with the same bytes is a no-op.
fn write_artifact_create_only(project: &Project, hash: &str, bytes: &[u8]) -> Result<()> {
    let dir = artifact_dir(project);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(hash);
    if path.exists() {
        if std::fs::read(&path)? == bytes {
            return Ok(());
        }
        bail!("artifact_conflict: {} has different bytes", path.display());
    }
    let tmp = dir.join(format!(".{hash}.{}.tmp", std::process::id()));
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    match std::fs::rename(&tmp, &path) {
        Ok(()) => {}
        Err(error) if path.exists() && std::fs::read(&path)? == bytes => {
            let _ = std::fs::remove_file(&tmp);
            let _ = error;
        }
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(error.into());
        }
    }
    File::open(&dir)?.sync_all()?;
    Ok(())
}

/// Store report bytes under their own hash and return that artifact name.
pub(crate) fn store_artifact(project: &Project, bytes: &[u8]) -> Result<String> {
    let hash = hash_bytes(bytes);
    write_artifact_create_only(project, &hash, bytes)?;
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

fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
    // Written whole beside the target, then linked into place: a reader never
    // sees a half-written event (the dot name is skipped by every lister).
    let tmp = dir(project).join(format!(".{}.{}.tmp", event.id, std::process::id()));
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&expected)?;
        file.sync_all()?;
    }
    let linked = std::fs::hard_link(&tmp, &path);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => {
            sync_parent(&path)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let actual = std::fs::read(&path)
                .with_context(|| format!("could not read sealed event {}", path.display()))?;
            if actual == expected {
                Ok(())
            } else {
                bail!(
                    "event_conflict: sealed event {} has different bytes",
                    event.id
                )
            }
        }
        Err(error) => Err(error).with_context(|| format!("could not seal {}", path.display())),
    }
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
    let mut file = match OpenOptions::new().create_new(true).write(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let actual = std::fs::read_to_string(&path).unwrap_or_default();
            if actual == text {
                return Ok(());
            }
            bail!(
                "receipt_conflict: {} already has different bytes",
                path.display()
            );
        }
        Err(error) => {
            return Err(error).with_context(|| format!("could not write {}", path.display()));
        }
    };
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    sync_parent(&path)
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
        if !entry.path().extension().is_some_and(|ext| ext == "toml") {
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
    cached_events(project, Some(thread))
        .map(|(events, _)| events)
        .unwrap_or_else(|| {
            list(project)
                .into_iter()
                .filter(|event| event.thread == thread)
                .collect()
        })
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
    if state != DeliveryState::Submitted && states(project, event)?.contains(&state) {
        return Ok(());
    }
    let path = journal_path(project, event)?;
    let line = DeliveryLine {
        event: event.to_string(),
        state,
    };
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    serde_json::to_writer(&mut file, &line)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    sync_parent(&path)
}

fn delivery_lines(project: &Project, event: &str) -> Result<Vec<DeliveryLine>> {
    let path = journal_path(project, event)?;
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).context("delivery journal line does not parse"))
        .collect()
}

pub(crate) fn states(project: &Project, event: &str) -> Result<Vec<DeliveryState>> {
    Ok(delivery_lines(project, event)?
        .into_iter()
        .map(|line| line.state)
        .collect())
}

pub(crate) fn typed_line(event: &Event) -> Result<String> {
    match &event.payload {
        EventPayload {
            done: Some(done),
            waiting: None,
            failed: None,
        } => Ok(format!(
            "DONE {} {} {}",
            event.thread, done.report_path, done.sha
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
        .max_by(|a, b| (&a.created, &a.id).cmp(&(&b.created, &b.id)))
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
        .max_by(|a, b| (&a.created, &a.id).cmp(&(&b.created, &b.id)))
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
        // In-place repair does not change the directory stamp.
        std::fs::write(&path, bytes(&event).unwrap()).unwrap();
        assert!(list_checked(&project).1);
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
        let hash = format!("{:x}", Sha256::digest(report));
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
