//! Ticker snapshots of atomically replaced records. CLI readers stay uncached.
//!
//! An unchanged directory costs one stat. When it changes, enumerate filenames
//! and reuse unchanged records by inode/mtime/length, rather than parsing history
//! again. Never pin a partial snapshot: repairs must be retried even in place.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use anyhow::Result;

/// Held for the ticker's lifetime, not for one pass. Drop restores fresh CLI
/// reads; tests can use the same scope without leaking snapshots between runs.
pub(crate) struct Cache;

impl Cache {
    pub(crate) fn new() -> Self {
        crate::thread::set_cache(true);
        crate::events::set_cache(true);
        crate::review::set_cache(true);
        Self
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        crate::thread::set_cache(false);
        crate::events::set_cache(false);
        crate::review::set_cache(false);
    }
}

#[derive(Clone, PartialEq)]
struct Stamp {
    inode: u64,
    len: u64,
    modified: SystemTime,
}

fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some(Stamp {
        inode: metadata.ino(),
        len: metadata.len(),
        modified: metadata.modified().ok()?,
    })
}

struct Snapshot<T> {
    stamp: Option<Stamp>,
    records: BTreeMap<String, (Option<Stamp>, T)>,
    rows: Rc<Vec<T>>,
    readable: bool,
}

pub(crate) struct Records<T> {
    directories: BTreeMap<PathBuf, Snapshot<T>>,
}

impl<T> Default for Records<T> {
    fn default() -> Self {
        Self {
            directories: BTreeMap::new(),
        }
    }
}

impl<T: Clone> Records<T> {
    pub(crate) fn read(
        &mut self,
        dir: PathBuf,
        mut load: impl FnMut(&str) -> Result<T>,
    ) -> (Rc<Vec<T>>, Vec<anyhow::Error>) {
        let current = stamp(&dir);
        if let Some(snapshot) = self.directories.get(&dir)
            && snapshot.readable
            && snapshot.stamp == current
        {
            return (snapshot.rows.clone(), Vec::new());
        }
        let mut old = self.directories.remove(&dir);
        let mut records = BTreeMap::new();
        let mut errors = Vec::new();
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(error) => {
                            errors.push(error.into());
                            continue;
                        }
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
                        errors.push(anyhow::anyhow!("record filename is not UTF-8"));
                        continue;
                    };
                    let file_stamp = stamp(&entry.path());
                    let cached = old
                        .as_mut()
                        .and_then(|snapshot| snapshot.records.remove(&id));
                    let row = match cached {
                        Some((previous, row)) if file_stamp.is_some() && previous == file_stamp => {
                            Ok(row)
                        }
                        _ => load(&id),
                    };
                    match row {
                        Ok(row) => {
                            records.insert(id, (file_stamp, row));
                        }
                        Err(error) => errors.push(error),
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => errors.push(error.into()),
        }
        let rows: Rc<Vec<T>> = Rc::new(records.values().map(|(_, row)| row.clone()).collect());
        self.directories.insert(
            dir,
            Snapshot {
                stamp: current,
                records,
                rows: rows.clone(),
                readable: errors.is_empty(),
            },
        );
        (rows, errors)
    }
}
