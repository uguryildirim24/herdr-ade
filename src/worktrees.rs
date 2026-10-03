//! Safety checks for removing lane and review worktrees.
//!
//! Git does not consider ignored files when deciding whether a worktree is
//! clean enough to remove. ADE does: only ignored paths covered by the
//! global or repository-specific disposable lists may be discarded.

use std::collections::BTreeSet;
use std::path::{Component, Path};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::project::Project;
use crate::runner::Runner;

const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

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

pub(crate) fn human_size(bytes: u64) -> String {
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

fn validate_disposable(raw_paths: impl IntoIterator<Item = String>) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for raw in raw_paths {
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

fn same_repo(left: &str, right: &str) -> bool {
    let canonical =
        |path: &str| std::fs::canonicalize(path).unwrap_or_else(|_| Path::new(path).to_path_buf());
    canonical(left) == canonical(right)
}

/// Global disposable paths plus the paths owned by this project's repository
/// row or, for a harness repository, its config row.
pub(crate) fn disposable(config_dir: &Path, project: &Project, repo: &str) -> Result<Vec<String>> {
    let (settings, _) = project.read_project_md()?;
    disposable_for_rows(config_dir, repo, &settings.repos)
}

pub(crate) fn disposable_for_rows(
    config_dir: &Path,
    repo: &str,
    rows: &[crate::project::Repo],
) -> Result<Vec<String>> {
    let document = crate::config::Document::read(config_dir)?;
    let mut paths = document.section::<WorktreeConfig>("worktrees")?.disposable;
    paths.extend(
        rows.iter()
            .filter(|row| same_repo(&row.path, repo))
            .flat_map(|row| row.disposable.iter().cloned()),
    );
    paths.extend(
        crate::harness::repos_from(&document)?
            .into_iter()
            .filter(|row| same_repo(&row.path, repo))
            .flat_map(|row| row.disposable),
    );
    validate_disposable(paths)
}

fn parse_status(text: &str) -> (Vec<String>, Vec<String>) {
    let mut dirty = Vec::new();
    let mut ignored = Vec::new();
    let mut records = text.split('\0');
    while let Some(record) = records.next() {
        if record.len() <= 3 {
            continue;
        }
        if record.as_bytes()[..2]
            .iter()
            .any(|b| matches!(b, b'R' | b'C'))
        {
            // In porcelain -z, the destination is followed by the original path.
            records.next();
        }
        let path = record[3..].to_string();
        if &record[..2] == "!!" {
            ignored.push(path);
        } else {
            dirty.push(path);
        }
    }
    (dirty, ignored)
}

fn component_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut pattern_at, mut value_at) = (0, 0);
    let (mut star_at, mut star_value_at) = (None, 0);
    while value_at < value.len() {
        if pattern_at < pattern.len() && pattern[pattern_at] == value[value_at] {
            pattern_at += 1;
            value_at += 1;
        } else if pattern_at < pattern.len() && pattern[pattern_at] == '*' {
            star_at = Some(pattern_at);
            pattern_at += 1;
            star_value_at = value_at;
        } else if let Some(star) = star_at {
            star_value_at += 1;
            value_at = star_value_at;
            pattern_at = star + 1;
        } else {
            return false;
        }
    }
    pattern[pattern_at..].iter().all(|part| *part == '*')
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
        let pattern: Vec<_> = entry.split('/').collect();
        if pattern.len() == 1 {
            components
                .iter()
                .any(|value| component_matches(pattern[0], value))
        } else {
            pattern.len() <= components.len()
                && pattern
                    .iter()
                    .zip(&components)
                    .all(|(pattern, value)| component_matches(pattern, value))
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

fn roots(
    ignored: Vec<String>,
    nested: Vec<String>,
    disposable: &[String],
    report_artifact_stored: bool,
) -> BTreeSet<String> {
    let mut roots: BTreeSet<String> = ignored
        .into_iter()
        .filter(|path| {
            !disposable_path(path, disposable)
                && !(report_artifact_stored
                    && (path == ".reports" || path.starts_with(".reports/")))
        })
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
    disposable: &[String],
    report_artifact_stored: bool,
) -> Result<Inspection> {
    let text = crate::git::worktree_status_with_ignored(runner, repo, path)?;
    inspect_local_status(&text, path, disposable, report_artifact_stored)
}

/// Decode a status captured in doctor's parallel git batch. The same
/// classification and kept-data walk is used by explicit removal.
pub(crate) fn inspect_local_status(
    text: &str,
    path: &str,
    disposable: &[String],
    report_artifact_stored: bool,
) -> Result<Inspection> {
    let (dirty, ignored) = parse_status(text);
    let nested = nested_worktrees(Path::new(path))?;
    let ignored_data = roots(ignored, nested, disposable, report_artifact_stored)
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
    machine_path: &str,
    path: &str,
    disposable: &[String],
    report_artifact_stored: bool,
) -> Result<Inspection> {
    const MARKER: &str = "__HERDR_NESTED_WORKTREES__";
    let quoted = crate::remote::quote(path);
    // NUL framing cannot collide with a status path: every porcelain record
    // starts with its two-byte status and a space, and paths cannot contain NUL.
    let script = crate::remote::with_path(
        machine_path,
        &format!(
            "cd {quoted} && git status --porcelain --ignored --untracked-files=all -z && printf '\\0{MARKER}\\0' && find . -mindepth 2 -name .git -print0"
        ),
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
    let mut ignored_data = Vec::new();
    for relative in roots(ignored, nested, disposable, report_artifact_stored) {
        let full = format!("{}/{}", path.trim_end_matches('/'), relative);
        let script = crate::remote::with_path(
            machine_path,
            &format!("du -sk -- {}", crate::remote::quote(&full)),
        );
        let out = crate::remote::ssh(runner, target, &script, None, CHECK_TIMEOUT)?;
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
    fn first_unstaged_status_path_reaches_local_and_box_inspection_verbatim() {
        use crate::runner::fake::{FakeRunner, ok};

        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_string_lossy();
        let status = " M README.md\0 M scripts/circuit_tour.py\0";
        let runner = FakeRunner::new();
        runner.on("status --porcelain", ok(status));
        let local = inspect_local(&runner, &path, &path, &[], false).unwrap();
        assert_eq!(local.dirty, ["README.md", "scripts/circuit_tour.py"]);
        let box_runner = FakeRunner::new();
        box_runner.on(
            "status --porcelain",
            ok(&format!("{status}\0__HERDR_NESTED_WORKTREES__\0")),
        );
        let boxed = inspect_remote(&box_runner, "box", "/bin", &path, &[], false).unwrap();
        assert_eq!(boxed.dirty, local.dirty);
    }

    #[test]
    fn nul_status_consumes_rename_and_copy_original_paths() {
        for status in ["R ", " R", "C ", " C", "RM", "MC"] {
            let text =
                format!("{status} new name\0old name\0 M modified\0?? untracked\0!! ignored\0");
            let (dirty, ignored) = parse_status(&text);
            assert_eq!(dirty, ["new name", "modified", "untracked"], "{status}");
            assert_eq!(ignored, ["ignored"], "{status}");
        }
    }

    #[test]
    fn nul_status_keeps_paths_verbatim_and_consumes_short_original_paths() {
        let (dirty, ignored) = parse_status("R  new\nname \0x\0C  copied\0 M old\0!! cache \0");
        assert_eq!(dirty, ["new\nname ", "copied"]);
        assert_eq!(ignored, ["cache "]);
    }

    #[test]
    fn a_disposable_name_matches_that_path_component_only() {
        let configured = vec!["target".to_string(), "web/vendor".to_string()];
        assert!(disposable_path("crate/target/debug/a", &configured));
        assert!(disposable_path("web/vendor/a", &configured));
        assert!(!disposable_path("targets/a", &configured));
        assert!(!disposable_path("other/vendor/a", &configured));
    }

    #[test]
    fn wildcard_matches_inside_one_path_part_only() {
        let configured = vec!["runs/pytest-*".to_string()];
        assert!(disposable_path("runs/pytest-x/output.bin", &configured));
        assert!(!disposable_path("runs/seed-1/output.bin", &configured));
        assert!(component_matches("a*a", "aaa"));
        assert!(!disposable_path(
            "other/runs/pytest-x/output.bin",
            &configured
        ));

        assert!(
            roots(
                vec!["runs/pytest-x/output.bin".into()],
                vec![],
                &configured,
                false
            )
            .is_empty()
        );
        assert_eq!(
            roots(
                vec!["runs/pytest-x/output.bin".into()],
                vec!["runs/pytest-x/nested".into()],
                &configured,
                false
            ),
            BTreeSet::from(["runs/pytest-x/nested".to_string()]),
            "a wildcard must never make a nested checkout disposable"
        );
        assert_eq!(
            roots(
                vec![
                    "runs/pytest-x/output.bin".into(),
                    "runs/seed-1/output.bin".into()
                ],
                vec![],
                &configured,
                false
            ),
            BTreeSet::from(["runs".to_string()])
        );
    }

    #[test]
    fn an_artifact_backed_reports_folder_is_disposable_but_nested_work_stays() {
        assert!(roots(vec![".reports/t-0001.md".into()], vec![], &[], true).is_empty());
        assert_eq!(
            roots(
                vec![".reports/t-0001.md".into(), "runs/raw.bin".into()],
                vec![],
                &[],
                true
            ),
            BTreeSet::from(["runs".to_string()])
        );
        assert_eq!(
            roots(
                vec![".reports/t-0001.md".into()],
                vec![".reports/nested".into()],
                &[],
                true
            ),
            BTreeSet::from([".reports/nested".to_string()])
        );
    }

    #[test]
    fn a_project_repository_list_does_not_apply_to_another_repository() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("config.toml"), "").unwrap();
        let project = crate::project::create(
            &home.path().join("root"),
            "demo",
            "",
            vec![
                crate::project::Repo {
                    path: "/one".into(),
                    disposable: vec!["runs/pytest-*".into()],
                    ..crate::project::Repo::default()
                },
                crate::project::Repo {
                    path: "/two".into(),
                    ..crate::project::Repo::default()
                },
            ],
        )
        .unwrap();

        assert_eq!(
            disposable(&config, &project, "/one").unwrap(),
            ["runs/pytest-*".to_string()]
        );
        assert!(disposable(&config, &project, "/two").unwrap().is_empty());
    }

    #[test]
    fn absolute_disposable_path_is_rejected_instead_of_broadened() {
        assert!(
            validate_disposable(["/target".to_string()])
                .unwrap_err()
                .to_string()
                .contains("must be a relative path name")
        );
    }

    #[test]
    fn remote_status_marker_filename_is_still_kept_as_data() {
        use crate::runner::fake::{FakeRunner, ok};

        let runner = FakeRunner::new();
        runner.on("du -sk", ok("4\t/wt/__HERDR_NESTED_WORKTREES__\n"));
        runner.on(
            "status --porcelain --ignored --untracked-files=all -z",
            ok("!! __HERDR_NESTED_WORKTREES__\0\0__HERDR_NESTED_WORKTREES__\0"),
        );

        let inspection =
            inspect_remote(&runner, "box", "/custom/bin:/bin", "/wt", &[], false).unwrap();

        assert_eq!(
            inspection.ignored_data,
            vec![DataPath {
                path: "__HERDR_NESTED_WORKTREES__".into(),
                bytes: 4096,
            }]
        );
    }
}
