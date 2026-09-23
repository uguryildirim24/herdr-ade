use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_herdr-ade");

fn run(root: &Path, args: &[&str]) -> String {
    let result = Command::new(BIN)
        .env_clear()
        .env("HOME", root)
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}: {}",
        args.join(" "),
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

fn fixture(root: &Path) -> PathBuf {
    run(root, &["new", "adeherdr"]);
    let dir = root.join("adeherdr/.state/tasks");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 1..=20 {
        std::fs::write(
            dir.join(format!("job-{i:04}.toml")),
            format!(
                "id = 'job-{i:04}'\ntitle = 'Event {i}'\nauthority = ['request:q-fixture']\nacceptance = ['Confirm live event']\ncreated = '2026-09-23T00:00:00Z'\n[wait]\nkind = 'event'\ntarget = 'event-{i}'\nsnapshot = ''\nsince = '2026-09-23T00:00:00Z'\n"
            ),
        )
        .unwrap();
    }
    dir
}

#[test]
fn twenty_distinct_event_holds_take_one_line_on_second_context() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path());
    run(home.path(), &["context", "adeherdr"]);
    let text = run(home.path(), &["context", "adeherdr"]);
    let rows: Vec<_> = text.lines().filter(|line| line.starts_with("- ")).collect();
    assert_eq!(rows.len(), 1, "{text}");
    assert_eq!(
        rows[0],
        "- 20 task(s) wait to verify acceptance conditions; list with `ha task list adeherdr`"
    );
    assert_eq!(text.lines().count(), 34, "{text}");
}

#[test]
fn a_resolved_hold_or_new_action_returns_to_its_own_context_row() {
    let home = tempfile::tempdir().unwrap();
    let dir = fixture(home.path());
    let path = dir.join("job-0001.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        path,
        original.replace(
            "kind = 'event'\ntarget = 'event-1'\nsnapshot = ''",
            "kind = 'round'\ntarget = 'r1'\nsnapshot = 'Open'",
        ),
    )
    .unwrap();
    let rounds = home.path().join("adeherdr/.state/rounds");
    std::fs::create_dir_all(&rounds).unwrap();
    std::fs::write(
        rounds.join("r1.toml"),
        "round = 'r1'\nphase = 'abandoned'\nbranch = 'main'\nplain = 'Ended'\npolicy_hash = 'fixture'\n[manifest]\nrevision = 1\nmembers = []\n",
    )
    .unwrap();
    let path = dir.join("job-0002.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        path,
        original.replace("[wait]", "attempts = ['t-missing']\n[wait]"),
    )
    .unwrap();
    run(home.path(), &["context", "adeherdr"]);
    let text = run(home.path(), &["context", "adeherdr"]);
    assert!(
        text.contains("- 18 task(s) wait to verify acceptance conditions"),
        "{text}"
    );
    assert!(
        text.contains("- `job-0001` [open] Event 1 — next: verify 1 acceptance condition(s)"),
        "{text}"
    );
    assert!(
        text.contains("- `job-0002` [unknown] Event 2 — next: repair the missing attempt record"),
        "{text}"
    );
    assert!(text.contains("  waits on event: event-2"), "{text}");
}

#[test]
fn task_list_shows_each_active_hold_on_one_line() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path());
    let text = run(home.path(), &["task", "list", "adeherdr"]);
    assert_eq!(text.lines().count(), 20, "{text}");
    for i in 1..=20 {
        assert!(
            text.lines()
                .any(|line| line.starts_with(&format!("job-{i:04} "))
                    && line.contains(&format!("— waits on event: event-{i}"))),
            "{text}"
        );
    }
}
