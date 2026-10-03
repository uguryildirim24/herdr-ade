//! Answer only a managed Claude lane's own repository trust dialog. Claude
//! persists the decision itself; ADE never writes its shared config file.
use anyhow::Result;
use std::path::Path;

/// Project folders lack the managed-worktree proof required to answer a dialog.
/// Only a readable, valid Claude config can establish that a folder is untrusted.
pub(crate) fn check_folder(ctx: &Ctx, kind: &str, remote: bool, folder: &Path) -> Result<()> {
    if kind != "claude" || remote {
        return Ok(());
    }
    let Ok(bytes) = std::fs::read(ctx.env.home.join(".claude.json")) else {
        return Ok(());
    };
    let Ok(config) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(());
    };
    let trusted = folder.ancestors().any(|path| {
        config
            .get("projects")
            .and_then(|projects| projects.get(path.to_str()?))
            .and_then(|project| project.get("hasTrustDialogAccepted"))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    });
    if !trusted {
        return Err(crate::refusal::error(
            format!(
                "claude_folder_untrusted: {} is not trusted by Claude Code; the agent would stop on its trust question",
                folder.display()
            ),
            "Run `claude` once in the repository and answer yes to the trust question, or trust it in your Claude settings",
        ));
    }
    Ok(())
}

use crate::herdr::Herdr;
use crate::paths::Ctx;
use crate::project::Project;
use crate::thread::{self, Kind, Thread};

#[derive(Debug, PartialEq)]
enum Dialog {
    Old,
    New,
}

fn eligible(ctx: &Ctx, project: &Project, t: &Thread, cwd: &str, screen: &str) -> Option<Dialog> {
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
        return None;
    }
    let repo = std::fs::canonicalize(&t.repo).ok()?;
    let worktree = std::fs::canonicalize(&t.worktree_path).ok()?;
    // A path that resolves inside the right repo is not enough: ADE places
    // worktrees at this exact location and registers their gitdir there.
    if worktree != repo.join(".worktrees").join(&t.id)
        || std::fs::canonicalize(cwd).ok().as_ref() != Some(&worktree)
        || std::fs::canonicalize(&t.cwd).ok().as_ref() != Some(&worktree)
    {
        return None;
    }
    let marker = worktree.join(".git");
    if !std::fs::symlink_metadata(&marker).is_ok_and(|m| m.file_type().is_file()) {
        return None;
    }
    let gitdir = std::fs::read_to_string(&marker).ok()?;
    let gitdir = gitdir.trim().strip_prefix("gitdir: ")?;
    let common = std::fs::canonicalize(repo.join(".git").join("worktrees")).ok()?;
    let registered_gitdir = common.join(&t.id);
    if std::fs::canonicalize(gitdir).ok().as_deref() != Some(registered_gitdir.as_path())
        || std::fs::read_to_string(registered_gitdir.join("gitdir"))
            .ok()
            .and_then(|path| std::fs::canonicalize(path.trim()).ok())
            .as_deref()
            != Some(marker.as_path())
        || std::fs::read_to_string(registered_gitdir.join("HEAD"))
            .ok()
            .is_none_or(|head| head.trim() != format!("ref: refs/heads/{}", t.branch))
    {
        return None;
    }
    // The lane's own project must still register this repo. Another project's
    // registration does not authorize this project's lane.
    if !Project::load(&ctx.root, &project.slug)
        .ok()
        .and_then(|current| current.read_project_md().ok())
        .is_some_and(|(settings, _)| {
            settings
                .repos
                .iter()
                .any(|r| std::fs::canonicalize(&r.path).ok().as_ref() == Some(&repo))
        })
    {
        return None;
    }
    // The dialog must actually name this worktree, not just its repo root or
    // a different folder. Resolve the displayed path (macOS may render /var
    // where canonicalize returns /private/var).
    if !screen.lines().any(|line| {
        let path = line.trim().strip_prefix("❯ ").unwrap_or(line.trim());
        std::fs::canonicalize(path).ok().as_ref() == Some(&worktree)
    }) {
        return None;
    }
    if screen.contains("Trust this folder?") && screen.contains("1. Yes") {
        Some(Dialog::Old)
    } else if screen.contains("Quick safety check: Is this a project you created or one you trust?")
        && screen.contains("Yes, I trust this folder")
    {
        Some(Dialog::New)
    } else {
        None
    }
}

/// Once per start, only while the agent is blocked on its exact trust dialog.
/// A missing screen or identity evidence is a refusal, not a guessed approval.
pub(crate) fn answer(ctx: &Ctx, project: &Project, t: &Thread, herdr: &Herdr<'_>) -> Result<bool> {
    if t.trust_answered || t.launch.kind != "claude" || t.is_remote() {
        return Ok(false);
    }
    let screen = herdr.pane_read_text(&t.pane_id, "visible")?;
    let cwd = herdr.pane_cwd(&t.pane_id)?;
    let Some(dialog) = eligible(ctx, project, t, &cwd, &screen) else {
        return Ok(false);
    };
    // Persist before sending: a delayed next poll must not answer twice.
    thread::update(project, &t.id, |record| record.trust_answered = true)?;
    match dialog {
        Dialog::Old => herdr.pane_submit_text(&t.pane_id, "1")?,
        Dialog::New => {
            herdr.pane_send_keys(&t.pane_id, "Down")?;
            // Down can return before Claude redraws. Wait only for the exact
            // Yes highlight in the visible screen, never in scrollback.
            if let Err(error) = herdr.call(
                &[
                    "pane",
                    "wait-output",
                    &t.pane_id,
                    "--source",
                    "visible",
                    "--regex",
                    r"(?m)^[ \t]*❯ Yes, I trust this folder[ \t]*\r?$",
                    "--timeout",
                    "1000",
                ],
                std::time::Duration::from_secs(6),
            ) {
                if error.code == "timeout" {
                    return Ok(true);
                }
                return Err(error.into());
            }
            let screen = herdr.pane_read_text(&t.pane_id, "visible")?;
            if screen
                .lines()
                .any(|line| line.trim() == "❯ Yes, I trust this folder")
            {
                herdr.pane_send_keys(&t.pane_id, "Enter")?;
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{self, Repo};
    use crate::runner::fake::{FakeRunner, fail, ok};
    use crate::thread::Status;

    #[test]
    fn project_folder_trust_inherits_from_parent_and_only_refuses_claude() {
        let home = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(home.path(), &[]);
        let ctx = Ctx {
            env: &env,
            root: home.path().into(),
            config_dir: home.path().join("cfg"),
            runner: &crate::runner::RealRunner,
            detached_ticker: false,
        };
        let repo = home.path().join("project");
        let folder = repo.join("threads/t-0001");
        let config = home.path().join(".claude.json");
        assert!(check_folder(&ctx, "claude", false, &folder).is_ok());
        std::fs::write(&config, "not json").unwrap();
        assert!(check_folder(&ctx, "claude", false, &folder).is_ok());
        std::fs::write(&config, r#"{"projects":{}}"#).unwrap();
        let refusal = check_folder(&ctx, "claude", false, &folder).unwrap_err();
        assert!(format!("{refusal:#}").contains("claude_folder_untrusted"));
        assert!(format!("{refusal:#}").contains(&folder.display().to_string()));
        assert!(check_folder(&ctx, "pi", false, &folder).is_ok());
        assert!(check_folder(&ctx, "claude", true, &folder).is_ok());
        std::fs::write(&config, serde_json::json!({"projects": {(repo.to_str().unwrap()): {"hasTrustDialogAccepted": true}}}).to_string()).unwrap();
        assert!(check_folder(&ctx, "claude", false, &folder).is_ok());
    }

    fn managed_lane() -> (tempfile::TempDir, Project, Thread) {
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
        (tmp, project, t)
    }

    fn safety_screen(path: &str) -> String {
        format!(
            " Accessing workspace:\n\n {path}\n\n Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work\n from your team). If not, take a moment to review what's in this folder first.\n\n Claude Code'll be able to read, edit, and execute files here.\n\n Security guide\n\n ❯ No, exit\n   Yes, I trust this folder"
        )
    }

    #[test]
    fn safety_check_only_enters_after_verified_yes_for_the_exact_worktree() {
        for (right_path, highlight) in [
            (true, "yes"),
            (false, "yes"),
            (true, "no"),
            (true, "other"),
            (true, "timeout"),
        ] {
            let (tmp, project, t) = managed_lane();
            let saved = thread::allocate(&project, |record| {
                *record = t.clone();
                record.pane_id = "w1:p2".into();
            })
            .unwrap();
            let screen = safety_screen(if right_path { &t.cwd } else { &t.repo });
            let readback = match highlight {
                "yes" => screen.replace(" ❯ No, exit\n   Yes", "   No, exit\n ❯ Yes"),
                "no" => screen.clone(),
                _ => "❯ Something else\nYes, I trust this folder".into(),
            };
            let screens =
                std::cell::RefCell::new(std::collections::VecDeque::from([screen, readback]));
            let fake = FakeRunner::new();
            fake.on_fn(
                |cmd| cmd.display().contains("pane read"),
                move |_| Ok(ok(&screens.borrow_mut().pop_front().unwrap())),
            );
            fake.on("pane get", ok(&format!(r#"{{"result":{{"pane":{{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1","cwd":"{}"}}}}}}"#, t.cwd)));
            fake.on_fn(
                |cmd| cmd.display().contains("pane wait-output"),
                move |cmd| {
                    assert_eq!(
                        &cmd.args[cmd.args.len() - 9..],
                        &[
                            "pane",
                            "wait-output",
                            "w1:p2",
                            "--source",
                            "visible",
                            "--regex",
                            r"(?m)^[ \t]*❯ Yes, I trust this folder[ \t]*\r?$",
                            "--timeout",
                            "1000",
                        ][..]
                    );
                    assert_eq!(cmd.timeout, std::time::Duration::from_secs(6));
                    Ok(if highlight == "timeout" {
                        fail(1, r#"{"error":{"code":"timeout","message":"timed out waiting for output match"}}"#)
                    } else {
                        ok(r#"{"result":{}}"#)
                    })
                },
            );
            let project_for_key = project.clone();
            let id = t.id.clone();
            fake.on_fn(
                |cmd| cmd.display().contains("pane send-keys"),
                move |_| {
                    assert!(thread::load(&project_for_key, &id).unwrap().trust_answered);
                    Ok(ok(r#"{"result":{}}"#))
                },
            );
            let env = crate::paths::Env::for_test(tmp.path(), &[]);
            let ctx = Ctx {
                env: &env,
                root: tmp.path().to_path_buf(),
                config_dir: tmp.path().join("config"),
                runner: &fake,
                detached_ticker: false,
            };
            let herdr = Herdr::new("herdr", "test.sock", &fake);
            assert_eq!(answer(&ctx, &project, &saved, &herdr).unwrap(), right_path);
            let answered = thread::load(&project, &t.id).unwrap();
            assert_eq!(answered.trust_answered, right_path);
            assert!(!answer(&ctx, &project, &answered, &herdr).unwrap());
            assert_eq!(fake.count("pane send-text"), 0);
            let calls = fake.calls.borrow();
            let actions: Vec<_> = calls
                .iter()
                .filter_map(|cmd| {
                    if cmd.display().contains("pane read") {
                        Some("read")
                    } else if cmd.display().contains("pane wait-output") {
                        Some("wait")
                    } else if cmd.display().contains("pane send-keys") {
                        Some(cmd.args.last().unwrap().as_str())
                    } else {
                        None
                    }
                })
                .collect();
            let expected = if !right_path {
                vec!["read", "read"]
            } else if highlight == "yes" {
                vec!["read", "Down", "wait", "read", "Enter"]
            } else if highlight == "timeout" {
                vec!["read", "Down", "wait"]
            } else {
                vec!["read", "Down", "wait", "read"]
            };
            assert_eq!(
                actions, expected,
                "right_path={right_path}, highlight={highlight}"
            );
        }
    }

    #[test]
    fn only_the_managed_worktree_and_its_live_pane_can_be_trusted() {
        let (tmp, project, mut t) = managed_lane();
        let repo = std::path::PathBuf::from(&t.repo);
        let worktree = std::path::PathBuf::from(&t.worktree_path);
        let id = t.id.clone();
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
        assert_eq!(
            eligible(&ctx, &project, &t, &t.cwd, &screen),
            Some(Dialog::Old)
        );
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
        let herdr = Herdr::new("herdr", "test.sock", &fake);
        assert!(answer(&live_ctx, &project, &saved, &herdr).unwrap());
        let answered = thread::load(&project, &id).unwrap();
        assert!(answered.trust_answered);
        assert!(!answer(&live_ctx, &project, &answered, &herdr).unwrap());
        assert_eq!(fake.count("pane send-text"), 1);
        assert_eq!(fake.count("pane send-keys"), 1);
        assert!(eligible(&ctx, &project, &t, repo.to_str().unwrap(), &screen).is_none());
        assert!(eligible(&ctx, &project, &t, &t.cwd, "Trust this folder?\n  1. Yes").is_none());
        t.worktree_path = repo.join("other").to_string_lossy().into_owned();
        assert!(eligible(&ctx, &project, &t, &t.cwd, &screen).is_none());
        t.worktree_path = worktree.to_string_lossy().into_owned();
        let marker = worktree.join(".git");
        let target = std::fs::read_to_string(&marker).unwrap();
        std::fs::remove_file(&marker).unwrap();
        std::os::unix::fs::symlink(repo.join(".git"), &marker).unwrap();
        assert!(eligible(&ctx, &project, &t, &t.cwd, &screen).is_none());
        std::fs::remove_file(&marker).unwrap();
        std::fs::write(&marker, target).unwrap();
        let gitdir = repo.join(".git/worktrees").join(&id);
        let head = std::fs::read_to_string(gitdir.join("HEAD")).unwrap();
        std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/unrelated\n").unwrap();
        assert!(eligible(&ctx, &project, &t, &t.cwd, &screen).is_none());
        std::fs::write(gitdir.join("HEAD"), head).unwrap();
        let backlink = std::fs::read_to_string(gitdir.join("gitdir")).unwrap();
        std::fs::write(
            gitdir.join("gitdir"),
            repo.join(".git").display().to_string(),
        )
        .unwrap();
        assert!(eligible(&ctx, &project, &t, &t.cwd, &screen).is_none());
        std::fs::write(gitdir.join("gitdir"), backlink).unwrap();
        assert_eq!(
            eligible(&ctx, &project, &t, &t.cwd, &screen),
            Some(Dialog::Old)
        );
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
        let other_wt = unregistered.join(".worktrees").join(&id);
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
        t.thread_dir = thread::thread_dir(&t.worktree_path, &project.slug, &id);
        let other_screen = format!("Trust this folder?\n{}\n  1. Yes", other_wt.display());
        assert!(eligible(&ctx, &project, &t, &t.cwd, &other_screen).is_none());
        // A different project's registration must not authorize this lane.
        project::create(
            tmp.path(),
            "second",
            "test",
            vec![Repo {
                path: unregistered.to_string_lossy().into_owned(),
                ..Repo::default()
            }],
        )
        .unwrap();
        assert!(eligible(&ctx, &project, &t, &t.cwd, &other_screen).is_none());
    }
}
