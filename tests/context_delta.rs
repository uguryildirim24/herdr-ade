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
        self.wait_number(1);
    }

    fn wait_number(&self, n: usize) {
        let thread = format!("t-{n:04}");
        let event = format!("{thread}-1-1");
        self.write(
            &format!(".state/threads/{thread}.toml"),
            format!("id = '{thread}'\nstatus = 'open'\nattempt = 1\ntitle = 'fixture lane'\n"),
        );
        self.write(&format!(".state/events/{event}.toml"), format!("id = '{event}'\nop = '{event}'\nthread = '{thread}'\nattempt = 1\ncreated = '2026-10-02T00:00:00Z'\n[recipient]\npane = 'w1:p1'\ncoordinator_attempt = 1\n[payload.waiting]\ntext = 'fixture waiting reason'\n"));
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
fn events_precede_plan_history_and_overflow_is_acknowledged_only_when_rendered() {
    let fixture = Fixture::new();
    fixture.read();
    fixture.plan(21);
    for n in 1..=21 {
        fixture.wait_number(n);
    }
    let receipt = fixture
        .project
        .join(".state/deliveries/t-0021-1-1/00000001.json");
    let first = fixture.read();
    assert!(first.contains("fixture waiting reason"), "{first}");
    assert!(!first.contains("- Plan s-001:"), "{first}");
    assert!(
        fixture
            .project
            .join(".state/deliveries/t-0001-1-1/00000001.json")
            .exists()
    );
    assert!(!receipt.exists());
    let second = fixture.read();
    assert!(second.contains("- Event t-0021-1-1:"), "{second}");
    assert!(second.contains("fixture waiting reason"));
    assert!(
        std::fs::read_to_string(&receipt)
            .unwrap()
            .contains("acknowledged")
    );
    let receipts = std::fs::read(&receipt).unwrap();
    assert!(!fixture.read().contains("- Event t-0021-1-1:"));
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
        // A closed socket reader makes every stdout write fail on Linux and
        // macOS, without relying on Linux's /dev/full device.
        let (writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(reader);
        let writer: std::os::fd::OwnedFd = writer.into();
        let failed = command.stdout(Stdio::from(writer)).output().unwrap();
        assert!(!failed.status.success(), "json={json}");
        assert_eq!(std::fs::read(&cursor).unwrap(), before);
        assert!(!fixture.project.join(".state/inbox-seen.json").exists());
        assert!(
            !fixture
                .project
                .join(".state/deliveries/t-0001-1-1")
                .exists()
        );
        let replay = fixture.read();
        for row in [
            "- Plan s-001:",
            "unread inbox message",
            "fixture waiting reason",
        ] {
            assert!(replay.contains(row), "{replay}");
        }
        let second = fixture.read();
        assert!(!second.contains("unread inbox message"), "{second}");
        assert!(!second.contains("fixture waiting reason"), "{second}");
    }
}

#[test]
fn failed_receipts_leave_the_cursor_replayable() {
    let fixture = Fixture::new();
    fixture.read();
    let cursor = fixture.project.join(".state/context-cursor.json");
    let before = std::fs::read(&cursor).unwrap();
    fixture.wait();
    fixture.write(".state/inbox/i-fixture.md", "+++\nid = 'i-fixture'\nkind = 'courier-delivery'\nevent = 't-0001-1-1'\nsummary = 'receipt replay'\n+++\n");
    // Interrupt receipt persistence after output and inbox-seen persistence.
    fixture.write(".state/deliveries", "blocked");
    let failed = fixture.command().output().unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stdout).contains("receipt replay"));
    assert!(fixture.project.join(".state/inbox-seen.json").exists());
    assert_eq!(std::fs::read(&cursor).unwrap(), before);
    std::fs::remove_file(fixture.project.join(".state/deliveries")).unwrap();
    assert!(fixture.read().contains("receipt replay"));
    let receipt = fixture
        .project
        .join(".state/deliveries/t-0001-1-1/00000001.json");
    assert!(
        std::fs::read_to_string(&receipt)
            .unwrap()
            .contains("acknowledged")
    );
    assert!(!fixture.read().contains("receipt replay"));
    assert_eq!(
        std::fs::read_dir(receipt.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn changed_standing_instructions_show_the_words_before_historical_overflow() {
    let fixture = Fixture::new();
    fixture.read();
    fixture.plan(21);
    fixture.write(".state/notes.jsonl", "{\"schema\":1,\"id\":\"n-fixture\",\"kind\":\"instruction\",\"at\":\"2026-10-02T00:00:00Z\",\"request\":\"q-fixture\",\"text\":\"Keep the fixture instruction visible.\"}\n");
    let first = fixture.read();
    assert!(
        first.contains("Keep the fixture instruction visible."),
        "{first}"
    );
    let second = fixture.read();
    assert!(
        !second.contains("Keep the fixture instruction visible."),
        "{second}"
    );
    assert!(
        !fixture
            .read()
            .contains("Keep the fixture instruction visible.")
    );
}
