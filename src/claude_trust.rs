//! Answer only a managed Claude lane's own repository trust dialog. Claude
//! persists the decision itself; ADE never writes its shared config file.
use anyhow::Result;

use crate::herdr::Herdr;
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::thread::{self, Kind, Thread};

fn eligible(ctx: &Ctx, project: &Project, t: &Thread, cwd: &str, screen: &str) -> bool {
    if t.launch.kind != "claude"
        || t.is_remote()
        || t.kind != Kind::Worktree
        || t.repo.is_empty()
        || t.worktree_path.is_empty()
        || t.partial.is_some()
        || t.launch.brief_hash.is_empty()
        || !crate::events::artifact_path(project, &t.launch.brief_hash).is_file()
        || t.thread_dir != thread::thread_dir(&t.worktree_path, &project.slug, &t.id)
        || t.branch != thread::branch_name(&project.slug, &t.id, &t.title)
    {
        return false;
    }
    let Ok(repo) = std::fs::canonicalize(&t.repo) else {
        return false;
    };
    let Ok(worktree) = std::fs::canonicalize(&t.worktree_path) else {
        return false;
    };
    // A path that resolves inside the right repo is not enough: ADE places
    // worktrees at this exact location and registers their gitdir there.
    if worktree != repo.join(".worktrees").join(&t.id)
        || std::fs::canonicalize(cwd).ok().as_ref() != Some(&worktree)
        || std::fs::canonicalize(&t.cwd).ok().as_ref() != Some(&worktree)
    {
        return false;
    }
    let marker = worktree.join(".git");
    if !std::fs::symlink_metadata(&marker).is_ok_and(|m| m.file_type().is_file()) {
        return false;
    }
    let Ok(gitdir) = std::fs::read_to_string(marker) else {
        return false;
    };
    let Some(gitdir) = gitdir.trim().strip_prefix("gitdir: ") else {
        return false;
    };
    let Ok(common) = std::fs::canonicalize(repo.join(".git").join("worktrees")) else {
        return false;
    };
    if std::fs::canonicalize(gitdir).ok().as_deref() != Some(common.join(&t.id).as_path()) {
        return false;
    }
    // Only a canonical root explicitly listed in a project's PROJECT.md is
    // eligible, even if the current project record happens to name the repo.
    if !project::list_slugs(&ctx.root).iter().any(|slug| {
        Project::load(&ctx.root, slug)
            .ok()
            .and_then(|p| p.read_project_md().ok())
            .is_some_and(|(settings, _)| {
                settings
                    .repos
                    .iter()
                    .any(|r| std::fs::canonicalize(&r.path).ok().as_ref() == Some(&repo))
            })
    }) {
        return false;
    }
    // The dialog must actually name this worktree, not just its repo root or
    // a different folder. Resolve the displayed path (macOS may render /var
    // where canonicalize returns /private/var).
    screen.contains("Trust this folder?")
        && screen.lines().any(|line| {
            let path = line.trim().strip_prefix("❯ ").unwrap_or(line.trim());
            std::fs::canonicalize(path).ok().as_ref() == Some(&worktree)
        })
        && screen.contains("1. Yes")
}

/// Once per start, only while the agent is blocked on its exact trust dialog.
/// A missing screen or identity evidence is a refusal, not a guessed approval.
pub(crate) fn answer(ctx: &Ctx, project: &Project, t: &Thread, herdr: &Herdr<'_>) -> Result<bool> {
    if t.trust_answered || t.launch.kind != "claude" || t.is_remote() {
        return Ok(false);
    }
    let screen = herdr.pane_read_text(&t.pane_id, "visible")?;
    let cwd = herdr.pane_cwd(&t.pane_id)?;
    if !eligible(ctx, project, t, &cwd, &screen) {
        return Ok(false);
    }
    // Persist before sending: a delayed next poll must not answer twice.
    thread::update(project, &t.id, |record| record.trust_answered = true)?;
    herdr.pane_submit_text(&t.pane_id, "1")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Repo;
    use crate::runner::fake::{FakeRunner, ok};
    use crate::thread::Status;

    #[test]
    fn only_the_managed_worktree_and_its_live_pane_can_be_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "initial",
        ]);
        let project = project::create(
            tmp.path(),
            "sample",
            "test",
            vec![Repo {
                path: repo.to_string_lossy().into_owned(),
                ..Repo::default()
            }],
        )
        .unwrap();
        let id = "t-0001";
        let worktree = repo.join(".worktrees").join(id);
        let branch = thread::branch_name(&project.slug, id, "task");
        git(&[
            "worktree",
            "add",
            "-qb",
            &branch,
            worktree.to_str().unwrap(),
        ]);
        let mut t = Thread {
            id: id.into(),
            title: "task".into(),
            status: Status::Starting,
            kind: Kind::Worktree,
            repo: repo.to_string_lossy().into_owned(),
            worktree_path: worktree.to_string_lossy().into_owned(),
            cwd: worktree.to_string_lossy().into_owned(),
            branch,
            thread_dir: thread::thread_dir(&worktree.to_string_lossy(), &project.slug, id),
            ..Thread::default()
        };
        t.launch.kind = "claude".into();
        t.launch.brief_hash = thread::store_artifact(&project, b"brief").unwrap();
        let env = crate::paths::Env::for_test(tmp.path(), &[]);
        let ctx = Ctx {
            env: &env,
            root: tmp.path().to_path_buf(),
            config_dir: tmp.path().join("config"),
            runner: &crate::runner::RealRunner,
            detached_ticker: false,
        };
        let screen = format!(
            "Trust this folder?\n{}\n  1. Yes\n  2. No",
            worktree.display()
        );
        assert!(eligible(&ctx, &project, &t, &t.cwd, &screen));
        let saved = thread::allocate(&project, |record| {
            *record = t.clone();
            record.pane_id = "w1:p2".into();
        })
        .unwrap();
        let fake = FakeRunner::new();
        fake.on("pane read", ok(&screen));
        fake.on("pane get", ok(&format!(r#"{{"result":{{"pane":{{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","cwd":"{}"}}}}}}"#, worktree.display())));
        fake.on("pane send-text", ok(r#"{"result":{}}"#));
        fake.on("pane send-keys", ok(r#"{"result":{}}"#));
        let live_ctx = Ctx {
            env: &env,
            root: tmp.path().to_path_buf(),
            config_dir: tmp.path().join("config"),
            runner: &fake,
            detached_ticker: false,
        };
        let herdr = Herdr::new("herdr", "", &fake);
        assert!(answer(&live_ctx, &project, &saved, &herdr).unwrap());
        let answered = thread::load(&project, id).unwrap();
        assert!(answered.trust_answered);
        assert!(!answer(&live_ctx, &project, &answered, &herdr).unwrap());
        assert_eq!(fake.count("pane send-text"), 1);
        assert_eq!(fake.count("pane send-keys"), 1);
        assert!(!eligible(
            &ctx,
            &project,
            &t,
            repo.to_str().unwrap(),
            &screen
        ));
        assert!(!eligible(
            &ctx,
            &project,
            &t,
            &t.cwd,
            "Trust this folder?\n  1. Yes"
        ));
        t.worktree_path = repo.join("other").to_string_lossy().into_owned();
        assert!(!eligible(&ctx, &project, &t, &t.cwd, &screen));
        t.worktree_path = worktree.to_string_lossy().into_owned();
        let marker = worktree.join(".git");
        let target = std::fs::read_to_string(&marker).unwrap();
        std::fs::remove_file(&marker).unwrap();
        std::os::unix::fs::symlink(repo.join(".git"), &marker).unwrap();
        assert!(!eligible(&ctx, &project, &t, &t.cwd, &screen));
        std::fs::remove_file(&marker).unwrap();
        std::fs::write(&marker, target).unwrap();
        let unregistered = tmp.path().join("other-repo");
        std::fs::create_dir(&unregistered).unwrap();
        let run = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&unregistered)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&["init", "-q"]);
        run(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "initial",
        ]);
        let other_wt = unregistered.join(".worktrees").join(id);
        run(&[
            "worktree",
            "add",
            "-qb",
            &t.branch,
            other_wt.to_str().unwrap(),
        ]);
        t.repo = unregistered.to_string_lossy().into_owned();
        t.cwd = other_wt.to_string_lossy().into_owned();
        t.worktree_path = t.cwd.clone();
        t.thread_dir = thread::thread_dir(&t.worktree_path, &project.slug, id);
        let other_screen = format!("Trust this folder?\n{}\n  1. Yes", other_wt.display());
        assert!(!eligible(&ctx, &project, &t, &t.cwd, &other_screen));
    }
}
