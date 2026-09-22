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
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Held while worktree add/remove, `info/exclude` edits, and plugin ref writes
/// run. Keyed by `git rev-parse --git-common-dir`.
pub(crate) struct RepoLock {
    _file: File,
    pub(crate) common_dir: PathBuf,
}

fn git(runner: &dyn Runner, repo: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let out = runner.run(
        &Cmd::new("git", timeout)
            // These calls return answers in stdout, never in failure status.
            .exit_meaning(crate::runner::ExitMeaning::Required)
            .args(["-C", repo])
            .args(args.iter().copied()),
    )?;
    if !out.success() {
        bail!("git {}: {}", args.join(" "), out.error_text());
    }
    Ok(out.stdout.trim().to_string())
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
    git(
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
/// The runner uses the same contract so a normal no never enters the ledger.
pub(crate) fn is_ancestor(
    runner: &dyn Runner,
    repo: &str,
    ancestor: &str,
    descendant: &str,
) -> Result<bool> {
    let out = runner.run(
        &Cmd::new("git", GIT_TIMEOUT)
            .args([
                "-C",
                repo,
                "merge-base",
                "--is-ancestor",
                ancestor,
                descendant,
            ])
            .exit_meaning(crate::runner::ExitMeaning::Boolean),
    )?;
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
    // with `refs/`: round and worktree callers use short branch names.
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

/// `git update-ref <ref> <new> <old>`: refuses when the old value does not match.
fn update_ref(runner: &dyn Runner, repo: &str, git_ref: &str, new: &str, old: &str) -> Result<()> {
    git(
        runner,
        repo,
        &["update-ref", git_ref, new, old],
        Duration::from_secs(5),
    )?;
    Ok(())
}

/// Porcelain worktree rows: `(path, branch)` where branch is `refs/heads/...` or empty.
pub(crate) fn worktree_list(runner: &dyn Runner, repo: &str) -> Result<Vec<(PathBuf, String)>> {
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
fn branch_checkout(runner: &dyn Runner, repo: &str, branch: &str) -> Result<Option<PathBuf>> {
    let want = branch_ref(branch);
    for (path, found) in worktree_list(runner, repo)? {
        if found == want {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// The one D9 commit: `files` (path, text) on `branch`, whose head must be
/// `expected_old`. The caller holds the repository lock ([`lock`]); git runs
/// outside the project lock.
///
/// - Branch checked out clean: write, add, commit in that checkout. An
///   untracked copy of a file being committed (a turn file the critic wrote)
///   is allowed; a modified tracked copy is someone's edit and refuses.
/// - Branch checked out dirty: `integration_checkout_dirty`.
/// - Branch not checked out: a temporary index in `tmp_dir` plus
///   `update-ref <branch> <new> <expected_old>`.
pub(crate) fn commit_files_locked(
    runner: &dyn Runner,
    repo: &Path,
    branch: &str,
    files: &[(&str, &str)],
    message: &str,
    expected_old: &str,
    tmp_dir: &Path,
) -> Result<String> {
    let repo_s = repo.to_string_lossy().into_owned();
    let git_ref = branch_ref(branch);
    let head = git(
        runner,
        &repo_s,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("{git_ref}^{{commit}}"),
        ],
        Duration::from_secs(5),
    )
    .with_context(|| format!("branch_missing: `{branch}` does not exist"))?;
    if head != expected_old {
        bail!("head_moved: `{branch}` is at {head}, expected {expected_old}");
    }
    let own: Vec<&str> = files.iter().map(|(p, _)| *p).collect();
    match branch_checkout(runner, &repo_s, branch)? {
        Some(dir) => {
            let dir_s = dir.to_string_lossy().into_owned();
            let status = git(
                runner,
                &dir_s,
                &["status", "--porcelain", "--untracked-files=all"],
                Duration::from_secs(10),
            )?;
            let dirty: Vec<String> = status
                .lines()
                .filter(|l| l.len() > 3)
                .filter(|l| {
                    let path = l[3..].trim().trim_matches('"');
                    !(l.starts_with("??") && own.contains(&path))
                })
                .map(|l| l[3..].trim().to_string())
                .collect();
            if !dirty.is_empty() {
                bail!(
                    "integration_checkout_dirty: {} has uncommitted changes ({})",
                    dir.display(),
                    dirty.join(", ")
                );
            }
            for (path, text) in files {
                let target = dir.join(path);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                crate::project::write_atomic(&target, text.as_bytes())?;
            }
            let mut add = vec!["add", "--"];
            add.extend(own.iter().copied());
            git(runner, &dir_s, &add, GIT_TIMEOUT)?;
            let mut commit = vec!["commit", "-q", "--no-verify", "-m", message, "--"];
            commit.extend(own.iter().copied());
            git(runner, &dir_s, &commit, WRITE_TIMEOUT)?;
            git(
                runner,
                &dir_s,
                &["rev-parse", "HEAD"],
                Duration::from_secs(5),
            )
        }
        None => {
            std::fs::create_dir_all(tmp_dir)
                .with_context(|| format!("could not create {}", tmp_dir.display()))?;
            let index = tmp_dir.join(format!(
                "index-{}-{}",
                std::process::id(),
                jiff::Timestamp::now().as_nanosecond()
            ));
            let index_s = index.to_string_lossy().into_owned();
            let with_index = |args: &[&str], stdin: Option<&str>| -> Result<String> {
                let mut cmd = Cmd::new("git", WRITE_TIMEOUT)
                    .args(["-C", repo_s.as_str()])
                    .args(args.iter().copied())
                    .env("GIT_INDEX_FILE", index_s.as_str());
                if let Some(text) = stdin {
                    cmd = cmd.stdin(text);
                }
                let out = runner.run(&cmd)?;
                if !out.success() {
                    bail!("git {}: {}", args.join(" "), out.error_text());
                }
                Ok(out.stdout.trim().to_string())
            };
            let result = (|| -> Result<String> {
                with_index(&["read-tree", expected_old], None)?;
                for (path, text) in files {
                    let blob = with_index(&["hash-object", "-w", "--stdin"], Some(text))?;
                    let cacheinfo = format!("100644,{blob},{path}");
                    with_index(&["update-index", "--add", "--cacheinfo", &cacheinfo], None)?;
                }
                let tree = with_index(&["write-tree"], None)?;
                let commit = git(
                    runner,
                    &repo_s,
                    &["commit-tree", &tree, "-p", expected_old, "-m", message],
                    WRITE_TIMEOUT,
                )?;
                update_ref(runner, &repo_s, &git_ref, &commit, expected_old)?;
                Ok(commit)
            })();
            let _ = std::fs::remove_file(&index);
            result
        }
    }
}

/// Adds `.herdr-project/` and `.worktrees/` to `info/exclude` when missing;
/// the caller holds the repository lock (D4).
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
            let root = tempfile::tempdir().unwrap();
            let project = crate::project::create(root.path(), "demo", "", vec![]).unwrap();
            let _scope = crate::ledger::Scope::new(&[&project]);
            let fake = FakeRunner::new();
            fake.on("merge-base --is-ancestor", output);
            let runner = crate::ledger::RecordingRunner(&fake);
            assert!(is_ancestor(&runner, "/repo", "a", "b").is_err());
            let rows = crate::ledger::list(&project).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].kind, "command-failed");
        }

        let root = tempfile::tempdir().unwrap();
        let project = crate::project::create(root.path(), "demo", "", vec![]).unwrap();
        let _scope = crate::ledger::Scope::new(&[&project]);
        let fake = FakeRunner::new();
        fake.on_fn(|_| true, |_| Err(anyhow::anyhow!("could not spawn git")));
        assert!(is_ancestor(&crate::ledger::RecordingRunner(&fake), "/repo", "a", "b").is_err());
        assert_eq!(crate::ledger::list(&project).unwrap().len(), 1);
    }

    #[test]
    fn absent_branch_is_an_answer_but_a_broken_repository_is_not() {
        let (_dir, repo) = repo_with_commit();
        let root = tempfile::tempdir().unwrap();
        let project = crate::project::create(root.path(), "demo", "", vec![]).unwrap();
        let _scope = crate::ledger::Scope::new(&[&project]);
        let runner = crate::ledger::RecordingRunner(&RealRunner);
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
        assert!(crate::ledger::list(&project).unwrap().is_empty());
        assert!(branch_head(&runner, &root.path().to_string_lossy(), "main").is_err());
        assert_eq!(crate::ledger::list(&project).unwrap().len(), 1);
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

    fn commit_file_on_branch(
        runner: &dyn Runner,
        repo: &str,
        branch: &str,
        relative_path: &str,
        contents: &[u8],
        message: &str,
    ) -> Result<String> {
        let text = std::str::from_utf8(contents).with_context(|| {
            format!("{relative_path} is not text; only text files are committed")
        })?;
        let lock = lock(runner, repo)?;
        let head = rev_parse(runner, repo, &branch_ref(branch))
            .with_context(|| format!("branch_missing: `{branch}` does not exist"))?;
        commit_files_locked(
            runner,
            Path::new(repo),
            branch,
            &[(relative_path, text)],
            message,
            &head,
            &lock.common_dir.join("herdr-ade-tmp"),
        )
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
