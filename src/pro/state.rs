//! The plugin's state files (SPEC-pro-bridge v2, "Lane lifecycle and turn
//! protocol").
//!
//! Everything is written atomically (temp, fsync, rename). Lane and turn
//! records are TOML; `usage.jsonl` is one JSON line per Pro send; the cooldown
//! file holds one RFC 3339 stamp.

use std::fs::File;
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{Layout, now_rfc3339, parse_rfc3339, seconds_since};

unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}

const LOCK_EX: i32 = 2;

/// A process lock used to serialize starts and turn admission.
pub(crate) struct FileLock {
    _file: File,
}

impl FileLock {
    pub(crate) fn acquire(path: &Path) -> Result<FileLock> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let file =
            File::create(path).with_context(|| format!("could not open {}", path.display()))?;
        // SAFETY: flock takes an open fd and a flag; it blocks until granted.
        if unsafe { flock(file.as_raw_fd(), LOCK_EX) } != 0 {
            bail!("could not lock {}", path.display());
        }
        Ok(FileLock { _file: file })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Lane {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) pane_id: String,
    #[serde(default)]
    pub(crate) tab_id: String,
    #[serde(default)]
    pub(crate) workspace_id: String,
    #[serde(default)]
    pub(crate) parent: Option<String>,
    #[serde(default)]
    pub(crate) cwd: String,
    /// The Codex config profile this lane runs; `None` is the Pro bridge lane.
    /// A picture lane is `Some("gpt-image-gen")` and never sees the bridge.
    #[serde(default)]
    pub(crate) profile: Option<String>,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) rollout: Option<String>,
    pub(crate) started_at: String,
    pub(crate) state: String,
    /// The plugin's stop switch: reconcile never prints a resume line for it.
    #[serde(default)]
    pub(crate) stopped: bool,
    #[serde(default)]
    pub(crate) last_turn: Option<String>,
}

impl Lane {
    pub(crate) fn read(layout: &Layout, name: &str) -> Result<Lane> {
        check_name(name)?;
        let path = layout.lane(name);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("no lane `{name}` ({})", path.display()))?;
        toml::from_str(&text).with_context(|| format!("{} does not parse", path.display()))
    }

    pub(crate) fn write(&self, layout: &Layout) -> Result<()> {
        check_name(&self.name)?;
        write_atomic(&layout.lane(&self.name), &self.to_toml()?)
    }

    fn to_toml(&self) -> Result<String> {
        toml::to_string(self).context("could not serialize the lane record")
    }

    pub(crate) fn list(layout: &Layout) -> Result<Vec<Lane>> {
        let mut lanes = Vec::new();
        let Ok(entries) = std::fs::read_dir(layout.lanes()) else {
            return Ok(lanes);
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".toml") else {
                continue;
            };
            lanes.push(Lane::read(layout, stem)?);
        }
        lanes.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(lanes)
    }

    /// Only `ready` accepts a turn (spec Design).
    pub(crate) fn ready(&self) -> bool {
        self.state == "ready"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Turn {
    pub(crate) tag: String,
    pub(crate) lane: String,
    pub(crate) brief: String,
    pub(crate) out: String,
    pub(crate) notify: String,
    #[serde(default)]
    pub(crate) attachments: Vec<String>,
    pub(crate) state: String,
    pub(crate) started_at: String,
    #[serde(default)]
    pub(crate) finished_at: Option<String>,
    #[serde(default)]
    pub(crate) detail: Option<String>,
    /// Typed failure class; absent on successful and historical turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failure_class: Option<String>,
    #[serde(default)]
    pub(crate) packet: Option<String>,
    /// The path the answer was actually written to (the `.<n>.md` fallback).
    #[serde(default)]
    pub(crate) written: Option<String>,
}

impl Turn {
    pub(crate) fn read(layout: &Layout, tag: &str) -> Result<Turn> {
        let path = layout.turn(tag);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("no turn `{tag}` ({})", path.display()))?;
        toml::from_str(&text).with_context(|| format!("{} does not parse", path.display()))
    }

    pub(crate) fn write(&self, layout: &Layout) -> Result<()> {
        self.write_to(&layout.turn(&self.tag))
    }

    fn write_to(&self, path: &Path) -> Result<()> {
        let text = toml::to_string(self).context("could not serialize the turn record")?;
        write_atomic(path, &text)
    }

    pub(crate) fn list(layout: &Layout) -> Result<Vec<Turn>> {
        let mut turns = Vec::new();
        let Ok(entries) = std::fs::read_dir(layout.turns()) else {
            return Ok(turns);
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".toml") else {
                continue;
            };
            if let Ok(turn) = Turn::read(layout, stem) {
                turns.push(turn);
            }
        }
        Ok(turns)
    }
}

/// Atomic write: temp file in the same directory, fsync, rename.
pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    {
        let mut file = std::fs::File::create(&tmp)
            .with_context(|| format!("could not create {}", tmp.display()))?;
        file.write_all(text.as_bytes())
            .with_context(|| format!("could not write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("could not fsync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

/// Append one line, creating the file when needed.
pub(crate) fn append_line(path: &Path, line: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("could not open {}", path.display()))?;
    writeln!(file, "{line}").with_context(|| format!("could not append to {}", path.display()))?;
    Ok(())
}

/// Record one Pro send: the state table's `usage.jsonl`.
pub(crate) fn record_usage(layout: &Layout, lane: &str, tag: &str) -> Result<()> {
    let line = serde_json::json!({
        "ts": now_rfc3339(),
        "lane": lane,
        "tag": tag,
    })
    .to_string();
    append_line(&layout.usage(), &line)
}

/// The cooldown stamp, when one is set.
pub(crate) fn cooldown_until(layout: &Layout) -> Option<jiff::Timestamp> {
    let text = std::fs::read_to_string(layout.cooldown()).ok()?;
    parse_rfc3339(&text)
}

pub(crate) fn cooldown_active(layout: &Layout, now: jiff::Timestamp) -> bool {
    cooldown_until(layout).is_some_and(|until| until > now)
}

pub(crate) fn set_cooldown(layout: &Layout, until: jiff::Timestamp) -> Result<()> {
    write_atomic(&layout.cooldown(), &until.to_string())
}

pub(crate) fn clear_cooldown(layout: &Layout) -> Result<()> {
    match std::fs::remove_file(layout.cooldown()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("could not clear the cooldown"),
    }
}

/// The bridge's last-seen identity; a changed pid means a daemon restart
/// (spec §4, the breaker).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BridgeState {
    #[serde(default)]
    pub(crate) pid: Option<u32>,
    #[serde(default)]
    pub(crate) version: Option<String>,
    #[serde(default)]
    pub(crate) accepting: Option<bool>,
}

impl BridgeState {
    pub(crate) fn read(layout: &Layout) -> BridgeState {
        std::fs::read_to_string(layout.bridge_state())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub(crate) fn write(&self, layout: &Layout) -> Result<()> {
        write_atomic(
            &layout.bridge_state(),
            &serde_json::to_string(self).context("could not serialize the bridge state")?,
        )
    }
}

/// One running turn's lock file. `prepare` creates it and the collector
/// removes it on exit. Liveness is by mtime, so a `turn` call that created the
/// lock and then spawned a short-lived process cannot be mistaken for dead (a
/// crash leaves it until [`LOCK_STALE`]).
#[derive(Debug)]
pub(crate) struct Inflight {
    path: PathBuf,
}

impl Inflight {
    pub(crate) fn create(layout: &Layout, tag: &str) -> Result<Inflight> {
        let path = layout.inflight_lock(tag);
        write_atomic(&path, "turn\n")?;
        Ok(Inflight { path })
    }

    pub(crate) fn release(self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A lock with no write for this long belongs to a turn that can no longer be
/// running (two-hour turn timeout plus slack).
const LOCK_STALE: Duration = Duration::from_secs(125 * 60);

/// The number of in-flight turns, with locks older than [`LOCK_STALE`] removed
/// first.
pub(crate) fn inflight_count(layout: &Layout) -> usize {
    let mut live = 0;
    let Ok(entries) = std::fs::read_dir(layout.inflight()) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let fresh = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age < LOCK_STALE);
        if fresh {
            live += 1;
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    live
}

/// Failed, refused or cooled turns finished within the window. Two of those in
/// ten minutes trip the breaker (spec §4).
pub(crate) fn recent_failures(layout: &Layout, now: jiff::Timestamp, window_secs: i64) -> usize {
    Turn::list(layout)
        .unwrap_or_default()
        .into_iter()
        .filter(|turn| matches!(turn.state.as_str(), "failed" | "refused" | "cooldown"))
        .filter(|turn| {
            turn.finished_at
                .as_deref()
                .and_then(|stamp| seconds_since(stamp, now))
                .is_some_and(|secs| secs >= 0 && secs <= window_secs)
        })
        .count()
}

/// A lane refuses to start or resume when the name is taken by a live lane.
pub(crate) fn name_taken(layout: &Layout, name: &str) -> bool {
    Lane::read(layout, name)
        .map(|lane| lane.state != "gone")
        .unwrap_or(false)
}

/// Two lanes must never share one Codex thread: launcher Full mode would make
/// them share a conversation key (spec, resume).
pub(crate) fn session_in_use(layout: &Layout, session: &str, except: &str) -> Option<String> {
    Lane::list(layout).ok()?.into_iter().find_map(|lane| {
        (lane.name != except && lane.state != "gone" && lane.session_id.as_deref() == Some(session))
            .then_some(lane.name)
    })
}

/// Validate a lane name: it becomes a file name and a herdr agent name.
pub(crate) fn check_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("`{name}` is not a usable lane name (letters, digits, `-` and `_`)");
    }
    Ok(())
}

/// Validate a turn id: it becomes a file name and part of the DONE line.
pub(crate) fn check_tag(tag: &str) -> Result<()> {
    if tag.is_empty()
        || !tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("`{tag}` is not a usable turn id (letters, digits, `-` and `_`)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> (tempfile::TempDir, Layout) {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        (dir, layout)
    }

    #[test]
    fn lane_round_trips_and_is_listed() {
        let (_dir, layout) = layout();
        let lane = Lane {
            name: "pro".into(),
            pane_id: "w1:p2".into(),
            tab_id: "w1:t2".into(),
            workspace_id: "w1".into(),
            parent: Some("w1:p1".into()),
            cwd: "/w".into(),
            profile: None,
            session_id: Some("abc".into()),
            rollout: Some("/r.jsonl".into()),
            started_at: now_rfc3339(),
            state: "ready".into(),
            stopped: false,
            last_turn: None,
        };
        lane.write(&layout).unwrap();
        let back = Lane::read(&layout, "pro").unwrap();
        assert_eq!(back, lane);
        assert!(back.ready());
        assert_eq!(Lane::list(&layout).unwrap().len(), 1);
        assert!(name_taken(&layout, "pro"));
        assert!(!name_taken(&layout, "other"));
    }

    #[test]
    fn turn_round_trips_and_atomic_write_leaves_no_temp() {
        let (_dir, layout) = layout();
        let turn = Turn {
            tag: "pro-01".into(),
            lane: "pro".into(),
            brief: "/b.md".into(),
            out: "/o.md".into(),
            notify: "hcoord".into(),
            attachments: vec!["/s.md".into()],
            state: "loading".into(),
            started_at: now_rfc3339(),
            finished_at: None,
            detail: None,
            failure_class: None,
            packet: Some("/p.md".into()),
            written: None,
        };
        turn.write(&layout).unwrap();
        assert_eq!(Turn::read(&layout, "pro-01").unwrap(), turn);
        let leftovers: Vec<_> = std::fs::read_dir(layout.turns())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn cooldown_expires() {
        let (_dir, layout) = layout();
        let now = jiff::Timestamp::now();
        assert!(!cooldown_active(&layout, now));
        set_cooldown(&layout, now + jiff::SignedDuration::from_secs(60)).unwrap();
        assert!(cooldown_active(&layout, now));
        set_cooldown(&layout, now - jiff::SignedDuration::from_secs(60)).unwrap();
        assert!(!cooldown_active(&layout, now));
        clear_cooldown(&layout).unwrap();
        assert!(!layout.cooldown().exists());
    }

    #[test]
    fn inflight_counts_a_fresh_lock_and_cleans_a_stale_one() {
        let (_dir, layout) = layout();
        let live = Inflight::create(&layout, "pro-01").unwrap();
        assert_eq!(inflight_count(&layout), 1);
        live.release();
        assert_eq!(inflight_count(&layout), 0);
        // A lock older than the stale window is removed, not counted.
        let stale = layout.inflight_lock("stale");
        std::fs::write(&stale, "turn\n").unwrap();
        let old = std::time::SystemTime::now() - LOCK_STALE - Duration::from_secs(60);
        let file = std::fs::File::options().write(true).open(&stale).unwrap();
        file.set_modified(old).unwrap();
        assert_eq!(inflight_count(&layout), 0);
        assert!(!stale.exists());
    }

    #[test]
    fn recent_failures_counts_only_the_window() {
        let (_dir, layout) = layout();
        let now = jiff::Timestamp::now();
        for (tag, secs, state) in [
            ("a", 5, "failed"),
            ("b", 60, "cooldown"),
            ("c", 3600, "failed"),
            ("d", 5, "delivered"),
        ] {
            let stamp = (now - jiff::SignedDuration::from_secs(secs)).to_string();
            let turn = Turn {
                tag: tag.into(),
                lane: "pro".into(),
                brief: "/b".into(),
                out: "/o".into(),
                notify: "n".into(),
                attachments: vec![],
                state: state.into(),
                started_at: stamp.clone(),
                finished_at: Some(stamp),
                detail: None,
                failure_class: None,
                packet: None,
                written: None,
            };
            turn.write(&layout).unwrap();
        }
        assert_eq!(recent_failures(&layout, now, 600), 2);
    }

    #[test]
    fn session_in_use_finds_the_other_lane() {
        let (_dir, layout) = layout();
        for (name, session) in [("pro", "abc"), ("other", "xyz")] {
            Lane {
                name: name.into(),
                pane_id: "p".into(),
                tab_id: "t".into(),
                workspace_id: "w".into(),
                parent: None,
                cwd: "/w".into(),
                profile: None,
                session_id: Some(session.into()),
                rollout: None,
                started_at: now_rfc3339(),
                state: "ready".into(),
                stopped: false,
                last_turn: None,
            }
            .write(&layout)
            .unwrap();
        }
        assert_eq!(session_in_use(&layout, "abc", "pro"), None);
        assert_eq!(session_in_use(&layout, "abc", "other"), Some("pro".into()));
    }
}
