//! Synthetic box lanes seal through the CLI without detaching visible panes.

#![cfg(unix)]

use std::os::unix::fs::{PermissionsExt, symlink};
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
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap(),
    )
}

#[test]
fn done_and_waiting_keep_the_visible_box_lanes_parent() {
    for mode in [
        "waiting",
        "default",
        "explicit",
        "allowed",
        "forbidden",
        "reviewer",
        "wrong-sha",
        "outside",
        "symlink",
    ] {
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
        let repo = home.path().join("Work Projects/lane");
        let subdir = repo.join("src");
        std::fs::create_dir_all(&subdir).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(subdir.join("allowed.rs"), "initial\n").unwrap();
        git(&repo, &["add", "src"]);
        git(&repo, &["commit", "-qm", "Fixture"]);
        let base = String::from_utf8(git(&repo, &["rev-parse", "HEAD"]).stdout).unwrap();
        let base = base.trim();
        if matches!(mode, "allowed" | "forbidden" | "reviewer") {
            let file = if mode == "allowed" {
                "src/allowed.rs"
            } else {
                "outside file.txt"
            };
            std::fs::write(repo.join(file), "changed\n").unwrap();
            git(&repo, &["add", file]);
            git(&repo, &["commit", "-qm", "Lane change"]);
            if mode == "forbidden" {
                // A net-empty change still touched an out-of-scope file in a commit.
                git(&repo, &["rm", file]);
                git(&repo, &["commit", "-qm", "Undo outside change"]);
            }
        }
        let sha = String::from_utf8(git(&repo, &["rev-parse", "HEAD"]).stdout).unwrap();
        let sha = sha.trim();
        let runtime = repo.join(".herdr-project/demo-t-0001");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(repo.join(".git/info/exclude"), "/.herdr-project/\n").unwrap();
        std::fs::write(runtime.join("report.md"), "Recorded report.\n").unwrap();
        std::fs::write(runtime.join("explicit report.md"), "Explicit report.\n").unwrap();
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
                 role = '{}'\n{}\
                 box_repo = '{}'\nbox_worktree = '{}'\nbrief_commit = '{base}'\n\
                 branch = 'lane-fixture'\npublish_url = '{}'\n\
                 [recipient]\npane = 'w1:p1'\ncoordinator_attempt = 1\n",
                if mode == "reviewer" {
                    "reviewer"
                } else {
                    "lane"
                },
                if matches!(mode, "allowed" | "forbidden" | "reviewer") {
                    "paths = ['src/**']\n"
                } else {
                    ""
                },
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
        ).unwrap();
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        let outside = repo.with_file_name("lane-outside");
        std::fs::create_dir(&outside).unwrap();
        let cwd = match mode {
            "outside" => outside.clone(),
            "symlink" => {
                let link = repo.join("escape");
                symlink(&outside, &link).unwrap();
                link
            }
            _ => subdir,
        };
        let args = match mode {
            "waiting" | "outside" | "symlink" => vec!["waiting", "Missing fixture input"],
            "explicit" => vec![
                "done",
                "--report",
                ".herdr-project/demo-t-0001/explicit report.md",
                "--sha",
                sha,
            ],
            "wrong-sha" => vec!["done", "--sha", "deadbeef"],
            _ => vec!["done"],
        };
        let output = Command::new(BIN)
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .env("HERDR_BIN_PATH", &herdr)
            .env("HERDR_PANE_ID", "w2:p1")
            .env("HERDR_SOCKET_PATH", home.path().join("fixture.sock"))
            .env("HERDR_ADE_LAUNCH", "demo/t-0001/1/fixture")
            .current_dir(cwd)
            .args(["--root", root.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap();
        let event_path = root.join("demo/.state/events/t-0001-1-1.toml");
        let refusal = match mode {
            "forbidden" => Some("lane_paths_exceeded"),
            "wrong-sha" => Some("sha_mismatch"),
            "outside" | "symlink" => Some("bootstrap_mismatch"),
            _ => None,
        };
        if let Some(reason) = refusal {
            assert!(!output.status.success(), "{mode}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(reason), "{mode}: {stderr}");
            if mode == "forbidden" {
                assert!(
                    stderr.contains("outside file.txt") && stderr.contains("wider --paths"),
                    "{stderr}"
                );
            }
            assert!(!event_path.exists());
            assert!(
                git(&remote, &["for-each-ref", "--format=%(refname)"])
                    .stdout
                    .is_empty()
            );
            continue;
        }
        let output = success(output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("sealed"));
        let calls = std::fs::read_to_string(home.path().join("herdr-calls")).unwrap();
        assert!(calls.contains("pane process-info"), "{calls}");
        assert!(!calls.contains("--clear-token parent"), "{calls}");
        assert!(
            !calls.contains("tab close") && !calls.contains("workspace close"),
            "{calls}"
        );
        let event = std::fs::read_to_string(event_path).unwrap();
        if mode == "waiting" {
            assert!(event.contains("[payload.waiting]") && event.contains("class = \"unknown\""));
        } else {
            assert!(
                event.contains("[payload.done]") && event.contains(sha),
                "{event}"
            );
            assert!(
                event.contains(if matches!(mode, "allowed" | "reviewer") {
                    "has_changes = true"
                } else {
                    "has_changes = false"
                }),
                "{event}"
            );
            assert!(
                event.contains(if mode == "explicit" {
                    "explicit report.md"
                } else {
                    "/report.md"
                }),
                "{event}"
            );
            assert_eq!(
                String::from_utf8(git(&repo, &["rev-parse", "HEAD"]).stdout)
                    .unwrap()
                    .trim(),
                sha
            );
        }
    }
}
