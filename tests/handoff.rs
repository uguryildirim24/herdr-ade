//! The stdin command path used by the compaction mod.
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn handoff_reads_piped_note_saves_stdout_and_consumes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("root");
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_herdr-ade"));
        command
            .env_clear()
            .env("HOME", home.path())
            .args(["--root", root.to_str().unwrap()]);
        command
    };
    assert!(
        command()
            .args(["new", "demo", "--goal", "Fresh recovery"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let state = root.join("demo/.state");
    for name in ["context-cursor.json", "inbox-seen.json"] {
        std::fs::write(state.join(name), "unchanged receipt").unwrap();
    }
    let plain = command().args(["handoff", "demo"]).output().unwrap();
    assert!(
        plain.status.success(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    assert!(
        String::from_utf8(plain.stdout)
            .unwrap()
            .contains("Fresh recovery")
    );
    assert!(!state.join("handoffs").exists());
    let mut child = command()
        .args(["handoff", "demo", "--note-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Session note from stdin\n  exact indentation\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Session note from stdin\n  exact indentation\n"));
    assert!(text.chars().count() <= 16_000);
    let files: Vec<_> = std::fs::read_dir(state.join("handoffs"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(std::fs::read_to_string(files[0].path()).unwrap(), text);
    assert!(text.contains(files[0].path().to_str().unwrap()));
    for name in ["context-cursor.json", "inbox-seen.json"] {
        assert_eq!(
            std::fs::read_to_string(state.join(name)).unwrap(),
            "unchanged receipt"
        );
    }
}
