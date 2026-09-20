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
    for id in ["t-0001", "t-0002"] {
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
    let text = context(home.path(), "w1:p1", true);
    assert!(text.contains("current wait"));
    assert!(text.contains("report: threads/t-0001.md"));
    assert!(!text.contains("superseded"));
    let receipt = project.join("deliveries/t-0001-2-2.jsonl");
    assert!(!receipt.exists(), "peek never acknowledges");
    context(home.path(), "w9:p9", false);
    assert!(!receipt.exists(), "another pane never acknowledges");
    context(home.path(), "w1:p1", false);
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
