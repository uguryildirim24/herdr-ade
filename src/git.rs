//! Repository lock and git helpers (SPEC-ADE D4, D6, D9, item 34).
//!
//! Lock order: the project lock is never held while this lock is acquired.
//! Git runs outside the project lock. Public helpers stay for A2/A3 merge
//! and for tests that do not call every path from this crate's binary.

#![allow(dead_code)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::runner::{Cmd, Runner};

const GIT_TIMEOUT: Duration = Duration::from_secs(20);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Held while worktree add/remove, `info/exclude` edits, and plugin ref writes
/// run. Keyed by `git rev-parse --git-common-dir`.
pub struct RepoLock {
    _file: File,
    pub common_dir: PathBuf,
}

fn git(runner: &dyn Runner, repo: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let out = runner.run(
        &Cmd::new("git", timeout)
            .args(["-C", repo])
            .args(args.iter().copied()),
    )?;
    if !out.success() {
        bail!("git {}: {}", args.join(" "), out.error_text());
    }
    Ok(out.stdout.trim().to_string())
}

/// Absolute `git-common-dir` for `repo`.
pub fn common_dir(runner: &dyn Runner, repo: &str) -> Result<PathBuf> {
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
pub fn lock(runner: &dyn Runner, repo: &str) -> Result<RepoLock> {
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
pub fn worktree_add(
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

/// `git worktree remove` without `--force`. A dirty tree refuses.
pub fn worktree_remove(runner: &dyn Runner, repo: &str, path: &str) -> Result<()> {
    git(runner, repo, &["worktree", "remove", path], GIT_TIMEOUT)?;
    Ok(())
}

/// True when `ancestor` is an ancestor of `descendant`.
pub fn is_ancestor(
    runner: &dyn Runner,
    repo: &str,
    ancestor: &str,
    descendant: &str,
) -> Result<bool> {
    let out = runner.run(&Cmd::new("git", Duration::from_secs(5)).args([
        "-C",
        repo,
        "merge-base",
        "--is-ancestor",
        ancestor,
        descendant,
    ]))?;
    Ok(out.success())
}

/// SHA of `refs/heads/<branch>`, or of any ref name passed in.
pub fn rev_parse(runner: &dyn Runner, repo: &str, rev: &str) -> Result<String> {
    git(runner, repo, &["rev-parse", rev], Duration::from_secs(5))
}

/// `git update-ref <ref> <new> <old>`: refuses when the old value does not match.
pub fn update_ref(
    runner: &dyn Runner,
    repo: &str,
    git_ref: &str,
    new: &str,
    old: &str,
) -> Result<()> {
    git(
        runner,
        repo,
        &["update-ref", git_ref, new, old],
        Duration::from_secs(5),
    )?;
    Ok(())
}

/// Porcelain worktree rows: `(path, branch)` where branch is `refs/heads/...` or empty.
pub fn worktree_list(runner: &dyn Runner, repo: &str) -> Result<Vec<(PathBuf, String)>> {
    let text = git(
        runner,
        repo,
        &["worktree", "list", "--porcelain"],
        Duration::from_secs(5),
    )?;
    let mut rows = Vec::new();
    let mut path = PathBuf::new();
    let mut branch = String::new();
    let flush = |rows: &mut Vec<(PathBuf, String)>, path: &mut PathBuf, branch: &mut String| {
        if !path.as_os_str().is_empty() {
            rows.push((path.clone(), branch.clone()));
        }
        path.clear();
        branch.clear();
    };
    for line in text.lines() {
        if line.is_empty() {
            flush(&mut rows, &mut path, &mut branch);
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            path = PathBuf::from(rest);
        } else if let Some(rest) = line.strip_prefix("branch ") {
            branch = rest.to_string();
        }
    }
    flush(&mut rows, &mut path, &mut branch);
    Ok(rows)
}

fn branch_ref(branch: &str) -> String {
    if branch.starts_with("refs/") {
        branch.to_string()
    } else {
        format!("refs/heads/{branch}")
    }
}

/// Where `branch` is checked out, if anywhere.
pub fn branch_checkout(runner: &dyn Runner, repo: &str, branch: &str) -> Result<Option<PathBuf>> {
    let want = branch_ref(branch);
    for (path, found) in worktree_list(runner, repo)? {
        if found == want {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Commit one file on `branch` under the repository lock (SPEC-ADE D9).
///
/// - Branch checked out clean: write, add, commit in that checkout.
/// - Branch not checked out: temporary-index recipe plus `update-ref` with the
///   expected old value.
/// - Branch checked out dirty: `integration_checkout_dirty`.
pub fn commit_file_on_branch(
    runner: &dyn Runner,
    repo: &str,
    branch: &str,
    relative_path: &str,
    contents: &[u8],
    message: &str,
) -> Result<String> {
    let _lock = lock(runner, repo)?;
    let git_ref = branch_ref(branch);
    match branch_checkout(runner, repo, branch)? {
        Some(checkout) => commit_in_checkout(runner, &checkout, relative_path, contents, message),
        None => commit_detached(runner, repo, &git_ref, relative_path, contents, message),
    }
}

fn dirty(runner: &dyn Runner, cwd: &Path) -> Result<bool> {
    let out = runner.run(
        &Cmd::new("git", Duration::from_secs(5))
            .args(["status", "--porcelain"])
            .cwd(cwd),
    )?;
    if !out.success() {
        bail!("git status --porcelain: {}", out.error_text());
    }
    Ok(!out.stdout.trim().is_empty())
}

fn commit_in_checkout(
    runner: &dyn Runner,
    checkout: &Path,
    relative_path: &str,
    contents: &[u8],
    message: &str,
) -> Result<String> {
    if dirty(runner, checkout)? {
        bail!("integration_checkout_dirty");
    }
    let dest = checkout.join(relative_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    crate::project::write_atomic(&dest, contents)?;
    let cwd = checkout.to_string_lossy().into_owned();
    git(runner, &cwd, &["add", "--", relative_path], GIT_TIMEOUT)?;
    git(
        runner,
        &cwd,
        &["commit", "-m", message, "--", relative_path],
        WRITE_TIMEOUT,
    )?;
    git(runner, &cwd, &["rev-parse", "HEAD"], Duration::from_secs(5))
}

/// Commit `relative_path` with parent `parent` and do not update a ref.
/// Used when the new lane branch does not exist yet (SPEC-ADE D4, D9).
pub fn commit_file_from_parent(
    runner: &dyn Runner,
    repo: &str,
    parent: &str,
    relative_path: &str,
    contents: &[u8],
    message: &str,
) -> Result<String> {
    let _lock = lock(runner, repo)?;
    let parent_sha = rev_parse(runner, repo, parent)?;
    write_commit_with_file(runner, repo, &parent_sha, relative_path, contents, message)
}

fn commit_detached(
    runner: &dyn Runner,
    repo: &str,
    git_ref: &str,
    relative_path: &str,
    contents: &[u8],
    message: &str,
) -> Result<String> {
    let old = git(
        runner,
        repo,
        &["rev-parse", git_ref],
        Duration::from_secs(5),
    )?;
    let new = write_commit_with_file(runner, repo, &old, relative_path, contents, message)?;
    update_ref(runner, repo, git_ref, &new, &old)?;
    Ok(new)
}

fn write_commit_with_file(
    runner: &dyn Runner,
    repo: &str,
    parent_sha: &str,
    relative_path: &str,
    contents: &[u8],
    message: &str,
) -> Result<String> {
    let blob = {
        let out = runner.run(
            &Cmd::new("git", WRITE_TIMEOUT)
                .args(["-C", repo, "hash-object", "-w", "--stdin"])
                .stdin(String::from_utf8_lossy(contents).into_owned()),
        )?;
        if !out.success() {
            bail!("git hash-object: {}", out.error_text());
        }
        out.stdout.trim().to_string()
    };
    let tmp_index = std::env::temp_dir().join(format!(
        "herdr-ade-index.{}.{}",
        std::process::id(),
        jiff::Timestamp::now().as_nanosecond()
    ));
    let index_s = tmp_index.to_string_lossy().into_owned();
    let run_index = |args: &[&str]| -> Result<String> {
        let out = runner.run(
            &Cmd::new("git", WRITE_TIMEOUT)
                .args(["-C", repo])
                .args(args.iter().copied())
                .env("GIT_INDEX_FILE", &index_s),
        )?;
        if !out.success() {
            bail!("git {}: {}", args.join(" "), out.error_text());
        }
        Ok(out.stdout.trim().to_string())
    };
    let result = (|| -> Result<String> {
        run_index(&["read-tree", parent_sha])?;
        let cacheinfo = format!("100644,{blob},{relative_path}");
        run_index(&["update-index", "--add", "--cacheinfo", &cacheinfo])?;
        let tree = run_index(&["write-tree"])?;
        let out = runner.run(&Cmd::new("git", WRITE_TIMEOUT).args([
            "-C",
            repo,
            "commit-tree",
            &tree,
            "-p",
            parent_sha,
            "-m",
            message,
        ]))?;
        if !out.success() {
            bail!("git commit-tree: {}", out.error_text());
        }
        Ok(out.stdout.trim().to_string())
    })();
    let _ = std::fs::remove_file(&tmp_index);
    result
}

/// Adds `.herdr-project/` and `.worktrees/` to `info/exclude` when missing.
pub fn exclude_plugin_paths(runner: &dyn Runner, repo: &str) -> Result<()> {
    let _lock = lock(runner, repo)?;
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
    fn ancestor_and_update_ref_go_through_the_runner() {
        let runner = FakeRunner::new();
        runner.on("merge-base --is-ancestor", ok(""));
        runner.on("update-ref", ok(""));
        assert!(is_ancestor(&runner, "/repo", "B", "C").unwrap());
        update_ref(&runner, "/repo", "refs/heads/main", "H", "V").unwrap();
        let calls = runner.calls.borrow();
        let uref = calls
            .iter()
            .find(|c| c.display().contains("update-ref"))
            .unwrap();
        assert_eq!(
            uref.args
                .iter()
                .rev()
                .take(3)
                .rev()
                .cloned()
                .collect::<Vec<_>>(),
            ["refs/heads/main", "H", "V"]
        );
    }

    #[test]
    fn commit_file_on_a_real_detached_branch() {
        let (_dir, repo) = repo_with_commit();
        let repo_s = repo.to_string_lossy().into_owned();
        let sha = commit_file_on_branch(
            &RealRunner,
            &repo_s,
            "main",
            "tasks/t-0001.md",
            b"# brief\n",
            "docs(tasks): t-0001",
        )
        .unwrap();
        assert_eq!(sha.len(), 40);
        // Detached recipe: the file is in the commit, not necessarily in the tree
        // of a second checkout. The current checkout is main and was dirty-checked.
        let head = rev_parse(&RealRunner, &repo_s, "refs/heads/main").unwrap();
        assert_eq!(head, sha);
        let show = RealRunner
            .run(&Cmd::new("git", Duration::from_secs(5)).args([
                "-C",
                &repo_s,
                "show",
                &format!("{sha}:tasks/t-0001.md"),
            ]))
            .unwrap();
        assert!(show.success());
        assert_eq!(show.stdout, "# brief\n");
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

    #[test]
    fn commit_file_from_parent_does_not_move_main() {
        let (_dir, repo) = repo_with_commit();
        let repo_s = repo.to_string_lossy().into_owned();
        let main = rev_parse(&RealRunner, &repo_s, "refs/heads/main").unwrap();
        let sha = commit_file_from_parent(
            &RealRunner,
            &repo_s,
            "main",
            "tasks/t-0001.md",
            b"# brief\n",
            "docs(tasks): t-0001",
        )
        .unwrap();
        assert_ne!(sha, main);
        assert_eq!(
            rev_parse(&RealRunner, &repo_s, "refs/heads/main").unwrap(),
            main
        );
        let show = RealRunner
            .run(&Cmd::new("git", Duration::from_secs(5)).args([
                "-C",
                &repo_s,
                "show",
                &format!("{sha}:tasks/t-0001.md"),
            ]))
            .unwrap();
        assert!(show.success());
        assert_eq!(show.stdout, "# brief\n");
    }

    #[test]
    fn dirty_checkout_is_refused() {
        let (_dir, repo) = repo_with_commit();
        let repo_s = repo.to_string_lossy().into_owned();
        std::fs::write(repo.join("dirty"), "x\n").unwrap();
        let err = commit_file_on_branch(
            &RealRunner,
            &repo_s,
            "main",
            "tasks/t-0001.md",
            b"x\n",
            "docs(tasks): t-0001",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("integration_checkout_dirty"), "{err}");
    }
}
