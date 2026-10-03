//! A context receipt covers only the rows successfully delivered to stdout.
use std::path::PathBuf;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_herdr-ade");

struct Fixture {
    home: tempfile::TempDir,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
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
        let fixture = Self {
            home,
            project: root.join("demo"),
        };
        fixture.write(".state/coordinator.json", r#"{"pane_id":"w1:p1","generation":1,"bootstrap":"acknowledged","launch":{"attempt":1,"brief_hash":"fixture"}}"#);
        fixture
    }

    fn write(&self, path: &str, text: impl AsRef<[u8]>) {
        let path = self.project.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(BIN);
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("HERDR_PANE_ID", "w1:p1")
            .env("HERDR_ADE_LAUNCH", "demo/coordinator/1/fixture")
            .args([
                "--root",
                self.project.parent().unwrap().to_str().unwrap(),
                "context",
                "demo",
            ]);
        command
    }

    fn read(&self) -> String {
        let output = self.command().output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn plan(&self, count: usize) {
        let mut plan =
            String::from("schema = 1\nrevision = 1\nkind = 'tool'\ndoes = 'fixture outcome'\n");
        for n in 1..=count {
            plan.push_str(&format!(
                "\n[[steps]]\nid = 's-{n:03}'\ntext = 'fixture step {n}'\n"
            ));
        }
        self.write(".state/plan.toml", plan);
    }

    fn wait(&self) {
        self.write(
            ".state/threads/t-0001.toml",
            "id = 't-0001'\nstatus = 'open'\nattempt = 1\ntitle = 'fixture lane'\n",
        );
        self.write(".state/events/t-0001-1-1.toml", "id = 't-0001-1-1'\nop = 't-0001-1-1'\nthread = 't-0001'\nattempt = 1\ncreated = '2026-10-02T00:00:00Z'\n[recipient]\npane = 'w1:p1'\ncoordinator_attempt = 1\n[payload.waiting]\ntext = 'fixture waiting reason'\n");
    }
}

#[test]
fn overflow_is_shown_exactly_once_across_successive_reads() {
    let fixture = Fixture::new();
    fixture.read();
    // The previous on-disk shape had no event map and had separate task
    // completion receipts. It must still be a usable cursor, not a first read.
    let path = fixture.project.join(".state/context-cursor.json");
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let object = old.as_object_mut().unwrap();
    object.remove("events");
    object.insert("completed".into(), serde_json::json!({}));
    object.insert("completion_receipt".into(), serde_json::json!(true));
    std::fs::write(path, serde_json::to_vec(&old).unwrap()).unwrap();
    fixture.plan(21);
    let first = fixture.read();
    let second = fixture.read();
    assert!(!first.contains("First read"), "{first}");
    let third = fixture.read();
    assert!(!third.contains("fixture step"), "{third}");
    let combined = format!("{first}{second}{third}");
    assert_eq!(combined.matches("- Plan outcome:").count(), 1);
    for n in 1..=21 {
        assert_eq!(
            combined.matches(&format!("- Plan s-{n:03}:")).count(),
            1,
            "{combined}"
        );
    }
}

#[test]
fn an_event_behind_plan_changes_is_acknowledged_only_when_rendered() {
    let fixture = Fixture::new();
    fixture.read();
    fixture.plan(21);
    fixture.wait();
    let receipt = fixture
        .project
        .join(".state/deliveries/t-0001-1-1/00000001.json");
    let first = fixture.read();
    assert!(!first.contains("fixture waiting reason"), "{first}");
    assert!(!receipt.exists());
    let second = fixture.read();
    assert!(second.contains("- Event t-0001-1-1:"), "{second}");
    assert!(second.contains("fixture waiting reason"));
    assert!(
        std::fs::read_to_string(&receipt)
            .unwrap()
            .contains("acknowledged")
    );
    let receipts = std::fs::read(&receipt).unwrap();
    assert!(!fixture.read().contains("fixture waiting reason"));
    assert_eq!(std::fs::read(&receipt).unwrap(), receipts);
}

#[test]
fn a_failing_writer_consumes_nothing_in_text_or_json() {
    for json in [false, true] {
        let fixture = Fixture::new();
        fixture.read();
        let cursor = fixture.project.join(".state/context-cursor.json");
        let before = std::fs::read(&cursor).unwrap();
        fixture.plan(21);
        fixture.wait();
        fixture.write(".state/inbox/i-fixture.md", "+++\nid = 'i-fixture'\nkind = 'note'\nsubject = 'fixture'\ncreated = 'x'\nsummary = 'unread inbox message'\n+++\n");
        let mut command = fixture.command();
        if json {
            command.arg("--json");
        }
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let failed = command.stdout(Stdio::from(full)).output().unwrap();
        assert!(!failed.status.success(), "json={json}");
        assert_eq!(std::fs::read(&cursor).unwrap(), before);
        assert!(!fixture.project.join(".state/inbox-seen.json").exists());
        assert!(
            !fixture
                .project
                .join(".state/deliveries/t-0001-1-1")
                .exists()
        );
        assert!(fixture.read().contains("- Plan s-001:"));
        let second = fixture.read();
        assert!(second.contains("unread inbox message"), "{second}");
        assert!(second.contains("fixture waiting reason"), "{second}");
    }
}

#[test]
fn changed_standing_instructions_show_the_words_and_remain_unread_in_overflow() {
    let fixture = Fixture::new();
    fixture.read();
    fixture.plan(21);
    fixture.write(".state/notes.jsonl", "{\"schema\":1,\"id\":\"n-fixture\",\"kind\":\"instruction\",\"at\":\"2026-10-02T00:00:00Z\",\"request\":\"q-fixture\",\"text\":\"Keep the fixture instruction visible.\"}\n");
    let first = fixture.read();
    assert!(!first.contains("Keep the fixture instruction visible."));
    let second = fixture.read();
    assert!(
        second.contains("Keep the fixture instruction visible."),
        "{second}"
    );
    assert!(
        !fixture
            .read()
            .contains("Keep the fixture instruction visible.")
    );
}
