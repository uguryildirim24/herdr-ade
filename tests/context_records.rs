//! Context receipts must follow the exact evidence shown, without inbox copies.
use std::path::Path;
use std::process::Command;

use sha2::Digest as _;

const BIN: &str = env!("CARGO_BIN_EXE_herdr-ade");

fn context(home: &Path, pane: &str, peek: bool) -> String {
    let mut command = Command::new(BIN);
    command
        .env_clear()
        .env("HOME", home)
        .env("HERDR_PANE_ID", pane)
        .env("HERDR_ADE_LAUNCH", "demo/coordinator/1/fixture")
        .args([
            "--root",
            home.join("root").to_str().unwrap(),
            "context",
            "demo",
        ]);
    if peek {
        command.arg("--peek");
    }
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn context_acknowledges_only_shown_current_attempt_evidence_for_its_binding() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    assert!(
        Command::new(BIN)
            .env_clear()
            .env("HOME", home.path())
            .args(["--root", root.to_str().unwrap(), "new", "demo"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let project = root.join("demo");
    std::fs::create_dir(project.join(".state/threads")).unwrap();
    std::fs::write(
        project.join(".state/coordinator.json"),
        r#"{"pane_id":"w1:p1","generation":3,"bootstrap":"acknowledged","launch":{"attempt":1,"brief_hash":"fixture"}}"#,
    )
    .unwrap();
    for id in ["t-0001", "t-0002", "t-0003"] {
        std::fs::write(
            project.join(format!(".state/threads/{id}.toml")),
            format!("id = \"{id}\"\nstatus = \"open\"\nattempt = 2\nlast_group = \"idle\"\nreport_hash = \"report\"\n"),
        )
        .unwrap();
        std::fs::write(
            project.join(format!(".state/threads/{id}.md")),
            "historical report\n",
        )
        .unwrap();
    }
    std::fs::create_dir_all(project.join(".state/events")).unwrap();
    for (id, thread, attempt, generation, text) in [
        ("t-0001-1-1", "t-0001", 1, 3, "superseded attempt"),
        ("t-0001-2-1", "t-0001", 2, 3, "superseded message"),
        ("t-0001-2-2", "t-0001", 2, 3, "current wait"),
        ("t-0002-2-1", "t-0002", 2, 2, "earlier coordinator"),
    ] {
        std::fs::write(project.join(format!(".state/events/{id}.toml")), format!(
            "id = \"{id}\"\nop = \"{id}\"\nthread = \"{thread}\"\nattempt = {attempt}\ncreated = \"2026-09-20T00:00:00Z\"\n[recipient]\npane = \"w1:p1\"\ncoordinator_attempt = {generation}\n[payload.waiting]\ntext = \"{text}\"\n"
        )).unwrap();
    }
    std::fs::write(
        project.join(".state/events/t-0003-2-1.toml"),
        "id = \"t-0003-2-1\"\nop = \"t-0003-2-1\"\nthread = \"t-0003\"\nattempt = 2\ncreated = \"2026-09-20T00:00:00Z\"\n[recipient]\npane = \"w1:p1\"\ncoordinator_attempt = 3\n[payload.failed]\ntext = \"compiler failure\"\n",
    )
    .unwrap();
    let text = context(home.path(), "w1:p1", true);
    assert!(
        text.contains("failed — failure unknown: compiler failure event=t-0003-2-1"),
        "{text}"
    );
    assert!(text.contains("report draft: .state/threads/t-0003.md (not completion)"));
    assert!(text.contains("current wait"));
    assert!(text.contains("report draft: .state/threads/t-0001.md (not completion)"));
    assert!(!text.contains("superseded"));
    let receipt = project.join(".state/deliveries/t-0001-2-2.jsonl");
    let failed_receipt = project.join(".state/deliveries/t-0003-2-1.jsonl");
    assert!(!receipt.exists(), "peek never acknowledges");
    assert!(!failed_receipt.exists());
    context(home.path(), "w9:p9", false);
    assert!(!receipt.exists(), "another pane never acknowledges");
    assert!(!failed_receipt.exists());
    let cursor = project.join(".state/context-cursor.json");
    assert!(!cursor.exists(), "another pane must not consume the delta");
    assert!(context(home.path(), "w1:p1", false).contains("First read"));
    assert!(cursor.exists());
    let before = std::fs::read(&cursor).unwrap();
    context(home.path(), "w9:p9", false);
    assert_eq!(std::fs::read(&cursor).unwrap(), before);
    let deliveries = project.join(".state/deliveries");
    assert!(
        std::fs::read_to_string(deliveries.join("t-0003-2-1.jsonl"))
            .unwrap()
            .contains("acknowledged")
    );
    assert!(
        std::fs::read_to_string(deliveries.join("t-0001-2-2.jsonl"))
            .unwrap()
            .contains("acknowledged")
    );
    for id in ["t-0001-1-1", "t-0001-2-1", "t-0002-2-1"] {
        assert!(!deliveries.join(format!("{id}.jsonl")).exists());
    }
    assert!(!project.join(".state/inbox-counter.json").exists());
}

#[test]
fn a_sealed_done_does_not_hide_a_different_report() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    assert!(
        Command::new(BIN)
            .env_clear()
            .env("HOME", home.path())
            .args(["--root", root.to_str().unwrap(), "new", "demo"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let project = root.join("demo");
    std::fs::create_dir_all(project.join(".state/events")).unwrap();
    std::fs::create_dir(project.join(".state/threads")).unwrap();
    std::fs::create_dir(project.join(".state/artifacts")).unwrap();
    let sealed = b"sealed report\n";
    let sealed_hash = format!("{:x}", sha2::Sha256::digest(sealed));
    std::fs::write(project.join(".state/artifacts").join(&sealed_hash), sealed).unwrap();
    std::fs::write(
        project.join(".state/events/t-0001-1-1.toml"),
        format!("id = \"t-0001-1-1\"\nop = \"t-0001-1-1\"\nthread = \"t-0001\"\nattempt = 1\ncreated = \"2026-09-20T00:00:00Z\"\n[recipient]\npane = \"w1:p1\"\ncoordinator_attempt = 1\n[payload.done]\nsha = \"sealed-sha\"\nreport_path = \"old/location\"\nartifact = \"{sealed_hash}\"\n"),
    )
    .unwrap();
    let draft_dir = project.join("draft");
    std::fs::create_dir(&draft_dir).unwrap();
    std::fs::write(draft_dir.join("report.md"), "updated report\n").unwrap();
    for (hash, separate_report) in [(sealed_hash.as_str(), false), ("updated-report", true)] {
        std::fs::write(
            project.join(".state/threads/t-0001.toml"),
            format!("id = \"t-0001\"\nstatus = \"open\"\nattempt = 1\nreport_hash = \"{hash}\"\nthread_dir = {:?}\n", draft_dir.to_string_lossy()),
        )
        .unwrap();
        let text = context(home.path(), "w1:p1", true);
        assert!(text.contains(&format!(
            "done: sealed-sha report=.state/artifacts/{sealed_hash}"
        )));
        assert_eq!(text.contains("report draft:"), separate_report, "{text}");
    }

    std::fs::write(
        project.join(".state/threads/t-0001.toml"),
        "id = \"t-0001\"\nstatus = \"resolved\"\nattempt = 1\n",
    )
    .unwrap();
    let text = context(home.path(), "w1:p1", true);
    assert!(!text.contains("## Threads needing action"), "{text}");
    assert!(!text.contains("done: sealed-sha"), "{text}");
}
