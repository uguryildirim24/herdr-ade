//! Git reads used by pile review and repository inspection.
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::runner::{Cmd, Output, Runner};

const GIT_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Git<'a> {
    pub runner: &'a dyn Runner,
    pub repo: PathBuf,
}

impl<'a> Git<'a> {
    pub fn new(runner: &'a dyn Runner, repo: impl Into<PathBuf>) -> Self {
        Git {
            runner,
            repo: repo.into(),
        }
    }

    fn cmd_in(&self, dir: &Path, args: &[&str]) -> Cmd {
        Cmd::new("git", GIT_TIMEOUT)
            .arg("-C")
            .arg(dir.to_string_lossy())
            .args(args.iter().copied())
    }

    pub fn output_in(&self, dir: &Path, args: &[&str]) -> Result<Output> {
        self.runner.run(&self.cmd_in(dir, args))
    }

    /// Trimmed stdout of a successful git call in `dir`.
    pub fn run_in(&self, dir: &Path, args: &[&str]) -> Result<String> {
        let out = self.output_in(dir, args)?;
        if !out.success() {
            bail!("`git {}` failed: {}", args.join(" "), out.error_text());
        }
        Ok(out.stdout.trim_end_matches('\n').to_string())
    }

    pub fn run(&self, args: &[&str]) -> Result<String> {
        self.run_in(&self.repo, args)
    }

    /// The commit a branch points at, or `None` when it does not exist.
    pub fn branch_head(&self, branch: &str) -> Result<Option<String>> {
        crate::git::branch_head(self.runner, &self.repo.to_string_lossy(), branch)
    }

    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        crate::git::is_ancestor(
            self.runner,
            &self.repo.to_string_lossy(),
            ancestor,
            descendant,
        )
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

    /// The tree of merging `other` into `into`, written nowhere. Uses
    /// `git merge-tree --write-tree` (git 2.38+): exit 1 with conflict
    /// diagnostics is a conflict; other failures are not. The first
    /// output line on success is the tree object id.
    pub fn merge_tree(&self, into: &str, other: &str) -> Result<String> {
        let out = self
            .runner
            .run(&self.cmd_in(&self.repo, &["merge-tree", "--write-tree", into, other]))?;
        if out.merge_tree_conflict() {
            return Err(crate::refusal::error(format!(
                "merge_conflict: {other} does not merge cleanly into {into}"
            )));
        }
        if !out.success() {
            bail!(
                "`git merge-tree --write-tree {into} {other}` failed: {}",
                out.error_text()
            );
        }
        out.stdout
            .lines()
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .with_context(|| {
                format!("`git merge-tree --write-tree {into} {other}` printed no tree")
            })
    }

    /// A two-parent merge commit for `tree`, made without touching any
    /// checkout. The caller updates the branch ref under the lock.
    pub fn commit_tree(
        &self,
        tree: &str,
        first: &str,
        second: &str,
        message: &str,
    ) -> Result<String> {
        self.run(&[
            "commit-tree",
            tree,
            "-p",
            first,
            "-p",
            second,
            "-m",
            message,
        ])
    }
}
