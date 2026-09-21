//! The rundown lists work, not completed records, and receipts cover only rows shown.
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_herdr-ade");

struct Project {
    home: tempfile::TempDir,
    dir: PathBuf,
}

impl Project {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let result = Command::new(BIN)
            .env_clear()
            .env("HOME", home.path())
            .args(["--root", root.to_str().unwrap(), "new", "demo"])
            .output()
            .unwrap();
        assert!(result.status.success());
        Self {
            home,
            dir: root.join("demo"),
        }
    }

    fn write(&self, path: impl AsRef<Path>, text: impl AsRef<[u8]>) {
        let path = self.dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn context(&self, peek: bool) -> String {
        let mut command = Command::new(BIN);
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("HERDR_PANE_ID", "w1:p1")
            .args([
                "--root",
                self.dir.parent().unwrap().to_str().unwrap(),
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

    fn round(&self, n: usize, phase: &str) {
        let mut record = format!(
            "round = \"r{n}\"\nphase = \"{phase}\"\nbranch = \"main\"\nplain = \"Work for round {n}\"\npolicy_hash = \"fixture\"\n[manifest]\nrevision = 1\nmembers = []\n"
        );
        if phase == "merged" {
            record.push_str("[merge]\nop = \"merge\"\nexpected_old = \"old\"\ncandidate = \"candidate\"\nverdict = \"verdict\"\nphase = \"checkpointed\"\nhead = \"checkpoint\"\n");
        }
        self.write(format!(".state/rounds/r{n}.toml"), record);
    }

    fn op(&self, id: &str, attempt: u32, state: &str) {
        self.write(format!("ops/{id}.toml"), format!("op = \"{id}\"\nrevision = 1\nthread = \"t-0001\"\nattempt = {attempt}\nkind = \"done\"\nhelper_pid = 1\nevent = \"{id}\"\nstate = \"{state}\"\ncreated = \"2026-09-20T00:00:00Z\"\n[recipient]\npane = \"w1:p1\"\ncoordinator_attempt = 1\n[requested]\nsha = \"sha\"\nreport_path = \"report.md\"\n"));
    }
}

#[test]
fn closed_rounds_are_counted_without_names_and_open_rounds_are_listed() {
    let p = Project::new();
    for n in 1..=44 {
        p.round(n, "merged");
    }
    p.round(45, "abandoned");
    p.round(46, "admitting");
    let text = p.context(true);
    assert!(
        text.contains("## Rounds (1 open; 44 merged, 1 abandoned not listed)"),
        "{text}"
    );
    assert!(text.contains("- r46 [Admitting] main — Work for round 46"));
    assert!(!text.contains("[Merged]"));
    assert!(!text.contains("[Abandoned]"));
    assert!(!text.contains("Work for round 1"));
    assert!(!text.contains("Work for round 45"));
}

#[test]
fn superseded_abandonment_and_obsolete_preparation_never_reach_digest() {
    let p = Project::new();
    p.write(
        "threads/t-0001.toml",
        "id = \"t-0001\"\nstatus = \"open\"\nattempt = 2\n",
    );
    p.op("t-0001-1-1", 1, "abandoned");
    p.op("t-0001-1-2", 1, "sealed");
    p.op("t-0001-1-3", 1, "staged");
    assert!(!p.context(true).contains("Completion preparation"));
    assert!(!p.context(true).contains("preparation-abandoned"));
    p.op("t-0001-2-1", 2, "reserved");
    let text = p.context(true);
    assert!(text.contains("## Completion preparation (1)"));
    assert!(text.contains("t-0001-2-1 attempt 2 (Reserved"));
    assert!(!text.contains("t-0001-1-"));
    p.write(
        "threads/t-0001.toml",
        "id = \"t-0001\"\nstatus = \"resolved\"\nattempt = 2\n",
    );
    assert!(!p.context(true).contains("Completion preparation"));
}

#[test]
fn terminal_preparation_does_not_hide_the_current_failure_or_change_records() {
    let p = Project::new();
    let thread = "id = \"t-0001\"\nstatus = \"failed\"\nattempt = 1\nerror = \"Report could not be sealed\"\n";
    p.write("threads/t-0001.toml", thread);
    // No successor operation: the terminal operation is still history, but
    // its unresolved thread must continue to tell the coordinator to act.
    p.op("t-0001-1-1", 1, "abandoned");
    let op_path = p.dir.join("ops/t-0001-1-1.toml");
    let before = std::fs::read(&op_path).unwrap();
    let text = p.context(true);
    assert!(text.contains("## Open threads (1)"), "{text}");
    assert!(text.contains("- t-0001 ["), "{text}");
    assert!(
        text.contains("failure unknown: Report could not be sealed"),
        "{text}"
    );
    assert!(!text.contains("Completion preparation"), "{text}");
    assert!(!text.contains("t-0001-1-1"), "{text}");
    assert_eq!(std::fs::read(&op_path).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(p.dir.join("threads/t-0001.toml")).unwrap(),
        thread
    );
}

#[test]
fn round_and_preparation_lists_are_bounded() {
    let p = Project::new();
    for n in 1..=21 {
        p.round(n, "admitting");
        p.op(&format!("t-0001-1-{n:02}"), 1, "staged");
    }
    let text = p.context(true);
    assert!(text.contains("- r20 [Admitting]"));
    assert!(!text.contains("- r21 [Admitting]"));
    assert!(text.contains("1 more; read .state/rounds/ (digest limit 20)"));
    assert!(text.contains("1 more; read ops/ (digest limit 20)"));
    assert!(!text.contains("t-0001-1-21"));
}

#[test]
fn inactive_routines_are_counted_and_live_routines_errors_and_memory_are_bounded() {
    let p = Project::new();
    p.write(
        "routines/retired.md",
        "+++\nschedule = \"every 1h\"\nenabled = false\n+++\nOld task\n",
    );
    let mut memory = String::new();
    for n in 1..=21 {
        p.write(
            format!("routines/live-{n:02}.md"),
            "+++\nschedule = \"every 1h\"\nenabled = true\n+++\nCheck the work\n",
        );
        p.write(format!("routines/broken-{n:02}.md"), "bad config");
        memory.push_str(&format!("- reference-{n:02}\n"));
    }
    p.write("MEMORY.md", memory);
    let text = p.context(true);
    assert!(text.contains("## Routines (21 enabled; 1 disabled not listed)"));
    assert!(text.contains("live-20"));
    assert!(!text.contains("live-21"));
    assert!(!text.contains("retired"));
    assert!(text.contains("config-error: routines/broken-20.md"));
    assert!(!text.contains("broken-21"));
    assert!(text.contains("reference-20"));
    assert!(!text.contains("reference-21"));
    assert!(text.contains("1 more; read MEMORY.md (digest limit 20)"));
    assert_eq!(
        text.matches("1 more; read routines/ (digest limit 20)")
            .count(),
        2
    );
}

#[test]
fn bounded_threads_and_inbox_do_not_receipt_hidden_rows() {
    let p = Project::new();
    p.write(
        ".state/coordinator.json",
        r#"{"pane_id":"w1:p1","generation":1,"bootstrap":"acknowledged","launch":{"attempt":1}}"#,
    );
    for n in 1..=21 {
        let id = format!("t-{n:04}");
        p.write(
            format!("threads/{id}.toml"),
            format!("id = \"{id}\"\nstatus = \"open\"\nattempt = 1\n"),
        );
        p.write(format!("events/{id}-1-1.toml"), format!("id = \"{id}-1-1\"\nop = \"{id}-1-1\"\nthread = \"{id}\"\nattempt = 1\ncreated = \"2026-09-20T00:00:00Z\"\n[recipient]\npane = \"w1:p1\"\ncoordinator_attempt = 1\n[payload.waiting]\ntext = \"wait-{n:04}\"\n"));
        p.write(format!("inbox/i-{n:04}.md"), format!("+++\nid = \"i-{n:04}\"\nkind = \"routine\"\nsubject = \"job\"\ncreated = \"2026-09-20T00:00:00Z\"\nsummary = \"message-{n:04}\"\n+++\n"));
    }
    let text = p.context(true);
    assert!(text.contains("1 more; read threads/ (digest limit 20)"));
    assert!(text.contains("1 more; read inbox/ (digest limit 20)"));
    assert!(text.contains("wait-0020"));
    assert!(!text.contains("wait-0021"));
    assert!(text.contains("message-0020"));
    assert!(!text.contains("message-0021"));
    // A valid coordinator receipt exercises event acknowledgements too.
    p.write(".state/coordinator.json", r#"{"pane_id":"w1:p1","generation":1,"bootstrap":"acknowledged","launch":{"attempt":1,"brief_hash":"fixture"}}"#);
    let result = Command::new(BIN)
        .env_clear()
        .env("HOME", p.home.path())
        .env("HERDR_PANE_ID", "w1:p1")
        .env("HERDR_ADE_LAUNCH", "demo/coordinator/1/fixture")
        .args([
            "--root",
            p.dir.parent().unwrap().to_str().unwrap(),
            "context",
            "demo",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(p.dir.join("deliveries/t-0020-1-1.jsonl").exists());
    assert!(!p.dir.join("deliveries/t-0021-1-1.jsonl").exists());
    let seen = std::fs::read_to_string(p.dir.join(".state/inbox-seen.json")).unwrap();
    assert!(seen.contains("i-0020"), "{seen}");
    assert!(!seen.contains("i-0021"), "{seen}");
}
