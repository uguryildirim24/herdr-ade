//! End-to-end checks of the built binary with a scrubbed environment.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_herdr-ade");

fn hp(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .env_clear()
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn overturn_and_withdraw_commands_keep_history_and_ask_output_starts_with_id() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let run = |args: &[&str]| {
        Command::new(BIN)
            .env_clear()
            .env("HOME", home.path())
            .env("USER", "rolf")
            .args(["--root", root.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    };
    assert!(run(&["new", "demo"]).status.success());
    assert!(
        run(&[
            "decide",
            "I kept the words short.",
            "--class",
            "routine",
            "--project",
            "demo"
        ])
        .status
        .success()
    );
    let overturned = run(&[
        "decide",
        "overturn",
        "d-0001",
        "I want more detail.",
        "--project",
        "demo",
    ]);
    assert!(
        overturned.status.success(),
        "{}",
        String::from_utf8_lossy(&overturned.stderr)
    );
    assert!(String::from_utf8_lossy(&overturned.stdout).contains("overturned by rolf"));
    assert!(
        !run(&[
            "decide",
            "overturn",
            "d-9999",
            "No thanks.",
            "--project",
            "demo"
        ])
        .status
        .success()
    );
    let ask_args = [
        "ask",
        "May I spend\n five dollars on this check?",
        "--choice",
        "Keep it running.",
        "--choice",
        "Stop it now.",
        "--project",
        "demo",
    ];
    let asked = run(&ask_args);
    assert!(
        asked.status.success(),
        "{}",
        String::from_utf8_lossy(&asked.stderr)
    );
    let text = String::from_utf8(asked.stdout).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.starts_with("a-1 revision 1:"));
    assert!(text.contains("1. Keep it running."));
    let duplicate = run(&ask_args);
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("ask_duplicate: `a-1`"));
    let withdrawn = run(&[
        "ask",
        "withdraw",
        "a-1",
        "No longer needed.",
        "--project",
        "demo",
    ]);
    assert!(
        withdrawn.status.success(),
        "{}",
        String::from_utf8_lossy(&withdrawn.stderr)
    );
    assert!(root.join("demo/asks/a-1/r1.toml").is_file());
    let record = std::fs::read_to_string(root.join("demo/asks/a-1/r1.withdrawn.toml")).unwrap();
    assert!(record.contains("by = \"rolf\""));
    assert!(run(&ask_args).status.success());
    assert!(
        run(&[
            "ask",
            "answer",
            "a-2",
            "--revision",
            "1",
            "1",
            "--project",
            "demo"
        ])
        .status
        .success()
    );
    let refused = run(&[
        "ask",
        "withdraw",
        "a-2",
        "No longer needed.",
        "--project",
        "demo",
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("answered ask cannot be withdrawn"));
}

#[test]
fn context_prints_a_usable_prefix_in_a_scrubbed_environment() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("my root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "Demo"])
            .status
            .success()
    );

    let out = hp(
        home.path(),
        &["--root", root_arg, "context", "demo", "--peek"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let prefix = text
        .lines()
        .next()
        .unwrap()
        .strip_prefix("Commands: ")
        .unwrap();
    // Fixed shape `<binary> --root <root>`, with the spaced root shell-quoted.
    assert_eq!(prefix, format!("{BIN} --root '{root_arg}'"));

    // The printed prefix works as typed, from a bare shell.
    let listed = Command::new("/bin/sh")
        .env_clear()
        .env("HOME", home.path())
        .args(["-c", &format!("{prefix} list")])
        .output()
        .unwrap();
    assert!(listed.status.success());
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        "demo\tactive\tno threads\n"
    );
}

#[test]
fn peek_records_nothing_and_context_records_seen_items() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );
    let item = "+++\nid = \"20260917T000000Z-routine-r-1\"\nkind = \"routine\"\nsubject = \"r\"\ncreated = \"x\"\nsummary = \"s\"\n+++\n";
    std::fs::write(
        root.join("demo/inbox/20260917T000000Z-routine-r-1.md"),
        item,
    )
    .unwrap();
    let seen = root.join("demo/.state/inbox-seen.json");

    assert!(
        hp(
            home.path(),
            &["--root", root_arg, "context", "demo", "--peek"]
        )
        .status
        .success()
    );
    assert!(!seen.exists());
    assert!(
        hp(home.path(), &["--root", root_arg, "context", "demo"])
            .status
            .success()
    );
    assert!(
        std::fs::read_to_string(&seen)
            .unwrap()
            .contains("routine-r-1")
    );
}

#[test]
fn path_like_names_and_slugs_are_refused() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        !hp(home.path(), &["--root", root_arg, "new", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(home.path(), &["--root", root_arg, "open", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(home.path(), &["--root", root_arg, "context", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(home.path(), &["--root", root_arg, "thread", "list", "../x"])
            .status
            .success()
    );
    assert!(
        !hp(
            home.path(),
            &["--root", root_arg, "delete", "../x", "--force"]
        )
        .status
        .success()
    );
    assert!(!root.exists());
    assert!(!home.path().join("x").exists());
}

#[test]
fn ticker_start_without_projects_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    assert!(hp(home.path(), &["ticker", "start"]).status.success());
    assert!(!home.path().join(".herdr-ade").exists());
    assert!(!home.path().join(".config").exists());
}
