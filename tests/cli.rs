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
fn decide_reports_its_classes_and_all_missing_class_requirements() {
    let run = |args: &[&str]| Command::new(BIN).args(args).output().unwrap();

    let help = run(&["decide", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for (class, meaning) in [
        ("what-you-get", "taste, direction, or the result"),
        ("money", "spend money"),
        ("undo", "cannot be undone"),
        ("routine", "ordinary choice"),
    ] {
        let line = help.lines().find(|line| line.contains(class)).unwrap();
        assert!(line.contains(meaning), "{line}");
    }
    assert!(
        help.contains("what-you-get, money, undo, routine"),
        "{help}"
    );

    let wrong = run(&["decide", "--class", "unknown"]);
    assert!(!wrong.status.success());
    let wrong = String::from_utf8(wrong.stderr).unwrap();
    assert!(
        wrong.contains("possible values: what-you-get, money, undo, routine"),
        "{wrong}"
    );

    let no_class = run(&["decide"]);
    assert!(!no_class.status.success());
    let no_class = String::from_utf8(no_class.stderr).unwrap();
    assert!(no_class.contains("a decision line"), "{no_class}");
    assert!(
        no_class.contains("what-you-get, money, undo, or routine"),
        "{no_class}"
    );

    let no_basis = run(&["decide", "--class", "what-you-get"]);
    assert!(!no_basis.status.success());
    let no_basis = String::from_utf8(no_basis.stderr).unwrap();
    assert!(no_basis.contains("a decision line"), "{no_basis}");
    assert!(no_basis.contains("; --basis"), "{no_basis}");
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
            "demo",
            "I kept the words short.",
            "--class",
            "routine",
        ])
        .status
        .success()
    );
    let overturned = run(&[
        "decide",
        "overturn",
        "demo",
        "d-0001",
        "I want more detail.",
    ]);
    assert!(
        overturned.status.success(),
        "{}",
        String::from_utf8_lossy(&overturned.stderr)
    );
    assert!(String::from_utf8_lossy(&overturned.stdout).contains("overturned by rolf"));
    assert!(
        !run(&["decide", "overturn", "demo", "d-9999", "No thanks.",])
            .status
            .success()
    );
    let ask_args = [
        "ask",
        "demo",
        "May I spend\n five dollars on this check?",
        "--choice",
        "Keep it running.",
        "--choice",
        "Stop it now.",
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
    let withdrawn = run(&["ask", "withdraw", "demo", "a-1", "No longer needed."]);
    assert!(
        withdrawn.status.success(),
        "{}",
        String::from_utf8_lossy(&withdrawn.stderr)
    );
    assert!(root.join("demo/.state/asks/a-1/r1.toml").is_file());
    let record =
        std::fs::read_to_string(root.join("demo/.state/asks/a-1/r1.withdrawn.toml")).unwrap();
    assert!(record.contains("by = \"rolf\""));
    assert!(run(&ask_args).status.success());
    assert!(
        run(&["ask", "answer", "demo", "a-2", "--revision", "1", "1",])
            .status
            .success()
    );
    let refused = run(&["ask", "withdraw", "demo", "a-2", "No longer needed."]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("answered ask cannot be withdrawn"));
}

#[test]
fn notes_decisions_and_tasks_accept_a_request_from_another_project() {
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
        "decide",
        "demo",
        "I will keep the overnight direction in the project record.",
        "--class",
        "what-you-get",
        "--basis",
        "request:source/q-cross",
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

    let notes = std::fs::read_to_string(root.join("demo/.state/notes.jsonl")).unwrap();
    let decisions = std::fs::read_to_string(root.join("demo/.state/decisions.jsonl")).unwrap();
    let task = std::fs::read_to_string(root.join("demo/.state/tasks/job-0001.toml")).unwrap();
    assert!(notes.contains("\"request\":\"source/q-cross\""), "{notes}");
    assert!(
        decisions.contains("\"basis\":\"request:source/q-cross\""),
        "{decisions}"
    );
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
    assert_eq!(
        String::from_utf8_lossy(&missing.stderr).trim(),
        "herdr-ade: request_authority: no request `q-lost` in project `missing`"
    );
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
    assert_eq!(text.lines().next(), Some("## Since your last context"));
    assert!(text.contains("# Project"));
    let prefix = text
        .lines()
        .find_map(|line| line.strip_prefix("Commands: "))
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
fn context_uses_ha_for_the_default_root_and_keeps_recipe_commands_in_the_skill() {
    let home = tempfile::tempdir().unwrap();
    let config_dir = home.path().join(".config/herdr-ade");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "[routing]\ndefault = \"pi_codex_sol_high\"\n",
    )
    .unwrap();
    assert!(hp(home.path(), &["new", "demo"]).status.success());

    let out = hp(home.path(), &["context", "demo", "--peek"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().next(), Some("## Since your last context"));
    assert!(text.contains("# Project"));
    assert!(text.contains("\nCommands: ha\n"), "{text}");
    let recipes = text
        .split("## Recipes\n")
        .nth(1)
        .unwrap()
        .split("\n## ")
        .next()
        .unwrap();
    assert!(!recipes.contains("--root"), "{recipes}");
    assert!(!recipes.contains("thread start"), "{recipes}");
    assert!(!recipes.contains("herdr-pro start"), "{recipes}");
    assert!(recipes.contains("Rolf's one-off choice"), "{recipes}");
}

#[test]
fn context_repeats_compactly_and_full_restores_standing_sections() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let args = ["--root", root.to_str().unwrap(), "context", "demo"];
    assert!(
        hp(
            home.path(),
            &["--root", root.to_str().unwrap(), "new", "demo"]
        )
        .status
        .success()
    );
    let first = String::from_utf8(hp(home.path(), &args).stdout).unwrap();
    assert!(first.contains("First read"));
    assert!(first.contains("## Standing instructions in force"));
    let repeat = String::from_utf8(hp(home.path(), &args).stdout).unwrap();
    assert!(
        repeat.starts_with("## Since your last context\n\nNothing new."),
        "{repeat}"
    );
    assert!(
        repeat.contains("Unchanged; run `ha context demo --full`"),
        "{repeat}"
    );
    assert!(!repeat.contains("## Standing instructions in force"));
    let full = String::from_utf8(
        hp(
            home.path(),
            &[
                "--root",
                root.to_str().unwrap(),
                "context",
                "demo",
                "--full",
            ],
        )
        .stdout,
    )
    .unwrap();
    assert!(full.contains("## Standing instructions in force"), "{full}");
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
    std::fs::create_dir(root.join("demo/.state/inbox")).unwrap();
    std::fs::write(
        root.join("demo/.state/inbox/20260917T000000Z-routine-r-1.md"),
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
    // Read-only failures are answers, not new ledger evidence.
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
    assert!(!root.join("demo/.state/ledger.jsonl").exists());
    let entry = |count| {
        format!(
            "{{\"record\":\"failure\",\"id\":\"f-0001\",\"at\":\"2026-09-22T00:00:00Z\",\"last_at\":\"2026-09-22T00:00:00Z\",\"kind\":\"command-failed\",\"subject\":\"ha thread start\",\"detail\":\"no thread\",\"count\":{count},\"closed\":false}}\n"
        )
    };
    std::fs::write(
        root.join("demo/.state/ledger.jsonl"),
        format!("{}{}", entry(1), entry(2)),
    )
    .unwrap();
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
    let out = ledger(&["list", "demo", "--json"]);
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
    let shown = ledger(&["show", "demo", id, "--json"]);
    assert!(shown.status.success());
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["outcome"], "shown");
    assert_eq!(shown["data"]["record"]["id"], id);
    assert_eq!(shown["data"]["record"]["count"], 2);

    let out = ledger(&["task", "demo", id]);
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
    let closed = ledger(&["done", "demo", id, "--json"]);
    assert!(closed.status.success());
    let closed: serde_json::Value = serde_json::from_slice(&closed.stdout).unwrap();
    assert_eq!(closed["outcome"], "closed");
    assert_eq!(closed["data"]["record"]["id"], id);
    assert_eq!(closed["data"]["record"]["closed"], true);
    assert_eq!(closed["data"]["changed"], true);
    assert_eq!(closed["message"], format!("{id} closed\n"));

    let shown: serde_json::Value =
        serde_json::from_slice(&ledger(&["show", "demo", id]).stdout).unwrap();
    assert_eq!(shown["closed"], true);
    let listed: serde_json::Value =
        serde_json::from_slice(&ledger(&["list", "demo", "--json"]).stdout).unwrap();
    let open = listed["data"]["result"].as_array().unwrap();
    assert!(open.iter().all(|entry| entry["id"] != id));
}

#[test]
fn round_show_without_a_round_lists_and_there_is_no_list_alias() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let root_arg = root.to_str().unwrap();
    assert!(
        hp(home.path(), &["--root", root_arg, "new", "demo"])
            .status
            .success()
    );

    let shown = hp(home.path(), &["--root", root_arg, "round", "show", "demo"]);
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let alias = hp(home.path(), &["--root", root_arg, "round", "list", "demo"]);
    assert!(!alias.status.success());
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
        &["round", "show", "missing", "r1"],
        &["thread", "show", "missing", "t-1"],
        &["ask"],
        &["decide"],
        &["plan", "show", "missing"],
        &["say", "missing", "--what", "This was checked."],
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
            .starts_with("## Since your last context")
    );
}

#[test]
fn ticker_start_without_projects_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    assert!(hp(home.path(), &["ticker", "start"]).status.success());
    assert!(!home.path().join(".herdr-ade").exists());
    assert!(!home.path().join(".config").exists());
}
