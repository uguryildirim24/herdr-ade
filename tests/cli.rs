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
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["outcome"], "listed");
    let entries = result["data"]["result"].as_array().unwrap();
    assert_eq!(entries[0]["count"], 2);
    assert_eq!(entries[0]["kind"], "command-failed");
    let id = entries[0]["id"].as_str().unwrap();
    let shown = ledger(&["show", id, "--json"]);
    assert!(shown.status.success());
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["outcome"], "shown");
    assert_eq!(shown["data"]["record"]["id"], id);
    assert_eq!(shown["data"]["record"]["count"], 2);

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
    let closed = ledger(&["done", id, "--json"]);
    assert!(closed.status.success());
    let closed: serde_json::Value = serde_json::from_slice(&closed.stdout).unwrap();
    assert_eq!(closed["outcome"], "closed");
    assert_eq!(closed["data"]["record"]["id"], id);
    assert_eq!(closed["data"]["record"]["closed"], true);
    assert_eq!(closed["data"]["changed"], true);
    assert_eq!(closed["message"], format!("{id} closed\n"));

    let shown: serde_json::Value = serde_json::from_slice(&ledger(&["show", id]).stdout).unwrap();
    assert_eq!(shown["closed"], true);
    let listed: serde_json::Value =
        serde_json::from_slice(&ledger(&["list", "--json"]).stdout).unwrap();
    let open = listed["data"]["result"].as_array().unwrap();
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
fn every_named_verb_returns_a_structured_refusal() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("missing-root");
    let root_arg = root.to_str().unwrap();
    let cases: &[&[&str]] = &[
        &["round", "show", "missing", "r1"],
        &["thread", "show", "missing", "t-1"],
        &["ask"],
        &["decide"],
        &["plan", "show", "--project", "missing"],
        &["say", "--what", "This was checked.", "--project", "missing"],
        &["done", "--report", "report.md", "--sha", "deadbeef"],
        &["thread", "resolve", "missing", "t-1"],
        &["context", "missing"],
        &["doctor", "unexpected"],
        &["harness", "unexpected"],
        &["inbox", "done", "missing", "--all"],
        &["open", "missing"],
    ];
    for args in cases {
        let mut full = vec!["--root", root_arg, "--json"];
        full.extend_from_slice(args);
        let output = hp(home.path(), &full);
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {:?}", output.stderr);
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["outcome"], "refused", "{args:?}: {result}");
        assert_eq!(
            result["command"]
                .as_str()
                .and_then(|command| command.split_whitespace().next()),
            args.first().copied(),
            "{args:?}: {result}"
        );
        assert!(
            result["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "{args:?}: {result}"
        );
    }
}

#[test]
fn successful_commands_keep_human_text_and_return_one_machine_record() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    let ordinary = hp(home.path(), &["--root", root_arg, "new", "demo"]);
    assert!(ordinary.status.success());
    assert!(String::from_utf8_lossy(&ordinary.stdout).starts_with("created `demo`"));

    let structured = hp(
        home.path(),
        &["--root", root_arg, "context", "demo", "--peek", "--json"],
    );
    assert!(structured.status.success());
    let result: serde_json::Value = serde_json::from_slice(&structured.stdout).unwrap();
    assert_eq!(result["outcome"], "shown");
    assert_eq!(result["command"], "context");
    assert_eq!(result["data"]["slug"], "demo");
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .starts_with("Commands: ")
    );
}

#[test]
fn ticker_start_without_projects_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    assert!(hp(home.path(), &["ticker", "start"]).status.success());
    assert!(!home.path().join(".herdr-ade").exists());
    assert!(!home.path().join(".config").exists());
}
