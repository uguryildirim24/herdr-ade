//! Safety checks for removing lane and review worktrees.
//!
//! Git does not consider ignored files when deciding whether a worktree is
//! clean enough to remove. ADE does: only ignored paths covered by the
//! editable `[worktrees].disposable` list may be discarded.

use std::collections::BTreeSet;
use std::path::{Component, Path};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::runner::Runner;

const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    worktrees: WorktreeConfig,
}

#[derive(Debug, Default, Deserialize)]
struct WorktreeConfig {
    #[serde(default)]
    disposable: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataPath {
    pub(crate) path: String,
    pub(crate) bytes: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Inspection {
    pub(crate) dirty: Vec<String>,
    pub(crate) ignored_data: Vec<DataPath>,
}

impl Inspection {
    pub(crate) fn ignored_reason(&self, worktree: &str) -> Option<String> {
        (!self.ignored_data.is_empty()).then(|| {
            format!(
                "ignored_data: data in {worktree}; not removing ({})",
                describe_data(&self.ignored_data)
            )
        })
    }
}

pub(crate) fn describe_data(paths: &[DataPath]) -> String {
    paths
        .iter()
        .map(|entry| format!("{}: {}", entry.path, human_size(entry.bytes)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn disposable(config_dir: &Path) -> Result<Vec<String>> {
    let file = config_dir.join("config.toml");
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", file.display()));
        }
    };
    let config: RawConfig =
        toml::from_str(&text).with_context(|| format!("{} does not parse", file.display()))?;
    let mut paths = Vec::new();
    for raw in config.worktrees.disposable {
        let path = raw.trim_end_matches('/');
        if path.is_empty()
            || raw.starts_with('/')
            || Path::new(path).is_absolute()
            || Path::new(path)
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            bail!("worktrees_disposable_invalid: `{raw}` must be a relative path name");
        }
        paths.push(path.to_string());
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn parse_status(text: &str) -> (Vec<String>, Vec<String>) {
    let mut dirty = Vec::new();
    let mut ignored = Vec::new();
    let nul_delimited = text.contains('\0');
    let records: Box<dyn Iterator<Item = &str>> = if nul_delimited {
        Box::new(text.split('\0'))
    } else {
        // Scripted tests written before status used `-z` still use lines.
        Box::new(text.lines())
    };
    for record in records.filter(|record| record.len() > 3) {
        let path = if nul_delimited {
            record[3..].to_string()
        } else {
            record[3..].trim().trim_matches('"').replace("\\\"", "\"")
        };
        if &record[..2] == "!!" {
            ignored.push(path);
        } else {
            dirty.push(path);
        }
    }
    (dirty, ignored)
}

fn disposable_path(path: &str, configured: &[String]) -> bool {
    let components: Vec<_> = Path::new(path)
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    configured.iter().any(|entry| {
        if entry.contains('/') {
            path == entry || path.starts_with(&format!("{entry}/"))
        } else {
            components.contains(&entry.as_str())
        }
    })
}

fn data_root(path: &str) -> String {
    Path::new(path)
        .components()
        .find_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .unwrap_or_else(|| path.to_string())
}

fn nested_worktrees(root: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("could not inspect {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if entry.file_name() == ".git" && dir != root {
                if kind.is_dir() || kind.is_file() {
                    found.push(
                        dir.strip_prefix(root)
                            .unwrap_or(&dir)
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                continue;
            }
            if kind.is_dir() && entry.file_name() != ".git" {
                pending.push(path);
            }
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

fn path_size(path: &Path) -> Result<u64> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("could not measure {}", path.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(metadata.len());
    }
    let mut total = 0u64;
    for entry in
        std::fs::read_dir(path).with_context(|| format!("could not measure {}", path.display()))?
    {
        total = total.saturating_add(path_size(&entry?.path())?);
    }
    Ok(total)
}

fn roots(ignored: Vec<String>, nested: Vec<String>, disposable: &[String]) -> BTreeSet<String> {
    let mut roots: BTreeSet<String> = ignored
        .into_iter()
        .filter(|path| !disposable_path(path, disposable))
        .map(|path| data_root(&path))
        .collect();
    // A nested checkout is durable data even when an enclosing path was
    // explicitly listed as disposable.
    roots.extend(nested);
    // Do not print a nested path twice when an already-kept parent covers it.
    let snapshot: Vec<_> = roots.iter().cloned().collect();
    roots.retain(|path| {
        !snapshot
            .iter()
            .any(|other| other != path && path.starts_with(&format!("{other}/")))
    });
    roots
}

pub(crate) fn inspect_local(
    runner: &dyn Runner,
    repo: &str,
    path: &str,
    config_dir: &Path,
) -> Result<Inspection> {
    let text = crate::git::worktree_status_with_ignored(runner, repo, path)?;
    let (dirty, ignored) = parse_status(&text);
    let nested = nested_worktrees(Path::new(path))?;
    let disposable = disposable(config_dir)?;
    let ignored_data = roots(ignored, nested, &disposable)
        .into_iter()
        .map(|relative| {
            let bytes = path_size(&Path::new(path).join(&relative))?;
            Ok(DataPath {
                path: relative,
                bytes,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Inspection {
        dirty,
        ignored_data,
    })
}

pub(crate) fn inspect_remote(
    runner: &dyn Runner,
    target: &str,
    path: &str,
    config_dir: &Path,
) -> Result<Inspection> {
    const MARKER: &str = "__HERDR_NESTED_WORKTREES__";
    let quoted = crate::remote::quote(path);
    // NUL framing cannot collide with a status path: every porcelain record
    // starts with its two-byte status and a space, and paths cannot contain NUL.
    let script = format!(
        "cd {quoted} && git status --porcelain --ignored --untracked-files=all -z && printf '\\0{MARKER}\\0' && find . -mindepth 2 -name .git -print0"
    );
    let out = crate::remote::ssh(runner, target, &script, None, CHECK_TIMEOUT)?;
    if !out.success() {
        bail!("worktree_status_failed: {path}: {}", out.error_text());
    }
    let delimiter = format!("\0{MARKER}\0");
    let (status, nested_text) = out
        .stdout
        .split_once(&delimiter)
        .context("worktree inspection output was incomplete")?;
    let (dirty, ignored) = parse_status(status);
    let nested = nested_text
        .split('\0')
        .filter_map(|entry| entry.strip_prefix("./"))
        .filter_map(|entry| entry.strip_suffix("/.git"))
        .map(str::to_string)
        .collect();
    let disposable = disposable(config_dir)?;
    let mut ignored_data = Vec::new();
    for relative in roots(ignored, nested, &disposable) {
        let full = format!("{}/{}", path.trim_end_matches('/'), relative);
        let out = crate::remote::ssh(
            runner,
            target,
            &format!("du -sk -- {}", crate::remote::quote(&full)),
            None,
            CHECK_TIMEOUT,
        )?;
        if !out.success() {
            bail!("could not measure {full}: {}", out.error_text());
        }
        let kib = out
            .stdout
            .split_whitespace()
            .next()
            .context("du printed no size")?
            .parse::<u64>()
            .context("du printed an invalid size")?;
        ignored_data.push(DataPath {
            path: relative,
            bytes: kib.saturating_mul(1024),
        });
    }
    Ok(Inspection {
        dirty,
        ignored_data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disposable_name_matches_that_path_component_only() {
        let configured = vec!["target".to_string(), "web/vendor".to_string()];
        assert!(disposable_path("crate/target/debug/a", &configured));
        assert!(disposable_path("web/vendor/a", &configured));
        assert!(!disposable_path("targets/a", &configured));
        assert!(!disposable_path("other/vendor/a", &configured));
    }

    #[test]
    fn absent_table_has_no_disposable_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[routing]\ndefault = 'x'\n").unwrap();
        assert!(disposable(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn absolute_disposable_path_is_rejected_instead_of_broadened() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[worktrees]\ndisposable = ['/target']\n",
        )
        .unwrap();
        assert!(
            disposable(dir.path())
                .unwrap_err()
                .to_string()
                .contains("must be a relative path name")
        );
    }

    #[test]
    fn remote_status_marker_filename_is_still_kept_as_data() {
        use crate::runner::fake::{FakeRunner, ok};

        let config = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new();
        runner.on("du -sk", ok("4\t/wt/__HERDR_NESTED_WORKTREES__\n"));
        runner.on(
            "status --porcelain --ignored --untracked-files=all -z",
            ok("!! __HERDR_NESTED_WORKTREES__\0\0__HERDR_NESTED_WORKTREES__\0"),
        );

        let inspection = inspect_remote(&runner, "box", "/wt", config.path()).unwrap();

        assert_eq!(
            inspection.ignored_data,
            vec![DataPath {
                path: "__HERDR_NESTED_WORKTREES__".into(),
                bytes: 4096,
            }]
        );
    }
}
