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
fn withdraw_commands_keep_history_and_ask_output_starts_with_id() {
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
    let withdrawn = run(&[
        "ask",
        "close",
        "demo",
        "a-1",
        "--withdraw",
        "No longer needed.",
    ]);
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
        run(&["ask", "close", "demo", "a-2", "--choice", "1",])
            .status
            .success()
    );
    let refused = run(&[
        "ask",
        "close",
        "demo",
        "a-2",
        "--withdraw",
        "No longer needed.",
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("answered ask cannot be withdrawn"));
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

    let notes = std::fs::read_to_string(root.join("demo/.state/notes.jsonl")).unwrap();
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
    assert_eq!(
        String::from_utf8_lossy(&missing.stderr).trim(),
        "herdr-ade: request_authority: no request `q-lost` in project `missing`"
    );
}

#[test]
fn note_add_reports_which_briefs_receive_it() {
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
        String::from_utf8(output.stdout).unwrap()
    };
    run(&["new", "demo"]);
    let talk = root.join("demo/.state/talk");
    std::fs::create_dir_all(&talk).unwrap();
    std::fs::write(
        talk.join("journal.jsonl"),
        "{\"seq\":1,\"at\":\"2026-09-23T00:00:00Z\",\"rolf\":{\"request\":\"q-note\",\"text\":\"Record this decision.\"}}\n",
    )
    .unwrap();
    run(&[
        "task",
        "add",
        "demo",
        "--title",
        "Record decision",
        "--request",
        "q-note",
        "--acceptance",
        "Decision recorded.",
    ]);
    run(&[
        "task",
        "add",
        "demo",
        "--title",
        "Share decision",
        "--request",
        "q-note",
        "--acceptance",
        "Decision shared.",
    ]);
    let add = [
        "note",
        "add",
        "demo",
        "Decision recorded.",
        "--kind",
        "memory",
        "--request",
        "q-note",
    ];
    assert_eq!(run(&add), "noted n-0001, in every lane brief\n");
    assert_eq!(
        run(&[&add[..], &["--task", "job-0001", "--task", "job-0002"]].concat()),
        "noted n-0002, in briefs for job-0001, job-0002\n"
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
    assert!(
        recipes.contains("coordinator's one-off lane choice"),
        "{recipes}"
    );
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
    assert!(!repeat.contains("# Project"), "{repeat}");
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
        &["ask"],
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
