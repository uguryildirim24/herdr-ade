//! Context renders one bounded set, with project orientation on the first read.
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
        command.env_clear().env("HOME", self.home.path()).args([
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
}

#[test]
fn context_starts_with_orientation_and_one_action_item_set() {
    let p = Project::new();
    p.write(
        ".state/threads/t-0001.toml",
        "id = \"t-0001\"\ntitle = \"Needs help\"\nstatus = \"failed\"\nattempt = 1\nerror = \"compiler failure\"\n",
    );
    let text = p.context(false);
    assert_eq!(text.matches("t-0001").count(), 1, "{text}");
    assert!(text.contains("compiler failure"), "{text}");
}

#[test]
fn context_shows_the_words_inside_a_pasted_message() {
    let p = Project::new();
    p.write(
        ".state/talk/journal.jsonl",
        concat!(
            "{\"seq\":1,\"at\":\"2026-09-22T00:00:00Z\",\"rolf\":{",
            "\"request\":\"q-paste\",",
            "\"text\":\"\\n<pasted_content id=\\\"2460\\\">\\nKeep these exact words.\\n</pasted_content id=\\\"2460\\\">\"}}\n"
        ),
    );

    let text = p.context(true);
    assert!(text.contains("Keep these exact words."), "{text}");
    assert!(!text.contains("<pasted_content id="), "{text}");
}
