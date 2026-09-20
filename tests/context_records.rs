//! Context receipts must follow the exact evidence shown, without inbox copies.
use std::path::Path;
use std::process::Command;

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
    std::fs::write(
        project.join(".state/coordinator.json"),
        r#"{"pane_id":"w1:p1","generation":3,"bootstrap":"acknowledged","launch":{"attempt":1,"brief_hash":"fixture"}}"#,
    )
    .unwrap();
    for id in ["t-0001", "t-0002", "t-0003"] {
        std::fs::write(
            project.join(format!("threads/{id}.toml")),
            format!("id = \"{id}\"\nstatus = \"open\"\nattempt = 2\nlast_group = \"idle\"\nreport_hash = \"report\"\n"),
        )
        .unwrap();
    }
    std::fs::create_dir_all(project.join("events")).unwrap();
    for (id, thread, attempt, generation, text) in [
        ("t-0001-1-1", "t-0001", 1, 3, "superseded attempt"),
        ("t-0001-2-1", "t-0001", 2, 3, "superseded message"),
        ("t-0001-2-2", "t-0001", 2, 3, "current wait"),
        ("t-0002-2-1", "t-0002", 2, 2, "earlier coordinator"),
    ] {
        std::fs::write(project.join(format!("events/{id}.toml")), format!(
            "id = \"{id}\"\nop = \"{id}\"\nthread = \"{thread}\"\nattempt = {attempt}\ncreated = \"2026-09-20T00:00:00Z\"\n[recipient]\npane = \"w1:p1\"\ncoordinator_attempt = {generation}\n[payload.waiting]\ntext = \"{text}\"\n"
        )).unwrap();
    }
    std::fs::write(
        project.join("events/t-0003-2-1.toml"),
        "id = \"t-0003-2-1\"\nop = \"t-0003-2-1\"\nthread = \"t-0003\"\nattempt = 2\ncreated = \"2026-09-20T00:00:00Z\"\n[recipient]\npane = \"w1:p1\"\ncoordinator_attempt = 3\n[payload.failed]\ntext = \"compiler failure\"\n",
    )
    .unwrap();
    let text = context(home.path(), "w1:p1", true);
    assert!(
        text.contains("failed: compiler failure event=t-0003-2-1"),
        "{text}"
    );
    assert!(text.contains("report: threads/t-0003.md"));
    assert!(text.contains("current wait"));
    assert!(text.contains("report: threads/t-0001.md"));
    assert!(!text.contains("superseded"));
    let receipt = project.join("deliveries/t-0001-2-2.jsonl");
    let failed_receipt = project.join("deliveries/t-0003-2-1.jsonl");
    assert!(!receipt.exists(), "peek never acknowledges");
    assert!(!failed_receipt.exists());
    context(home.path(), "w9:p9", false);
    assert!(!receipt.exists(), "another pane never acknowledges");
    assert!(!failed_receipt.exists());
    context(home.path(), "w1:p1", false);
    assert!(
        std::fs::read_to_string(&failed_receipt)
            .unwrap()
            .contains("acknowledged")
    );
    assert!(
        std::fs::read_to_string(&receipt)
            .unwrap()
            .contains("acknowledged")
    );
    for id in ["t-0001-1-1", "t-0001-2-1", "t-0002-2-1"] {
        assert!(!project.join(format!("deliveries/{id}.jsonl")).exists());
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
    std::fs::create_dir_all(project.join("events")).unwrap();
    std::fs::write(
        project.join("events/t-0001-1-1.toml"),
        "id = \"t-0001-1-1\"\nop = \"t-0001-1-1\"\nthread = \"t-0001\"\nattempt = 1\ncreated = \"2026-09-20T00:00:00Z\"\n[recipient]\npane = \"w1:p1\"\ncoordinator_attempt = 1\n[payload.done]\nsha = \"sealed-sha\"\nreport_path = \"artifacts/sealed-report\"\nartifact = \"sealed-report\"\n",
    )
    .unwrap();
    for (hash, separate_report) in [("sealed-report", false), ("updated-report", true)] {
        std::fs::write(
            project.join("threads/t-0001.toml"),
            format!("id = \"t-0001\"\nstatus = \"open\"\nattempt = 1\nreport_hash = \"{hash}\"\n"),
        )
        .unwrap();
        let text = context(home.path(), "w1:p1", true);
        assert!(text.contains("done: sealed-sha report=artifacts/sealed-report"));
        assert_eq!(
            text.contains("report: threads/t-0001.md (report bytes are not a completion)"),
            separate_report,
            "{text}"
        );
    }
}
