//! Repository lock and git helpers (SPEC-ADE D4, D6, D9, item 34).
//!
//! Lock order: the project lock is never held while this lock is acquired.
//! Git runs outside the project lock. Public helpers stay for A2/A3 merge
//! and for tests that do not call every path from this crate's binary.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::runner::{Cmd, Runner};

const GIT_TIMEOUT: Duration = Duration::from_secs(20);

/// Held while worktree add/remove, `info/exclude` edits, and plugin ref writes
/// run. Keyed by `git rev-parse --git-common-dir`.
pub(crate) struct RepoLock {
    _file: File,
    #[allow(dead_code)]
    pub(crate) common_dir: PathBuf,
}

fn git(runner: &dyn Runner, repo: &str, args: &[&str], timeout: Duration) -> Result<String> {
    Ok(git_raw(runner, repo, args, timeout)?.trim().to_string())
}

// Porcelain status has significant leading spaces and NUL-framed paths.
fn git_raw(runner: &dyn Runner, repo: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let out = runner.run(
        &Cmd::new("git", timeout)
            .args(["-C", repo])
            .args(args.iter().copied()),
    )?;
    if out.timed_out {
        bail!(
            "git {}: timed out; repo activity at timeout: {}",
            args.join(" "),
            repo_activity(repo)
        );
    }
    if !out.success() {
        bail!("git {}: {}", args.join(" "), out.error_text());
    }
    Ok(out.stdout)
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
    let raw = git(
        runner,
        repo,
        &["rev-parse", "--git-common-dir"],
        Duration::from_secs(5),
    )?;
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

/// `git worktree add <repo>/.worktrees/<id> -b <branch> <base>` (SPEC-ADE D4).
pub(crate) fn worktree_add(
    runner: &dyn Runner,
    repo: &str,
    id: &str,
    branch: &str,
    base: &str,
) -> Result<PathBuf> {
    let path = Path::new(repo).join(".worktrees").join(id);
    let path_s = path.to_string_lossy().into_owned();
    git(
        runner,
        repo,
        &["worktree", "add", &path_s, "-b", branch, base],
        GIT_TIMEOUT,
    )?;
    Ok(path)
}

/// `git worktree remove` without `--force`. Callers run ADE's stricter status
/// inspection first because Git itself permits deletion of ignored files.
pub(crate) fn worktree_remove(runner: &dyn Runner, repo: &str, path: &str) -> Result<()> {
    git(runner, repo, &["worktree", "remove", path], GIT_TIMEOUT)?;
    Ok(())
}

/// Drops registrations whose checkout directory is already gone. Expiring
/// immediately makes manual removal idempotent instead of retaining Git's
/// default grace-period entry.
pub(crate) fn worktree_prune(runner: &dyn Runner, repo: &str) -> Result<()> {
    git(
        runner,
        repo,
        &["worktree", "prune", "--expire=now"],
        GIT_TIMEOUT,
    )?;
    Ok(())
}

/// Full removal status, including ignored files. This is only for worktree
/// deletion safety: callers must classify `!!` rows against the editable
/// disposable-path list before removing anything.
pub(crate) fn worktree_status_with_ignored(
    runner: &dyn Runner,
    repo: &str,
    path: &str,
) -> Result<String> {
    git_raw(
        runner,
        repo,
        &[
            "-C",
            path,
            "status",
            "--porcelain",
            "--ignored",
            "--untracked-files=all",
            "-z",
        ],
        GIT_TIMEOUT,
    )
}

/// The branch checked out in the repository's main checkout.
pub(crate) fn symbolic_head(runner: &dyn Runner, repo: &str) -> Result<String> {
    git(
        runner,
        repo,
        &["symbolic-ref", "--short", "HEAD"],
        Duration::from_secs(5),
    )
}

/// SHA of `refs/heads/<branch>`, or of any ref name passed in.
pub(crate) fn rev_parse(runner: &dyn Runner, repo: &str, rev: &str) -> Result<String> {
    git(runner, repo, &["rev-parse", rev], Duration::from_secs(5))
}

/// A typed ancestry answer: 0 is yes, 1 with empty stderr is no; other exits,
/// diagnostics on a negative result, signals, timeouts and spawn errors fail.
/// A normal no is a result, not an error.
pub(crate) fn is_ancestor(
    runner: &dyn Runner,
    repo: &str,
    ancestor: &str,
    descendant: &str,
) -> Result<bool> {
    let out = runner.run(&Cmd::new("git", GIT_TIMEOUT).args([
        "-C",
        repo,
        "merge-base",
        "--is-ancestor",
        ancestor,
        descendant,
    ]))?;
    out.boolean_answer().with_context(|| {
        format!(
            "`git merge-base --is-ancestor {ancestor} {descendant}` failed: exit={:?}, {}",
            out.code,
            out.error_text()
        )
    })
}

/// Query an optional local branch without conflating an absent ref with a git
/// error. `for-each-ref` answers absence with empty output; every nonzero exit
/// still means the query failed. Match the full name (git also lists prefixes).
pub(crate) fn branch_head(runner: &dyn Runner, repo: &str, branch: &str) -> Result<Option<String>> {
    // Keep the local-branch namespace even when the supplied name starts
    // with `refs/`: review and worktree callers use short branch names.
    let want = format!("refs/heads/{branch}");
    let rows = git(
        runner,
        repo,
        &["for-each-ref", "--format=%(refname) %(objectname)", &want],
        Duration::from_secs(5),
    )?;
    Ok(rows.lines().find_map(|row| {
        let (name, sha) = row.split_once(' ')?;
        (name == want).then(|| sha.to_string())
    }))
}

pub(crate) fn exclude_plugin_paths_locked(runner: &dyn Runner, repo: &str) -> Result<()> {
    let exclude = git(
        runner,
        repo,
        &["rev-parse", "--git-path", "info/exclude"],
        Duration::from_secs(5),
    )?;
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
    use crate::runner::RealRunner;
    use crate::runner::fake::{FakeRunner, fail, ok};

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
            assert!(is_ancestor(&runner, "/repo", "a", "b").is_err());
        }

        let fake = FakeRunner::new();
        fake.on_fn(|_| true, |_| Err(anyhow::anyhow!("could not spawn git")));
        assert!(is_ancestor(&fake, "/repo", "a", "b").is_err());
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
        let error = branch_head(&fake, &repo.to_string_lossy(), "main")
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
        let repo_s = repo.to_string_lossy();
        assert!(
            branch_head(&runner, &repo_s, "cloud-only")
                .unwrap()
                .is_none()
        );
        assert!(branch_head(&runner, &repo_s, "main").unwrap().is_some());
        // Callers supply a local branch name, not an arbitrary ref. In
        // particular a tag must not pass integration-branch validation.
        git(&runner, &repo_s, &["tag", "release"], GIT_TIMEOUT).unwrap();
        for name in ["refs/tags/release", "refs/heads/main"] {
            assert!(branch_head(&runner, &repo_s, name).unwrap().is_none());
        }
        // A prefix match is not the requested branch.
        git(
            &runner,
            &repo_s,
            &["branch", "cloud-only/child"],
            GIT_TIMEOUT,
        )
        .unwrap();
        assert!(
            branch_head(&runner, &repo_s, "cloud-only")
                .unwrap()
                .is_none()
        );
        assert!(branch_head(&runner, &root.path().to_string_lossy(), "main").is_err());
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

    #[test]
    fn worktree_add_argv_and_remove_without_force() {
        let runner = FakeRunner::new();
        runner.on("rev-parse --git-common-dir", ok("/repo/.git\n"));
        runner.on("worktree add", ok(""));
        runner.on("worktree remove", ok(""));
        let path = worktree_add(&runner, "/repo", "t-0001", "lane/t-0001", "main").unwrap();
        assert_eq!(path, PathBuf::from("/repo/.worktrees/t-0001"));
        let calls = runner.calls.borrow();
        let add = calls
            .iter()
            .find(|c| c.display().contains("worktree add"))
            .unwrap();
        assert!(add.args.contains(&"/repo/.worktrees/t-0001".into()));
        assert!(add.args.contains(&"-b".into()));
        assert!(!add.args.iter().any(|a| a == "--force"));
        drop(calls);
        worktree_remove(&runner, "/repo", "/repo/.worktrees/t-0001").unwrap();
        let calls = runner.calls.borrow();
        let remove = calls
            .iter()
            .find(|c| c.display().contains("worktree remove"))
            .unwrap();
        assert!(!remove.args.iter().any(|a| a == "--force"));
    }

    #[test]
    fn dirty_remove_is_refused() {
        let runner = FakeRunner::new();
        runner.on("worktree remove", fail(1, "not a clean worktree"));
        let err = worktree_remove(&runner, "/repo", "/wt")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a clean worktree"), "{err}");
    }

    #[test]
    fn worktree_lifecycle_on_a_real_repo() {
        let (_dir, repo) = repo_with_commit();
        let repo_s = repo.to_string_lossy().into_owned();
        let wt = worktree_add(&RealRunner, &repo_s, "t-0001", "lane/t-0001", "main").unwrap();
        assert!(wt.is_dir());
        assert!(wt.join("README").is_file());
        worktree_remove(&RealRunner, &repo_s, &wt.to_string_lossy()).unwrap();
        assert!(!wt.exists());
        // Branch is kept.
        assert!(rev_parse(&RealRunner, &repo_s, "refs/heads/lane/t-0001").is_ok());
    }
}
