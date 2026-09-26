use std::process::Command;

#[test]
fn common_refusals_show_next_in_text_and_json() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let run = |json: bool, args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_herdr-ade"));
        cmd.env_clear()
            .env("HOME", home.path())
            .args(["--root", root.to_str().unwrap()]);
        if json {
            cmd.arg("--json");
        }
        cmd.args(args).output().unwrap()
    };
    assert!(run(false, &["new", "demo"]).status.success());
    let cases: &[(&[&str], &str, &str)] = &[
        (
            &[
                "task",
                "add",
                "demo",
                "--title",
                "",
                "--request",
                "request:q-1",
                "--acceptance",
                "done",
            ],
            "task_title:",
            "ha task add",
        ),
        (
            &[
                "task",
                "add",
                "demo",
                "--title",
                "work",
                "--request",
                "request:q-1",
                "--acceptance",
                "",
            ],
            "task_acceptance:",
            "ha task add",
        ),
        (
            &["task", "show", "demo", "job-9999"],
            "task_unknown:",
            "ha task list",
        ),
        (
            &[
                "note",
                "add",
                "demo",
                "",
                "--kind",
                "memory",
                "--request",
                "request:q-1",
            ],
            "note_text:",
            "ha note add",
        ),
        (
            &["task", "drop", "demo", "job-9999", "--reason", ""],
            "task_drop:",
            "ha task drop",
        ),
    ];
    for (args, reason, next) in cases {
        let text = run(false, args);
        assert!(!text.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&text.stderr);
        assert!(
            stderr.contains(reason) && stderr.contains(&format!("next: {next}")),
            "{args:?}: {stderr}"
        );
        let json = run(true, args);
        let record: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
        assert_eq!(record["outcome"], "refused");
        assert!(record["next"].as_str().unwrap().starts_with(next));
    }
}
