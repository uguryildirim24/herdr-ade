//! D30's public read paths must keep missing evidence visible, not omit work.
use std::process::Command;

#[test]
fn corrupt_truncated_and_empty_lanes_remain_visible_in_overview_handoff_and_rundown() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_herdr-ade"))
            .env_clear()
            .env("HOME", home.path())
            .args(["--root", root.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(&["new", "demo"]);
    let state = root.join("demo/.state");
    std::fs::create_dir_all(state.join("threads")).unwrap();
    std::fs::create_dir_all(state.join("tasks")).unwrap();
    let damaged = state.join("threads/t-0001.toml");
    std::fs::write(
        &damaged,
        "id = 't-0001'\ntitle = 'Before damage'\nstatus = 'open'\n",
    )
    .unwrap();
    // A sparse historical lane still loads with the defaulted additions.
    std::fs::write(
        state.join("threads/t-0002.toml"),
        "id = 't-0002'\ntitle = 'Readable historical lane'\nstatus = 'open'\n",
    )
    .unwrap();
    std::fs::write(state.join("tasks/job-0001.toml"), "id = 'job-0001'\ntitle = 'Historical finished task'\nauthority = ['request:historical']\nacceptance = ['done']\n[[installed]]\nat = 'then'\ncommand = 'historical install'\n").unwrap();
    let plan = "schema = 1\ngoal = 'Historical goal'\nrevision = 1\n[[steps]]\nid = 's-1'\ntasks = ['job-0001']\n[[steps.subtasks]]\nid = 's-2'\ntasks = ['job-0001']\n[[steps]]\nid = 's-3'\n";
    std::fs::write(state.join("plan.toml"), plan).unwrap();
    let before: serde_json::Value = serde_json::from_str(&run(&["install-check"])).unwrap();
    assert_eq!(before[0]["done"], 2);
    assert_eq!(before[0]["total"], 3);
    for invalid in ["not valid TOML at all!", "id = 't-0001'\ntitle = 'cut", ""] {
        std::fs::write(&damaged, invalid).unwrap();
        for args in [
            vec!["overview", "demo"],
            vec!["handoff", "demo"],
            vec!["context", "demo", "--peek"],
        ] {
            let text = run(&args);
            assert!(text.contains("Unreadable lane"), "{text}");
            assert!(text.contains(damaged.to_str().unwrap()), "{text}");
            assert!(text.contains("Readable historical lane"), "{text}");
            if invalid.is_empty() {
                assert!(text.contains("incomplete"), "{text}");
            } else {
                assert!(text.contains("TOML parse error"), "{text}");
            }
            assert!(!text.contains("## Current work\n\nNone."), "{text}");
        }
        // Rundown consumes exactly this machine-readable overview result.
        let json: serde_json::Value =
            serde_json::from_str(&run(&["--json", "overview", "demo"])).unwrap();
        let card = &json["data"]["result"];
        assert!(card["actions"].to_string().contains("Unreadable lane"));
        assert!(
            card["actions"]
                .to_string()
                .contains("Readable historical lane")
        );
        assert!(card["work"].as_str().unwrap().contains("1 unreadable"));
        let after: serde_json::Value = serde_json::from_str(&run(&["install-check"])).unwrap();
        assert_eq!(after[0]["done"], before[0]["done"]);
        assert_eq!(after[0]["total"], before[0]["total"]);
        assert_eq!(
            std::fs::read_to_string(state.join("plan.toml")).unwrap(),
            plan
        );
        assert_eq!(std::fs::read_to_string(&damaged).unwrap(), invalid);
    }
}
