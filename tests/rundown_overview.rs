//! D36: render the real producer's output, not a hand-written overview fixture.
#[path = "../src/rundown/view.rs"]
mod view;

use sha2::{Digest, Sha256};
use std::process::Command;

fn screen(reply: &serde_json::Value) -> (view::Card, String) {
    let card = view::Card::from_view("Demo", reply).unwrap();
    let text = std::iter::once(view::harness_line(
        &card.harness,
        160,
        jiff::Timestamp::now(),
    ))
    .chain(view::render(&card, 160, 0, ""))
    .collect::<Vec<_>>()
    .iter()
    .map(|line| view::visible(line))
    .collect::<Vec<_>>()
    .join("\n");
    (card, text)
}

#[test]
fn real_overview_renders_standalone_nested_held_and_unreadable_work() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_herdr-ade"))
            .env_clear()
            .env("HOME", home.path())
            .args(["--root", root.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    run(&["new", "demo"]);
    let state = root.join("demo/.state");
    for dir in ["threads", "tasks", "events", "artifacts"] {
        std::fs::create_dir_all(state.join(dir)).unwrap();
    }
    let report = "+++\nverdict = 'FAIL'\n+++\nNeeds a fix\n";
    let hash = format!("{:x}", Sha256::digest(report));
    std::fs::write(state.join("artifacts").join(&hash), report).unwrap();
    std::fs::write(state.join("threads/t-0001.toml"), "id = 't-0001'\nrole = 'critic'\nstatus = 'resolved'\nattempt = 1\nmerged_sha = 'checked-sha'\n").unwrap();
    std::fs::write(state.join("events/t-0001-1-1.toml"), format!("id = 't-0001-1-1'\nop = 't-0001-1-1'\nthread = 't-0001'\nattempt = 1\ncreated = '2026-09-20T00:00:00Z'\n[recipient]\npane = 'w1:p1'\ncoordinator_attempt = 1\n[payload.done]\nsha = 'checked-sha'\nreport_path = 'historical/report.md'\nartifact = '{hash}'\n")).unwrap();
    std::fs::write(state.join("threads/t-0002.toml"), "broken TOML!").unwrap();
    std::fs::write(
        state.join("tasks/job-0001.toml"),
        "id = 'job-0001'\ntitle = 'Historical finished work'\nauthority = ['request:historical']\nacceptance = ['done']\n[[installed]]\nat = 'then'\ncommand = 'historical install'\n",
    )
    .unwrap();
    let plan = "schema = 1\ngoal = 'Historical goal'\nrevision = 1\n[[steps]]\nid = 's-1'\ntext = 'Finished standalone'\ntasks = ['job-0001']\n[[steps]]\nid = 's-2'\ntext = 'Parent work'\n[[steps.subtasks]]\nid = 's-3'\ntext = 'Nested work'\n[[steps]]\nid = 's-4'\ntext = 'Held check'\nthreads = ['t-0001']\n";
    std::fs::write(state.join("plan.toml"), plan).unwrap();
    let before: serde_json::Value = serde_json::from_str(&run(&["install-check"])).unwrap();
    // These exact bytes cross the producer/consumer boundary.
    let output = run(&["--json", "overview", "demo"]);
    let reply: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert!(!output.contains("\"subtasks\":null"));
    assert!(
        reply["data"]["result"]["plan"]["steps"][2]["failed_check_hold"].is_object(),
        "{reply:#}"
    );
    let (card, text) = screen(&reply);
    assert_eq!(card.steps.len(), 3);
    assert_eq!(card.steps[1].subtasks.len(), 1);
    for expected in [
        "Demo",
        "1 of 3",
        "Finished standalone",
        "Nested work",
        "Held check",
        "Some work records could not be read",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    for error in [
        "Rundown read failed",
        "invalid type:",
        "missing field",
        "0 of 0",
    ] {
        assert!(!text.contains(error), "{text}");
    }
    let after: serde_json::Value = serde_json::from_str(&run(&["install-check"])).unwrap();
    assert_eq!(before, after);
    assert_eq!(before[0]["done"], 1);
    assert_eq!(before[0]["total"], 4);
    assert_eq!(
        std::fs::read_to_string(state.join("plan.toml")).unwrap(),
        plan
    );
}

#[test]
fn null_subtasks_load_and_one_bad_row_does_not_blank_readable_steps() {
    let mut reply = serde_json::json!({"plan": {"schema":1,"revision":1,"steps":[
        {"state":"done","text":"Readable standalone","subtasks":null},
        {"state":"left","text":"Readable parent","subtasks":[
            {"state":"left","text":"Readable child","subtasks":null},
            {"state":42,"text":"Damaged child"}
        ]},
        {"state":"running","text":"Readable held","failed_check_hold":{"message":"held by failed check"}},
        {"state":42,"text":"Damaged row"}
    ]},"work":"No active work","needs_you":"","actions":[]});
    let (card, text) = screen(&reply);
    assert_eq!(card.steps.len(), 3);
    assert_eq!(card.steps[1].subtasks.len(), 1);
    assert!(
        text.contains("Demo") && text.contains("Readable child") && text.contains("Readable held"),
        "{text}"
    );
    assert_eq!(text.matches("Rundown read failed:").count(), 1, "{text}");
    assert!(!text.contains("0 of 0"), "{text}");
    reply["plan"]["steps"].as_array_mut().unwrap().pop();
    reply["plan"]["steps"][1]["subtasks"]
        .as_array_mut()
        .unwrap()
        .pop();
    let (card, text) = screen(&reply);
    assert!(card.steps[0].subtasks.is_empty());
    assert!(card.steps[1].subtasks[0].subtasks.is_empty());
    assert!(!text.contains("Rundown read failed"), "{text}");
}
