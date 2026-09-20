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
fn ledger_cli_records_folds_prints_a_task_and_closes() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );
    for _ in 0..2 {
        assert!(
            !hp(
                home.path(),
                &["--root", root_arg, "thread", "show", "demo", "t-0001"]
            )
            .status
            .success()
        );
    }
    let ledger = |args: &[&str]| {
        Command::new(BIN)
            .env_clear()
            .env("HOME", home.path())
            .env("HERDR_ADE_LAUNCH", "demo/t-0001/1/brief")
            .args(["--root", root_arg, "ledger"])
            .args(args)
            .output()
            .unwrap()
    };
    let out = ledger(&["list", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(entries[0]["count"], 2);
    assert_eq!(entries[0]["kind"], "command-failed");
    let id = entries[0]["id"].as_str().unwrap();
    let out = ledger(&["task", id]);
    assert!(out.status.success());
    let task = String::from_utf8(out.stdout).unwrap();
    assert!(task.starts_with("# Fix an observed harness failure:"));
    for text in [
        "Count: 2",
        "First seen:",
        "Last seen:",
        "no thread",
        "regression test",
    ] {
        assert!(task.contains(text), "{task}");
    }
    assert!(ledger(&["done", id]).status.success());
    let shown: serde_json::Value = serde_json::from_slice(&ledger(&["show", id]).stdout).unwrap();
    assert_eq!(shown["closed"], true);
    let open: Vec<serde_json::Value> =
        serde_json::from_slice(&ledger(&["list", "--json"]).stdout).unwrap();
    assert!(open.iter().all(|entry| entry["id"] != id));
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
