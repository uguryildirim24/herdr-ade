use std::path::{Path, PathBuf};

use crate::contracts::{DonePayload, Event, EventPayload, Recipient, WaitingPayload};
use crate::project::Project;
use crate::runner::fake::ok;
use crate::runner::{RealRunner, Runner};
use crate::scenarios::World;
use crate::thread::{self, Kind, Status, sha256_hex};

/// Structured target facts for remote diagnostic fixtures. No host `df` runs.
pub fn diagnostic_output(
    cmd: &crate::runner::Cmd,
    free_kb: u64,
    refusal: Option<&str>,
) -> crate::runner::Output {
    use crate::pi::doctor::{FailureEvidence, Row};
    let input: serde_json::Value = serde_json::from_str(cmd.stdin.as_deref().unwrap()).unwrap();
    let input = &input["request"]["Doctor"];
    let mut rows = Vec::new();
    if let Some(disk) = input["disk"].as_array() {
        let free = free_kb as f64 * 1024.0 / 1_000_000_000.0;
        rows.push(if free >= disk[1].as_f64().unwrap() {
            Row::ok("disk", format!("{free:.1} GB free"))
        } else {
            Row::fail(
                "disk",
                format!(
                    "disk_low: {free:.1} GB free under {}",
                    disk[0].as_str().unwrap()
                ),
            )
        });
    }
    for native in input["natives"].as_array().unwrap() {
        rows.push(Row::ok(
            format!("recipe {}", native[0].as_str().unwrap()),
            "selected model ready",
        ));
    }
    for pair in input["models"].as_array().unwrap() {
        let label = format!(
            "provider {}/{}",
            pair[0].as_str().unwrap(),
            pair[1].as_str().unwrap()
        );
        let label = input["pi_ids"][&label]
            .as_array()
            .map_or(label.clone(), |ids| {
                format!(
                    "recipe {} ({label})",
                    ids.iter()
                        .map(|id| id.as_str().unwrap())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
        rows.push(Row::ok(label, "login ready"));
    }
    if let Some(detail) = refusal
        && let Some(row) = rows.iter_mut().find(|row| row.label != "disk")
    {
        row.level = crate::pi::doctor::Level::Fail;
        row.detail = format!("pi_not_ready: {detail}");
        row.evidence = if crate::pi::doctor::positive_sign_in_evidence(detail) {
            FailureEvidence::Provider
        } else {
            FailureEvidence::Unknown
        };
    }
    ok(&crate::box_helper::tests::ready(
        serde_json::json!({"rows": rows, "snapshot": {"panes": null, "agents": null, "builds": null, "build_sizes": {}, "build_error": null}}),
    ))
}

/// A failed courier updates observation evidence, never lane lifecycle or work.
pub fn assert_failed_observation_only(before: &thread::Thread, after: &thread::Thread) {
    assert_eq!(after.observation_source, "courier");
    assert!(!after.observation_attempted.is_empty());
    assert!(after.observation_error.contains("unreachable"));
    let mut normalized = after.clone();
    normalized.observation_source = before.observation_source.clone();
    normalized.observation_attempted = before.observation_attempted.clone();
    normalized.observation_error = before.observation_error.clone();
    normalized.updated = before.updated.clone();
    assert_eq!(
        toml::to_string(&normalized).unwrap(),
        toml::to_string(before).unwrap()
    );
}

pub struct Fx {
    pub world: World,
    pub project: Project,
    pub repo: PathBuf,
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub fn commit_file(dir: &Path, path: &str, text: &str, message: &str) -> String {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();
    git(dir, &["add", "--", path]);
    git(dir, &["commit", "-q", "--no-verify", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

pub fn fixture() -> Fx {
    let world = World::new();
    world
        .runner
        .on_fn(|cmd| cmd.program == "git", |cmd| RealRunner.run(cmd));
    world.runner.on("notification show", ok(r#"{"result":{}}"#));
    let repo = world.home.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo = std::fs::canonicalize(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join(".git/info/exclude"), ".worktrees/\n").unwrap();
    commit_file(&repo, "README.md", "hello\n", "initial");
    let project = world.project("demo", "a.sock");
    world.add_repo(&project, &repo.to_string_lossy());
    Fx {
        world,
        project,
        repo,
    }
}

impl Fx {
    /// A lane thread on its own worktree with one commit; returns (id, sha).
    pub fn lane(&self, n: u32) -> (String, String) {
        let wt = self.repo.join(".worktrees").join(format!("lane-{n}"));
        git(
            &self.repo,
            &[
                "worktree",
                "add",
                "-q",
                &wt.to_string_lossy(),
                "-b",
                &format!("lane/{n}"),
                "main",
            ],
        );
        let sha = commit_file(
            &wt,
            &format!("src/lane{n}.rs"),
            &format!("// lane {n}\n"),
            &format!("lane {n}"),
        );
        let t = thread::allocate(&self.project, |t| {
            t.title = format!("Lane {n}");
            t.kind = Kind::Worktree;
            t.status = Status::Open;
            t.agent = "claude".into();
            t.workspace_id = "w1".into();
            t.tab_id = format!("w1:t{}", n + 10);
            t.pane_id = format!("w1:p{}", n + 10);
            t.repo = self.repo.to_string_lossy().into_owned();
            t.worktree_path = wt.to_string_lossy().into_owned();
            t.cwd = t.worktree_path.clone();
            t.branch = format!("lane/{n}");
        })
        .unwrap();
        (t.id, sha)
    }

    /// A plain thread record without a worktree (a reviewer, say).
    pub fn thread(&self, title: &str) -> String {
        thread::allocate(&self.project, |t| {
            t.title = title.into();
            t.status = Status::Open;
            t.agent = "claude".into();
            t.pane_id = "w1:p9".into();
        })
        .unwrap()
        .id
    }

    /// Writes the artifact and a sealed `done` event, as A2's seal would.
    pub fn seal_done(&self, id: &str, attempt: u32, n: u32, sha: &str, report: &str) -> String {
        let artifact = sha256_hex(report.as_bytes());
        let dir = self.project.state_dir().join("artifacts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(&artifact), report).unwrap();
        self.seal(
            id,
            attempt,
            n,
            EventPayload {
                done: Some(DonePayload {
                    has_changes: None,
                    sha: sha.into(),
                    report_path: format!(".reports/{id}.md"),
                    artifact,
                    attestation: None,
                    published_ref: None,
                }),
                waiting: None,
                failed: None,
            },
        )
    }

    pub fn seal_waiting(&self, id: &str, attempt: u32, n: u32, text: &str) -> String {
        self.seal(
            id,
            attempt,
            n,
            EventPayload {
                done: None,
                waiting: Some(WaitingPayload {
                    text: text.into(),
                    ..Default::default()
                }),
                failed: None,
            },
        )
    }

    fn seal(&self, id: &str, attempt: u32, n: u32, payload: EventPayload) -> String {
        let event_id = format!("{id}-{attempt}-{n}");
        let dir = self.project.state_dir().join("events");
        std::fs::create_dir_all(&dir).unwrap();
        let event = Event {
            id: event_id.clone(),
            op: event_id.clone(),
            thread: id.into(),
            attempt,
            recipient: Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 0,
            },
            usage: None,
            created: format!("2026-09-18T10:{:02}:{:02}Z", attempt, n),
            payload,
        };
        std::fs::write(
            dir.join(format!("{event_id}.toml")),
            toml::to_string(&event).unwrap(),
        )
        .unwrap();
        event_id
    }
}
