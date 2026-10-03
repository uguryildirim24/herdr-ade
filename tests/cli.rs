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
fn notes_and_tasks_accept_a_request_from_another_project() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    let run = |args: &[&str]| {
        let output = hp(home.path(), &[&["--root", root_arg], args].concat());
        assert!(
            output.status.success(),
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    run(&["new", "demo"]);
    run(&["new", "source"]);
    let talk = root.join("source/.state/talk");
    std::fs::create_dir_all(&talk).unwrap();
    std::fs::write(
        talk.join("journal.jsonl"),
        concat!(
            "{\"seq\":1,\"at\":\"2026-09-23T00:00:00Z\",\"rolf\":{",
            "\"request\":\"q-cross\",",
            "\"text\":\"Keep the overnight direction in the project record.\"}}\n"
        ),
    )
    .unwrap();

    run(&[
        "note",
        "add",
        "demo",
        "Keep the overnight direction in the project record.",
        "--kind",
        "instruction",
        "--request",
        "source/q-cross",
    ]);
    run(&[
        "task",
        "add",
        "demo",
        "--title",
        "Record the overnight direction",
        "--request",
        "source/q-cross",
        "--acceptance",
        "The direction remains visible in context.",
    ]);

    let notes = std::fs::read_to_string(root.join("demo/.state/notes/n-0001.json")).unwrap();
    let task = std::fs::read_to_string(root.join("demo/.state/tasks/job-0001.toml")).unwrap();
    assert!(notes.contains("\"request\":\"source/q-cross\""), "{notes}");
    assert!(task.contains("request:source/q-cross"), "{task}");

    let missing = hp(
        home.path(),
        &[
            "--root",
            root_arg,
            "note",
            "add",
            "demo",
            "This request is missing.",
            "--kind",
            "memory",
            "--request",
            "missing/q-lost",
        ],
    );
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("request_authority:"));
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
    let item = "+++\nid = \"20260917T000000Z-note-r-1\"\nkind = \"note\"\nsubject = \"r\"\ncreated = \"x\"\nsummary = \"s\"\n+++\n";
    std::fs::create_dir(root.join("demo/.state/inbox")).unwrap();
    std::fs::write(
        root.join("demo/.state/inbox/20260917T000000Z-note-r-1.md"),
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
    assert!(std::fs::read_to_string(&seen).unwrap().contains("note-r-1"));
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
        !hp(home.path(), &["--root", root_arg, "delete", "../x"])
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
        &["review", "missing"],
        &["thread", "show", "missing", "t-1"],
        &["plan", "show", "missing"],
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
            .is_some_and(|message| !message.is_empty())
    );
}

#[test]
fn ticker_start_without_projects_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    assert!(hp(home.path(), &["ticker", "start"]).status.success());
    let install = Command::new(BIN)
        .env_clear()
        .env("HOME", home.path())
        .env("HERDR_ADE_INSTALL_TICKER", "1")
        .args(["ticker", "start"])
        .output()
        .unwrap();
    assert!(install.status.success());
    assert_eq!(install.stdout, b"HERDR_ADE_TICKER_NO_PROJECTS=1\n");
    assert!(!home.path().join(".herdr-ade").exists());
    assert!(!home.path().join(".config").exists());
}
