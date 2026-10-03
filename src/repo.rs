//! Repository-bound Git execution and interpretation.
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::runner::{Cmd, Output, Runner};

const GIT_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Git<'a> {
    runner: &'a dyn Runner,
    repo: PathBuf,
    timeout: Duration,
}

impl<'a> Git<'a> {
    pub fn new(runner: &'a dyn Runner, repo: impl Into<PathBuf>) -> Self {
        Git {
            runner,
            repo: repo.into(),
            timeout: GIT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Raw output, including nonzero exits for command-specific interpretation.
    /// A timeout is never an ordinary negative answer or conflict.
    pub fn output(&self, args: &[&str]) -> Result<Output> {
        let out = self.runner.run(
            &Cmd::new("git", self.timeout)
                .env("LC_ALL", "C")
                .args(["-C", &self.repo.to_string_lossy()])
                .args(args.iter().copied()),
        )?;
        if out.timed_out {
            bail!(
                "git {}: timed out; repo activity at timeout: {}",
                args.join(" "),
                crate::git::repo_activity(&self.repo.to_string_lossy())
            );
        }
        Ok(out)
    }

    /// Successful stdout without trimming: porcelain spaces and NULs matter.
    pub fn stdout(&self, args: &[&str]) -> Result<String> {
        let out = self.output(args)?;
        if !out.success() {
            bail!(
                "`git {}` failed: exit={:?}, {}",
                args.join(" "),
                out.code,
                out.error_text()
            );
        }
        Ok(out.stdout)
    }

    pub fn run_in(&self, dir: &Path, args: &[&str]) -> Result<String> {
        Git::new(self.runner, dir)
            .with_timeout(self.timeout)
            .run(args)
    }

    /// Successful stdout with only the trailing line terminator removed.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        Ok(self.stdout(args)?.trim_end_matches('\n').to_string())
    }

    /// Exact local branch head; absence is None, not a failed query.
    /// Even a supplied `refs/` prefix stays within the local-branch namespace.
    pub fn branch_head(&self, branch: &str) -> Result<Option<String>> {
        let want = format!("refs/heads/{branch}");
        let rows = self.run(&["for-each-ref", "--format=%(refname) %(objectname)", &want])?;
        Ok(rows.lines().find_map(|row| {
            let (name, sha) = row.split_once(' ')?;
            (name == want).then(|| sha.to_string())
        }))
    }

    /// Git's explicit not-a-repository diagnostic is absence; other failures fail.
    pub fn is_repository(&self) -> Result<bool> {
        let out = self.output(&["rev-parse", "--show-toplevel"])?;
        match out.code {
            Some(0) => Ok(true),
            Some(128) if out.stderr.starts_with("fatal: not a git repository") => Ok(false),
            _ => bail!(
                "could not inspect repository: exit={:?}, {}",
                out.code,
                out.error_text()
            ),
        }
    }

    /// Whether the two commits contain different file trees (empty commits do not count).
    pub fn trees_differ(&self, base: &str, sha: &str) -> Result<bool> {
        let base_tree = self.run(&["rev-parse", &format!("{base}^{{tree}}")])?;
        let sha_tree = self.run(&["rev-parse", &format!("{sha}^{{tree}}")])?;
        Ok(base_tree != sha_tree)
    }

    /// Zero is yes; one without diagnostics is no. All other outcomes fail.
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let args = ["merge-base", "--is-ancestor", ancestor, descendant];
        let out = self.output(&args)?;
        out.boolean_answer().with_context(|| {
            format!(
                "`git {}` failed: exit={:?}, {}",
                args.join(" "),
                out.code,
                out.error_text()
            )
        })
    }

    /// The worktree that has `branch` checked out, if any.
    pub fn checkout_of(&self, branch: &str) -> Result<Option<PathBuf>> {
        let text = self.run(&["worktree", "list", "--porcelain"])?;
        let want = format!("branch refs/heads/{branch}");
        let mut current: Option<PathBuf> = None;
        for line in text.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                current = Some(PathBuf::from(path));
            } else if line == want {
                return Ok(current);
            }
        }
        Ok(None)
    }

    /// `git status --porcelain` paths in `dir` (untracked included).
    pub fn dirty_paths(&self, dir: &Path) -> Result<Vec<String>> {
        let text = self.run_in(dir, &["status", "--porcelain", "--untracked-files=all"])?;
        Ok(text
            .lines()
            .filter(|l| l.len() > 3)
            .map(|l| l[3..].trim().trim_matches('"').to_string())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, fail, ok, timeout};

    #[test]
    fn raw_porcelain_and_caller_timeout_survive_the_boundary() {
        let runner = FakeRunner::new();
        let status = " M leading space\0!! ignored\nname\0";
        runner.on("status", ok(status));
        let git = Git::new(&runner, "/repo").with_timeout(Duration::from_secs(7));
        assert_eq!(git.stdout(&["status", "-z"]).unwrap(), status);
        assert_eq!(runner.calls.borrow()[0].timeout, Duration::from_secs(7));
        runner.on("show", ok(" content \n"));
        assert_eq!(git.run(&["show"]).unwrap(), " content ");
    }

    #[test]
    fn timeout_is_neither_success_absence_nor_conflict() {
        let runner = FakeRunner::new();
        runner.on(
            "git",
            Output {
                code: Some(0),
                stdout: "CONFLICT (content)".into(),
                ..timeout()
            },
        );
        let git = Git::new(&runner, "/repo");
        for error in [
            git.output(&["status"]).unwrap_err(),
            git.run(&["rev-parse", "HEAD"]).unwrap_err(),
            git.branch_head("main").unwrap_err(),
            git.is_repository().unwrap_err(),
        ] {
            assert!(
                error
                    .to_string()
                    .contains("timed out; repo activity at timeout:")
            );
            assert!(!crate::refusal::is(&error));
        }
    }

    #[test]
    fn bounded_capture_failure_is_not_a_parseable_git_answer() {
        use crate::runner::{Capture, IncompleteOutput, StreamEvidence};
        let runner = FakeRunner::new();
        runner.on_fn(
            |_| true,
            |_| {
                Err(IncompleteOutput {
                    capture: Capture {
                        output: ok("tree-id\n"),
                        stdout: StreamEvidence {
                            seen: 100,
                            retained: 8,
                            complete: true,
                            ..Default::default()
                        },
                        stderr: StreamEvidence::default(),
                    },
                }
                .into())
            },
        );
        let git = Git::new(&runner, "/repo");
        for error in [
            git.run(&["status"]).unwrap_err(),
            git.is_ancestor("a", "b").unwrap_err(),
        ] {
            assert!(error.downcast_ref::<IncompleteOutput>().is_some());
        }
    }

    #[test]
    fn only_explicit_not_a_repository_is_absence() {
        for (output, expected) in [
            (ok("/repo\n"), Some(true)),
            (
                fail(
                    128,
                    "fatal: not a git repository (or any of the parent directories): .git",
                ),
                Some(false),
            ),
            (fail(128, "fatal: detected dubious ownership"), None),
            (
                fail(
                    128,
                    "fatal: detected dubious ownership in repository at '/tmp/fatal: not a git repository'",
                ),
                None,
            ),
            (
                fail(
                    128,
                    "fatal: cannot change to 'fatal: not a git repository': No such file or directory",
                ),
                None,
            ),
            (fail(1, "object database error"), None),
            (timeout(), None),
            (Output::default(), None),
        ] {
            let runner = FakeRunner::new();
            runner.on("rev-parse", output);
            assert_eq!(Git::new(&runner, "/repo").is_repository().ok(), expected);
        }
    }
}
