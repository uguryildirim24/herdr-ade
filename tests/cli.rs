//! End-to-end checks of the built binary with a scrubbed environment.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

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
fn internal_install_check_reports_only_counts_and_readability_without_writes() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let project = root.join("demo");
    let state = project.join(".state");
    std::fs::create_dir_all(state.join("tasks")).unwrap();
    // The check must not parse or emit PROJECT.md's content.
    std::fs::write(
        project.join("PROJECT.md"),
        "private project content, not front matter",
    )
    .unwrap();
    std::fs::write(state.join("project.json"), r#"{"status":"archived"}"#).unwrap();
    std::fs::write(state.join("tasks/job-0001.toml"), "id = 'job-0001'\ntitle = 'private task title'\nauthority = ['request:historical']\nacceptance = ['done']\n[[installed]]\nat = 'then'\ncommand = 'historical install'\n").unwrap();
    let plan = "schema = 1\n[[steps]]\nid = 's-1'\ntext = 'private step text'\ntasks = ['job-0001']\n[[steps.subtasks]]\nid = 's-2'\ntasks = ['job-0001']\n[[steps]]\nid = 's-3'\n";
    std::fs::write(state.join("plan.toml"), plan).unwrap();
    let check = || {
        let output = hp(
            home.path(),
            &["--root", root.to_str().unwrap(), "install-check"],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    assert_eq!(
        check(),
        serde_json::json!([{"project":"demo","done":2,"total":3,"records_load":true}])
    );
    assert_eq!(
        std::fs::read_to_string(state.join("plan.toml")).unwrap(),
        plan
    );
    assert!(!root.join(".ticker.lock").exists());
    assert!(!state.join("context-cursor.json").exists());
    std::fs::write(state.join("coordinator.json"), r#"{"launch":123}"#).unwrap();
    assert_eq!(check()[0]["records_load"], false);
    std::fs::remove_file(state.join("coordinator.json")).unwrap();
    std::fs::create_dir(state.join("reviews")).unwrap();
    std::fs::write(state.join("reviews/review-1.toml"), "not valid TOML").unwrap();
    assert_eq!(check()[0]["records_load"], false);
    let help = hp(home.path(), &["--help"]);
    assert!(!String::from_utf8_lossy(&help.stdout).contains("install-check"));
}

#[test]
fn new_project_popup_lists_the_repository_like_the_cli_and_leaves_the_goal_for_chat() {
    for repository in [".", "/srv/app@box", ""] {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let mut popup = Command::new(BIN)
            .env_clear()
            .env("HOME", home.path())
            .current_dir(home.path())
            .args(["--root", root.to_str().unwrap(), "pane", "new"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(popup.stdin.take().unwrap(), "Menu\n{repository}\n").unwrap();
        let output = popup.wait_with_output().unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("Name: Repository:"), "{text}");
        assert!(!text.contains("Goal"), "{text}");
        if repository.is_empty() {
            assert!(text.contains("no repository given"), "{text}");
            assert!(!root.join("menu").exists());
            continue;
        }
        // The popup cannot open a coordinator in this scrubbed environment,
        // but its project must already have the same repository as `new --repo`.
        let cli = Command::new(BIN)
            .env_clear()
            .env("HOME", home.path())
            .current_dir(home.path())
            .args([
                "--root",
                root.to_str().unwrap(),
                "new",
                "Cli",
                "--repo",
                repository,
            ])
            .output()
            .unwrap();
        assert!(cli.status.success(), "{:?}", cli.stderr);
        let settings = |slug| {
            let page = std::fs::read_to_string(root.join(slug).join("PROJECT.md")).unwrap();
            toml::from_str::<toml::Value>(page.split("+++\n").nth(1).unwrap()).unwrap()
        };
        let menu = settings("menu");
        assert_eq!(menu["repos"], settings("cli")["repos"]);
        assert_eq!(menu["goal"].as_str(), Some(""));
    }
}

#[test]
fn thread_show_puts_the_durable_report_and_usage_before_the_record() {
    use sha2::Digest as _;
    let home = tempfile::tempdir().unwrap();
    assert!(hp(home.path(), &["new", "demo"]).status.success());
    let project = home.path().join(".herdr-ade/demo");
    let state = project.join(".state");
    std::fs::create_dir_all(state.join("threads")).unwrap();
    std::fs::write(
        state.join("threads/t-0001.toml"),
        "id = \"t-0001\"\nstatus = \"resolved\"\nattempt = 1\n",
    )
    .unwrap();
    let show = || {
        let output = hp(home.path(), &["thread", "show", "demo", "t-0001", "--json"]);
        assert!(output.status.success(), "{output:?}");
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        result["message"].as_str().unwrap().to_string()
    };
    assert!(show().starts_with("report: no report yet · usage unknown\ngroup = "));
    std::fs::create_dir_all(state.join("events")).unwrap();
    std::fs::create_dir_all(state.join("artifacts")).unwrap();
    let report = b"Finished.\n";
    let hash = format!("{:x}", sha2::Sha256::digest(report));
    let path = state.join("artifacts").join(&hash);
    std::fs::write(&path, report).unwrap();
    let event = format!(
        "id = \"t-0001-1-1\"\nop = \"t-0001-1-1\"\nthread = \"t-0001\"\nattempt = 1\ncreated = \"2026-10-03T00:00:00Z\"\n[recipient]\npane = \"w1:p1\"\ncoordinator_attempt = 1\n[payload.done]\nsha = \"abc\"\nreport_path = \"removed/report.md\"\nartifact = \"{hash}\"\n"
    );
    for (usage, summary) in [
        ("", "usage unknown"),
        (
            "[usage]\ninput = 200000\noutput = 0\ncache_read = 4900000\ncache_write = 0\nreasoning = 0\ntotal = 5100000\n",
            "5.1M tokens (4.9M cached)",
        ),
    ] {
        std::fs::write(
            state.join("events/t-0001-1-1.toml"),
            format!("{event}{usage}"),
        )
        .unwrap();
        assert!(show().starts_with(&format!("report: {} · {summary}\ngroup = ", path.display())));
    }
}

#[test]
fn help_and_version_are_successful_displays_even_with_json() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        vec!["review", "--help", "--json"],
        vec!["--version", "--json"],
    ] {
        let output = hp(home.path(), &args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
        let display = String::from_utf8(output.stdout).unwrap();
        assert!(!display.contains("\"outcome\":\"refused\""), "{display}");
        if args[0] == "review" {
            assert!(
                display.contains("enable automatic reviews for this project"),
                "{display}"
            );
        }
    }
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
fn ticker_start_before_the_first_project_confirms_a_running_loop() {
    let home = tempfile::tempdir().unwrap();
    let start = hp(home.path(), &["ticker", "start"]);
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    let install = Command::new(BIN)
        .env_clear()
        .env("HOME", home.path())
        .env("HERDR_ADE_INSTALL_TICKER", "1")
        .args(["ticker", "start"])
        .output()
        .unwrap();
    let before = hp(home.path(), &["ticker", "status"]);
    let created = hp(home.path(), &["new", "demo"]);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let after = hp(home.path(), &["ticker", "status"]);
    let stopped = hp(home.path(), &["ticker", "stop"]);
    assert!(stopped.status.success());
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );
    assert!(created.status.success());
    for status in [before, after] {
        let text = String::from_utf8_lossy(&status.stdout);
        assert!(text.contains("pid:"), "{text}");
        assert!(
            text.contains("recent progress") || text.contains("responsiveness unknown"),
            "{text}"
        );
        assert!(!text.contains("not running (lock released)"), "{text}");
    }
    assert!(!home.path().join(".config").exists());
}

#[test]
fn default_root_next_commands_need_no_ha_alias() {
    let home = tempfile::tempdir().unwrap();
    let created = hp(home.path(), &["new", "demo"]);
    let text = String::from_utf8_lossy(&created.stdout);
    assert!(
        text.contains(&format!(
            "next: {BIN} --root {} open demo",
            home.path().join(".herdr-ade").display()
        )),
        "{text}"
    );
    let doctor = hp(home.path(), &["--json", "doctor"]);
    assert!(!doctor.status.success());
    let result: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(result["data"]["healthy"], false);
    assert!(result["next"].as_str().unwrap().starts_with(BIN));
    assert!(!result["next"].as_str().unwrap().contains("ha doctor"));
}
