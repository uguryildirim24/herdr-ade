//! Repository lock and git helpers (SPEC-ADE D4, D6, D9, item 34).
//!
//! Lock order: the project lock is never held while this lock is acquired.
//! Git runs outside the project lock. Public helpers stay for A2/A3 merge
//! and for tests that do not call every path from this crate's binary.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::repo::Git;
use crate::runner::Runner;

const GIT_TIMEOUT: Duration = Duration::from_secs(20);
const RECOVERED_INDEX: &str = "herdr-ade-recovered-index";

/// Held while worktree add/remove, `info/exclude` edits, and plugin ref writes
/// run. Keyed by `git rev-parse --git-common-dir`.
pub(crate) struct RepoLock {
    _file: File,
    #[allow(dead_code)]
    pub(crate) common_dir: PathBuf,
}

/// Snapshot the repository without invoking git again: the timed-out git may
/// itself be blocking on repository state. A worktree's .git is a pointer to
/// its private gitdir; commondir then leads to the shared locks.
pub(crate) fn repo_activity(repo: &str) -> String {
    let dotgit = Path::new(repo).join(".git");
    let gitdir = if dotgit.is_file() {
        std::fs::read_to_string(&dotgit)
            .ok()
            .and_then(|text| text.trim().strip_prefix("gitdir: ").map(str::to_string))
            .map(|path| {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    path
                } else {
                    Path::new(repo).join(path)
                }
            })
    } else if dotgit.is_dir() {
        Some(dotgit)
    } else {
        None
    };
    let mut evidence = Vec::new();
    if let Some(dir) = gitdir {
        let common = std::fs::read_to_string(dir.join("commondir"))
            .ok()
            .map(|path| dir.join(path.trim()))
            .unwrap_or_else(|| dir.clone());
        for location in [&dir, &common] {
            for name in [
                "index.lock",
                "packed-refs.lock",
                "gc.pid",
                "HEAD.lock",
                "config.lock",
                "shallow.lock",
            ] {
                let path = location.join(name);
                if path.exists() {
                    evidence.push(path.display().to_string());
                }
            }
        }
    }
    evidence.sort();
    evidence.dedup();
    let locks = if evidence.is_empty() {
        "no lock markers found".to_string()
    } else {
        format!("lock markers: {}", evidence.join(", "))
    };
    let processes = std::process::Command::new("ps")
        .args(["-eo", "pid=,args="])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|line| {
                    line.contains(repo) && (line.contains("git ") || line.contains("git-"))
                })
                .take(8)
                .map(str::trim)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|lines| !lines.is_empty())
        .unwrap_or_else(|| "no other git processes observed".to_string());
    format!("{locks}; processes: {processes}")
}

/// Absolute `git-common-dir` for `repo`.
pub(crate) fn common_dir(runner: &dyn Runner, repo: &str) -> Result<PathBuf> {
    let raw = Git::new(runner, repo)
        .with_timeout(Duration::from_secs(5))
        .run(&["rev-parse", "--git-common-dir"])?;
    let path = PathBuf::from(&raw);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(Path::new(repo).join(path))
    }
}

/// Exclusive lock for one repository. The lock file lives in the common dir.
pub(crate) fn lock(runner: &dyn Runner, repo: &str) -> Result<RepoLock> {
    let common = common_dir(runner, repo)?;
    std::fs::create_dir_all(&common)
        .with_context(|| format!("could not create {}", common.display()))?;
    let path = common.join("herdr-ade.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open repository lock {}", path.display()))?;
    file.lock()?;
    Ok(RepoLock {
        _file: file,
        common_dir: common,
    })
}

/// Callers first apply ADE's stricter ignored-data/preservation checks. Keep
/// administration until checkout deletion succeeds: Git's `worktree remove`
/// deletes it even when a permission error leaves checkout files behind.
pub(crate) fn worktree_remove(runner: &dyn Runner, repo: &str, path: &str) -> Result<()> {
    let lock = lock(runner, repo)?;
    let pointer = Path::new(path).join(".git");
    let text = std::fs::read_to_string(&pointer)?;
    let raw = text
        .trim_end_matches('\n')
        .strip_prefix("gitdir: ")
        .context("worktree Git pointer missing")?;
    let admin = Path::new(raw);
    let common = std::fs::canonicalize(&lock.common_dir)?;
    anyhow::ensure!(
        admin.parent() == Some(common.join("worktrees").as_path()),
        "worktree is not registered in {repo}: {path}"
    );
    anyhow::ensure!(
        Path::new(std::fs::read_to_string(admin.join("gitdir"))?.trim_end_matches('\n')) == pointer,
        "worktree backlink mismatch: {path}"
    );
    anyhow::ensure!(!admin.join("locked").exists(), "worktree is locked: {path}");
    let recovered = recovered_index(admin)?;
    if let Some(sealed) = &recovered {
        anyhow::ensure!(
            rev_parse(runner, path, "HEAD")? == *sealed,
            "recovered worktree moved beyond its seal"
        );
    }
    with_worktree_status(runner, repo, path, |stream| {
        let mut row = Vec::new();
        loop {
            row.clear();
            if stream.read_until(0, &mut row)? == 0 {
                break;
            }
            anyhow::ensure!(row.last() == Some(&0), "incomplete Git status record");
            anyhow::ensure!(
                row.starts_with(b"!! ") || recovered.is_some() && row.starts_with(b" D "),
                "not a clean worktree: {path}"
            );
        }
        Ok(())
    })?;
    // Detect unwritable wall evidence before deleting tracked files. Retain
    // both checkout and index on permission refusal, not a half-removed lane.
    let mut pending = vec![PathBuf::from(path)];
    while let Some(dir) = pending.pop() {
        let _probe = tempfile::tempfile_in(&dir)?;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_name() != ".git" && entry.file_type()?.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    std::fs::remove_file(pointer)?;
    std::fs::remove_dir(path)?;
    std::fs::remove_dir_all(admin)?;
    Ok(())
}

/// Forget only the caller's absent checkout. A global prune cannot distinguish
/// a deleted checkout from another lane hidden by a mount namespace.
pub(crate) fn forget_worktree(runner: &dyn Runner, repo: &str, path: &str) -> Result<()> {
    anyhow::ensure!(
        std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "worktree still exists: {path}"
    );
    let lock = lock(runner, repo)?;
    let entries = match std::fs::read_dir(lock.common_dir.join("worktrees")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let pointer = Path::new(path).join(".git");
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let gitdir = std::fs::read_to_string(entry.path().join("gitdir"))?;
        if Path::new(gitdir.trim_end_matches('\n')) == pointer {
            anyhow::ensure!(
                !entry.path().join("locked").exists(),
                "worktree is locked: {path}"
            );
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

/// Rebuild lost administration from a sealed branch without changing checkout
/// files. The reconstructed index is only a comparison baseline; dirty files
/// and unique commits still face the normal removal safety checks.
fn recovered_index(admin: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(admin.join(RECOVERED_INDEX)) {
        Ok(sha) => Ok(Some(sha.trim_end_matches('\n').into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn repair_worktree(
    runner: &dyn Runner,
    repo: &str,
    path: &str,
    branch: &str,
    sealed: &str,
) -> Result<bool> {
    let pointer = Path::new(path).join(".git");
    if pointer.is_dir() {
        return Ok(false);
    }
    let raw = match std::fs::read_to_string(&pointer) {
        Ok(text) => Some(PathBuf::from(
            text.trim_end_matches('\n')
                .strip_prefix("gitdir: ")
                .context("worktree_metadata_invalid: Git pointer")?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(dir) = raw.as_ref().filter(|dir| dir.is_dir()) {
        if let Some(baseline) = recovered_index(dir)? {
            anyhow::ensure!(
                !sealed.is_empty()
                    && baseline == sealed
                    && Git::new(runner, repo).branch_head(branch)?.as_deref() == Some(sealed),
                "worktree_metadata_missing: recovered branch moved beyond retained seal; checkout kept for a decision"
            );
            return Ok(true);
        }
        return Ok(false);
    }
    if raw.is_none()
        && Path::new(path).parent() != Some(Path::new(repo).join(".worktrees").as_path())
    {
        // Do not infer administration for an arbitrary unregistered folder.
        // Its ordinary status check remains authoritative.
        return Ok(false);
    }
    anyhow::ensure!(
        !sealed.is_empty() && !branch.is_empty(),
        "worktree_metadata_missing: no sealed branch; checkout kept for a decision"
    );
    let lock = lock(runner, repo)?;
    let common = std::fs::canonicalize(&lock.common_dir)?;
    let missing_pointer = raw.is_none();
    let dir = raw.unwrap_or_else(|| {
        common
            .join("worktrees")
            .join(Path::new(path).file_name().expect("checkout name"))
    });
    anyhow::ensure!(
        dir.parent() == Some(common.join("worktrees").as_path()),
        "worktree_metadata_invalid: {} is not owned by {repo}",
        dir.display()
    );
    anyhow::ensure!(
        Git::new(runner, repo).branch_head(branch)?.as_deref() == Some(sealed),
        "worktree_metadata_missing: branch moved beyond seal {sealed}; checkout kept for a decision"
    );
    let temporary = tempfile::tempdir_in(common.join("worktrees"))?;
    let admin = temporary.path();
    std::fs::write(admin.join("commondir"), "../..\n")?;
    std::fs::write(admin.join("HEAD"), format!("ref: refs/heads/{branch}\n"))?;
    std::fs::write(admin.join("gitdir"), format!("{}\n", pointer.display()))?;
    std::fs::write(admin.join(RECOVERED_INDEX), format!("{sealed}\n"))?;
    Git::new(runner, repo).run(&[
        &format!("--git-dir={}", admin.display()),
        "read-tree",
        sealed,
    ])?;
    std::fs::rename(admin, &dir)?;
    if missing_pointer {
        std::fs::write(pointer, format!("gitdir: {}\n", dir.display()))?;
    }
    Ok(true)
}

/// Full removal status, including ignored files. This is only for worktree
/// deletion safety: callers must classify `!!` rows against the editable
/// disposable-path list before removing anything.
pub(crate) fn with_worktree_status<T>(
    runner: &dyn Runner,
    repo: &str,
    path: &str,
    classify: impl FnOnce(&mut dyn std::io::BufRead) -> Result<T>,
) -> Result<T> {
    let args = [
        "-C",
        path,
        "status",
        "--porcelain",
        "--ignored",
        "--untracked-files=all",
        "-z",
    ];
    if !runner.is_real() {
        let text = Git::new(runner, repo)
            .with_timeout(GIT_TIMEOUT)
            .stdout(&args)?;
        return classify(&mut std::io::Cursor::new(text));
    }
    let temporary = tempfile::tempdir()?;
    let logs = crate::runner::OutputLogs {
        stdout: temporary.path().join("status"),
        stderr: temporary.path().join("stderr"),
    };
    let capture = runner.capture(
        &crate::runner::Cmd::new("git", GIT_TIMEOUT)
            .env("LC_ALL", "C")
            .args(["-C", repo])
            .args(args),
        Some(&logs),
    )?;
    anyhow::ensure!(
        capture.complete() && capture.output.success(),
        "worktree status failed: {}",
        capture.output.error_text()
    );
    classify(&mut std::io::BufReader::new(File::open(logs.stdout)?))
}

/// The branch checked out in the repository's main checkout.
pub(crate) fn symbolic_head(runner: &dyn Runner, repo: &str) -> Result<String> {
    Git::new(runner, repo)
        .with_timeout(Duration::from_secs(5))
        .run(&["symbolic-ref", "--short", "HEAD"])
}

/// SHA of `refs/heads/<branch>`, or of any ref name passed in.
pub(crate) fn rev_parse(runner: &dyn Runner, repo: &str, rev: &str) -> Result<String> {
    Git::new(runner, repo)
        .with_timeout(Duration::from_secs(5))
        .run(&["rev-parse", rev])
}

pub(crate) fn exclude_plugin_paths_locked(runner: &dyn Runner, repo: &str) -> Result<()> {
    let exclude = Git::new(runner, repo)
        .with_timeout(Duration::from_secs(5))
        .run(&["rev-parse", "--git-path", "info/exclude"])?;
    let path = if Path::new(&exclude).is_absolute() {
        PathBuf::from(&exclude)
    } else {
        Path::new(repo).join(&exclude)
    };
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let mut text = current;
    for line in [".herdr-project/", ".worktrees/"] {
        if text.lines().any(|l| l.trim() == line) {
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(line);
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, text).with_context(|| format!("could not update {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, fail};
    use crate::runner::{Cmd, RealRunner};

    #[test]
    fn ancestry_errors_are_not_negative_answers() {
        use crate::runner::Output;
        for output in [
            fail(1, "object database error"),
            fail(2, ""),
            Output {
                code: Some(1),
                timed_out: true,
                ..Default::default()
            },
            Output {
                code: Some(0),
                timed_out: true,
                ..Default::default()
            },
            Output::default(), // signal
        ] {
            let fake = FakeRunner::new();
            fake.on("merge-base --is-ancestor", output);
            let runner = fake;
            assert!(Git::new(&runner, "/repo").is_ancestor("a", "b").is_err());
        }

        let fake = FakeRunner::new();
        fake.on_fn(|_| true, |_| Err(anyhow::anyhow!("could not spawn git")));
        assert!(Git::new(&fake, "/repo").is_ancestor("a", "b").is_err());
    }

    #[test]
    fn timeout_reports_repository_lock_snapshot() {
        let (_dir, repo) = repo_with_commit();
        let lock = repo.join(".git/packed-refs.lock");
        std::fs::write(&lock, "").unwrap();
        let fake = FakeRunner::new();
        fake.on(
            "for-each-ref",
            crate::runner::Output {
                timed_out: true,
                ..Default::default()
            },
        );
        let error = Git::new(&fake, &repo)
            .branch_head("main")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("timed out; repo activity at timeout:"),
            "{error}"
        );
        assert!(error.contains("packed-refs.lock"), "{error}");
    }

    #[test]
    fn absent_branch_is_an_answer_but_a_broken_repository_is_not() {
        let (_dir, repo) = repo_with_commit();
        let root = tempfile::tempdir().unwrap();
        let runner = RealRunner;
        let git = Git::new(&runner, &repo);
        assert!(git.branch_head("cloud-only").unwrap().is_none());
        assert!(git.branch_head("main").unwrap().is_some());
        // Callers supply a local branch name, not an arbitrary ref. In
        // particular a tag must not pass integration-branch validation.
        git.run(&["tag", "release"]).unwrap();
        for name in ["refs/tags/release", "refs/heads/main"] {
            assert!(git.branch_head(name).unwrap().is_none());
        }
        // A prefix match is not the requested branch.
        git.run(&["branch", "cloud-only/child"]).unwrap();
        assert!(git.branch_head("cloud-only").unwrap().is_none());
        assert!(Git::new(&runner, root.path()).branch_head("main").is_err());
    }

    fn repo_with_commit() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let repo_s = repo.to_string_lossy().into_owned();
        let run = |args: &[&str]| {
            RealRunner
                .run(
                    &Cmd::new("git", Duration::from_secs(5))
                        .args(["-C", &repo_s])
                        .args(args.iter().copied()),
                )
                .unwrap()
        };
        assert!(
            RealRunner
                .run(&Cmd::new("git", Duration::from_secs(5)).args(["init", "-b", "main", &repo_s]))
                .unwrap()
                .success()
        );
        let _ = run(&["config", "user.email", "ade@test"]);
        let _ = run(&["config", "user.name", "ade"]);
        std::fs::write(repo.join("README"), "x\n").unwrap();
        assert!(run(&["add", "README"]).success());
        assert!(run(&["commit", "-m", "init"]).success());
        (dir, repo)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn d67_forget_does_not_prune_a_checkout_hidden_by_a_namespace() {
        if let Ok(repo) = std::env::var("D67_TEST_REPO") {
            forget_worktree(&RealRunner, &repo, &std::env::var("D67_TEST_GONE").unwrap()).unwrap();
            return;
        }
        let (dir, repo) = repo_with_commit();
        let hidden_root = dir.path().join("hidden");
        std::fs::create_dir(&hidden_root).unwrap();
        let hidden = hidden_root.join("lane");
        let gone = dir.path().join("gone");
        for (path, branch) in [(&hidden, "hidden"), (&gone, "gone")] {
            Git::new(&RealRunner, &repo)
                .run(&["worktree", "add", "-b", branch, path.to_str().unwrap()])
                .unwrap();
        }
        std::fs::remove_dir_all(&gone).unwrap();
        let admin = std::fs::read_to_string(hidden.join(".git")).unwrap();
        let admin = Path::new(admin.trim().strip_prefix("gitdir: ").unwrap());
        let result = std::process::Command::new("/usr/bin/bwrap")
            .args([
                "--ro-bind",
                "/",
                "/",
                "--bind",
                repo.to_str().unwrap(),
                repo.to_str().unwrap(),
                "--tmpfs",
                hidden_root.to_str().unwrap(),
                "--dev",
                "/dev",
            ])
            .args([
                "--setenv",
                "D67_TEST_REPO",
                repo.to_str().unwrap(),
                "--setenv",
                "D67_TEST_GONE",
                gone.to_str().unwrap(),
            ])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "git::tests::d67_forget_does_not_prune_a_checkout_hidden_by_a_namespace",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(hidden.exists());
        assert!(admin.exists(), "hidden registration was pruned");
        assert!(
            Git::new(&RealRunner, &repo)
                .run(&["worktree", "list", "--porcelain"])
                .unwrap()
                .contains(hidden.to_str().unwrap())
        );
    }

    #[test]
    fn dirty_remove_is_refused() {
        let (_dir, repo) = repo_with_commit();
        let wt = repo.join("lane");
        Git::new(&RealRunner, &repo)
            .run(&["worktree", "add", "-b", "lane", wt.to_str().unwrap()])
            .unwrap();
        std::fs::write(wt.join("README"), "unique edit").unwrap();
        let err = worktree_remove(&RealRunner, repo.to_str().unwrap(), wt.to_str().unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a clean worktree"), "{err}");
        assert_eq!(
            std::fs::read_to_string(wt.join("README")).unwrap(),
            "unique edit"
        );
    }

    #[cfg(unix)]
    #[test]
    fn d67_permission_refusal_keeps_administration_until_files_are_removed() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, repo) = repo_with_commit();
        let wt = repo.join("lane");
        Git::new(&RealRunner, &repo)
            .run(&["worktree", "add", "-b", "lane", wt.to_str().unwrap()])
            .unwrap();
        let cache = wt.join("cache");
        std::fs::create_dir(&cache).unwrap();
        std::fs::write(cache.join("output"), "x").unwrap();
        std::fs::write(repo.join(".git/info/exclude"), "cache/\n").unwrap();
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o500)).unwrap();
        let refused = worktree_remove(&RealRunner, repo.to_str().unwrap(), wt.to_str().unwrap());
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(refused.is_err());
        let raw = std::fs::read_to_string(wt.join(".git")).unwrap();
        assert!(Path::new(raw.trim().strip_prefix("gitdir: ").unwrap()).exists());
        assert!(
            Git::new(&RealRunner, &wt)
                .run(&["rev-parse", "HEAD"])
                .is_ok()
        );
    }

    #[test]
    fn worktree_lifecycle_on_a_real_repo() {
        let (_dir, repo) = repo_with_commit();
        let repo_s = repo.to_string_lossy().into_owned();
        let wt = repo.join(".worktrees/t-0001");
        Git::new(&RealRunner, &repo)
            .with_timeout(GIT_TIMEOUT)
            .run(&[
                "worktree",
                "add",
                &wt.to_string_lossy(),
                "-b",
                "lane/t-0001",
                "main",
            ])
            .unwrap();
        assert!(wt.is_dir());
        assert!(wt.join("README").is_file());
        worktree_remove(&RealRunner, &repo_s, &wt.to_string_lossy()).unwrap();
        assert!(!wt.exists());
        // Branch is kept.
        assert!(rev_parse(&RealRunner, &repo_s, "refs/heads/lane/t-0001").is_ok());
    }
}
