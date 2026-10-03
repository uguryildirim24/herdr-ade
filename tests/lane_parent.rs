//! Synthetic box lanes seal through the CLI without detaching visible panes.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-ade");

fn success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git(cwd: &Path, args: &[&str]) -> Output {
    success(
        Command::new("git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap(),
    )
}

#[test]
fn done_and_waiting_keep_the_visible_box_lanes_parent() {
    for waiting in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        success(
            Command::new(BIN)
                .env_clear()
                .env("HOME", home.path())
                .args(["--root", root.to_str().unwrap(), "new", "demo"])
                .output()
                .unwrap(),
        );
        let repo = home.path().join("lane");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("report.md"), "Synthetic lane report.\n").unwrap();
        git(&repo, &["add", "report.md"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.com",
                "commit",
                "-qm",
                "Fixture",
            ],
        );
        let sha = git(&repo, &["rev-parse", "HEAD"]);
        let sha = String::from_utf8(sha.stdout).unwrap();
        let sha = sha.trim();
        let remote = home.path().join("remote.git");
        git(
            home.path(),
            &["init", "--bare", "-q", remote.to_str().unwrap()],
        );
        let cards = root.join("demo/.state/lanes");
        std::fs::create_dir_all(&cards).unwrap();
        std::fs::write(
            cards.join("t-0001.toml"),
            format!(
                "project = 'demo'\nthread = 't-0001'\nattempt = 1\n\
                 brief_hash = 'fixture'\nkind = 'pi'\npane_id = 'w2:p1'\n\
                 box_repo = '{}'\nbox_worktree = '{}'\nbrief_commit = '{sha}'\n\
                 branch = 'lane-fixture'\npublish_url = '{}'\n\
                 [recipient]\npane = 'w1:p1'\ncoordinator_attempt = 1\n",
                repo.display(),
                repo.display(),
                remote.display(),
            ),
        )
        .unwrap();
        let herdr = home.path().join("herdr");
        std::fs::write(
            &herdr,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/herdr-calls\"\n\
             printf '%s\\n' '{\"result\":{\"process_info\":{\"pane_id\":\"w2:p1\",\"foreground_processes\":[{\"pid\":1,\"name\":\"pi\"}]}}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        let args = if waiting {
            vec!["waiting", "Missing fixture input"]
        } else {
            vec!["done", "--report", "report.md", "--sha", sha]
        };
        let output = success(
            Command::new(BIN)
                .env_clear()
                .env("HOME", home.path())
                .env("PATH", "/usr/bin:/bin")
                .env("HERDR_BIN_PATH", &herdr)
                .env("HERDR_PANE_ID", "w2:p1")
                .env("HERDR_SOCKET_PATH", home.path().join("fixture.sock"))
                .env("HERDR_ADE_LAUNCH", "demo/t-0001/1/fixture")
                .current_dir(&repo)
                .args(["--root", root.to_str().unwrap()])
                .args(args)
                .output()
                .unwrap(),
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("sealed"));
        let calls = std::fs::read_to_string(home.path().join("herdr-calls")).unwrap();
        assert!(calls.contains("pane process-info"), "{calls}");
        assert!(!calls.contains("--clear-token parent"), "{calls}");
        assert!(
            !calls.contains("tab close") && !calls.contains("workspace close"),
            "{calls}"
        );
        let event =
            std::fs::read_to_string(root.join("demo/.state/events/t-0001-1-1.toml")).unwrap();
        assert!(event.contains(if waiting {
            "[payload.waiting]"
        } else {
            "[payload.done]"
        }));
    }
}
