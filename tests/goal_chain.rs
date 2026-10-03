//! CLI boundaries for R5. Seals stand in for courier inputs, not native thinking.
//! Native trials must receive only PROMPT, on the same base/recipe/environment.
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PROMPT: &str = "Make source reading trustworthy.";
const OUTCOME: &str = "An offline coverage ledger distinguishes complete, partial and unavailable sources; no crawler or invented coverage.";
const GOOD: &str =
    "S1 complete primary page\nS2 partial appendix inaccessible\nS3 unavailable endpoint offline\n";
const INADEQUATE: &str =
    "S1 complete primary page\nS2 complete appendix inaccessible\nS3 complete endpoint offline\n";

struct Chain {
    home: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
}

impl Chain {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let state = root.join("demo/.state");
        let f = Self { home, root, state };
        f.ok(&["new", "demo", "--goal", PROMPT]);
        // A frozen request is the only human authority. No prewritten plan.
        f.write(
            "requests/q-source.json",
            &json!({"id":"q-source", "text":PROMPT, "at":"2026-10-03T05:32:00Z"}).to_string(),
        );
        f
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_herdr-ade"))
            .env_clear()
            .env("HOME", self.home.path())
            .current_dir(self.home.path())
            .args(["--root", self.root.to_str().unwrap(), "--json"])
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn refused(&self, args: &[&str], reason: &str) {
        let out = self.run(args);
        assert!(!out.status.success(), "unexpected success: {args:?}");
        let diagnostic = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(diagnostic.contains(reason), "{diagnostic}");
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.state.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn check(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.state.join("goal-check.json")).unwrap()).unwrap()
    }

    fn plan(&self) -> toml::Value {
        toml::from_str(&std::fs::read_to_string(self.state.join("plan.toml")).unwrap()).unwrap()
    }

    fn job(&self, n: u32, title: &str, acceptance: &str) -> String {
        self.ok(&[
            "task",
            "add",
            "demo",
            "--title",
            title,
            "--request",
            "q-source",
            "--acceptance",
            acceptance,
        ]);
        let id = format!("job-{n:04}");
        let path = format!("tasks/{id}.toml");
        let mut record: toml::Value =
            toml::from_str(&std::fs::read_to_string(self.state.join(&path)).unwrap()).unwrap();
        // Thread/courier records are the injected boundary; task creation,
        // plan dependencies, disposition validation and projections are real CLI.
        let lane = format!("t-{n:04}");
        record["attempts"] = toml::Value::Array(vec![toml::Value::String(lane.clone())]);
        self.write(&path, &toml::to_string(&record).unwrap());
        self.write(
            &format!("threads/{lane}.toml"),
            &format!("id = '{lane}'\nstatus = 'open'\nattempt = 1\n"),
        );
        id
    }

    // The judge's decision is an injected boundary, just like the courier seal;
    // criterion recording and closure enforcement use the real coordinator CLI.
    fn accept(&self, n: u32, established: bool, evidence: &str) {
        let lane = format!("t-{n:04}");
        let path = format!("threads/{lane}.toml");
        let mut record: toml::Value =
            toml::from_str(&std::fs::read_to_string(self.state.join(&path)).unwrap()).unwrap();
        record["status"] = toml::Value::String("resolved".into());
        self.write(&path, &toml::to_string(&record).unwrap());
        self.write(
            "coordinator.json",
            &json!({"pane_id":"w1:p1", "agent_name":"independent-coordinator", "generation":1})
                .to_string(),
        );
        let task: toml::Value = toml::from_str(
            &std::fs::read_to_string(self.state.join(format!("tasks/job-{n:04}.toml"))).unwrap(),
        )
        .unwrap();
        let condition = task["acceptance"][0].as_str().unwrap();
        let reason = format!(
            "[[acceptance]]\nthread = {lane:?}\nevent = \"{lane}-1-1\"\ncriterion = 1\ncondition = {condition:?}\nestablished = {established}\nevidence = {evidence:?}"
        );
        self.ok(&["thread", "attest", "demo", &lane, "--reason", &reason]);
    }

    fn seal(&self, n: u32, report: Option<&str>, failed: bool) -> Option<String> {
        let report = report?;
        let lane = format!("t-{n:04}");
        let id = format!("{lane}-1-1");
        let hash = format!("{:x}", Sha256::digest(report));
        self.write(&format!("artifacts/{hash}"), report);
        let payload = if failed {
            json!({"failed":{"text":report, "class":"work_failed"}})
        } else {
            json!({"done":{"has_changes":false, "sha":"fixture-base", "report_path":"report.md", "artifact":hash}})
        };
        let event = json!({"id":id, "op":id, "thread":lane, "attempt":1, "recipient":{"pane":"w1:p1", "coordinator_attempt":1}, "created":"2026-10-03T06:00:00Z", "payload":payload});
        self.write(
            &format!("events/{id}.toml"),
            &toml::to_string(&event).unwrap(),
        );
        Some(hash)
    }
}

#[test]
fn cli_articulates_revises_extends_and_closes_preserving_accepted_results() {
    let f = Chain::new();
    assert!(!f.state.join("plan.toml").exists());
    f.ok(&[
        "plan",
        "check",
        "demo",
        "wait",
        "research",
        "--condition",
        "contrary evidence inspected",
        "--evidence",
        "Investigate coverage before claiming a full read",
    ]);
    f.ok(&["plan", "set", "demo", "--does", OUTCOME]);
    let research = f.job(
        1,
        "Inspect source coverage",
        "Account for every assigned endpoint, including inaccessible scope",
    );
    let build = f.job(
        2,
        "Deliver ledger",
        "Account for S1-S3 offline without claiming complete reading",
    );
    f.ok(&[
        "plan",
        "step",
        "add",
        "demo",
        "Investigate full-read assumption",
        "--task",
        &research,
    ]);
    f.ok(&[
        "plan",
        "step",
        "add",
        "demo",
        "Write full-read summary",
        "--task",
        &build,
        "--after",
        "s-1",
    ]);
    f.refused(
        &[
            "plan",
            "check",
            "demo",
            "action",
            &build,
            "--evidence",
            "Skip the research",
        ],
        "goal_check",
    );
    let research_hash = f.seal(1, Some("S2 appendix inaccessible; S3 offline. Critique: a full-read summary would invent evidence; use a coverage ledger."), false).unwrap();
    f.accept(
        1,
        true,
        "Research artifact: coordinator checked honest endpoint accounting and the scope revision",
    );
    f.ok(&["plan", "sync", "demo"]);
    f.ok(&[
        "plan",
        "step",
        "edit",
        "demo",
        "s-2",
        "Deliver honest coverage instead of a full-read summary",
    ]);
    f.ok(&["plan", "check", "demo", "action", &build, "--evidence", "Contrary inspected evidence invalidated the full-read assumption; keep the accepted research"]);
    let ledger_hash = f.seal(2, Some(GOOD), false).unwrap();
    assert_eq!(judge_ledger(Some(GOOD)), Some(true));
    f.accept(
        2,
        true,
        "Ledger artifact: independent grader confirmed S1 complete, S2 partial, S3 unavailable",
    );
    f.ok(&["plan", "sync", "demo"]);
    let initial = f.plan()["steps"].as_array().unwrap().clone();
    assert!(initial.iter().all(|s| s["state"].as_str() == Some("done")));
    let verify = f.job(
        3,
        "Offline acceptance",
        "Recheck S1-S3 coverage offline; missing evidence is not complete",
    );
    f.ok(&[
        "plan",
        "step",
        "add",
        "demo",
        "Recheck ledger offline: omitted from initial checklist",
        "--task",
        &verify,
        "--after",
        "s-2",
    ]);
    f.refused(
        &[
            "plan",
            "check",
            "demo",
            "close",
            "--task",
            &build,
            "--evidence",
            "The initial checklist is done",
        ],
        "unfinished",
    );
    f.ok(&[
        "plan",
        "check",
        "demo",
        "action",
        &verify,
        "--evidence",
        "Initial checklist omitted the outcome's offline recheck; add only that work",
    ]);
    assert_eq!(&f.plan()["steps"].as_array().unwrap()[..2], &initial[..]);
    let delivered = std::fs::read_to_string(f.state.join("artifacts").join(&ledger_hash)).unwrap();
    assert_eq!(judge_ledger(Some(&delivered)), Some(true));
    f.seal(3, Some("Offline acceptance: three assigned sources, each accounted for once. S2 stays partial, S3 stays unavailable; no additional features required."), false);
    f.accept(3, true, "Offline recheck artifact: coordinator confirmed every source accounted for without invented completeness");
    f.ok(&["plan", "sync", "demo"]);
    f.ok(&[
        "plan",
        "check",
        "demo",
        "close",
        "--task",
        &research,
        "--task",
        &build,
        "--task",
        &verify,
        "--evidence",
        "Sealed research, coverage ledger and offline recheck establish the bounded outcome",
    ]);
    let closed = f.check();
    let plan = f.plan();
    assert_eq!(closed["disposition"]["kind"], "closed");
    // New CLI processes exercise persisted reconciliation, not a memory cache.
    for _ in 0..3 {
        f.ok(&[
            "plan",
            "check",
            "demo",
            "close",
            "--task",
            &research,
            "--task",
            &build,
            "--task",
            &verify,
            "--evidence",
            "Sealed research, coverage ledger and offline recheck establish the bounded outcome",
        ]);
        assert_eq!(f.check(), closed);
        assert_eq!(f.plan(), plan);
    }
    assert_eq!(
        std::fs::read_to_string(f.state.join("artifacts").join(research_hash)).unwrap(),
        "S2 appendix inaccessible; S3 offline. Critique: a full-read summary would invent evidence; use a coverage ledger."
    );
    assert_eq!(
        std::fs::read_dir(f.state.join("requests")).unwrap().count(),
        1
    );
    // Reuse the real handoff boundary without another compaction quiz.
    let handoff = f.ok(&["handoff", "demo"]);
    let text = handoff.to_string();
    assert!(text.contains(PROMPT) && text.contains("Goal check closed"));
}

// Fixed known-good/inadequate/missing witnesses qualify this task's narrow
// outcome check. This is not a model judge or a pass-rate/feature-count score.
fn judge_ledger(report: Option<&str>) -> Option<bool> {
    report.map(|text| {
        let mut expected = std::collections::BTreeMap::from([
            ("S1", "complete"),
            ("S2", "partial"),
            ("S3", "unavailable"),
        ]);
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let (Some(source), Some(coverage), Some(_evidence)) =
                (words.next(), words.next(), words.next())
            else {
                return false;
            };
            if expected.remove(source) != Some(coverage) {
                return false;
            }
        }
        expected.is_empty()
    })
}

#[test]
fn grader_witnesses_distinguish_outcome_from_no_change_completion() {
    for (report, failed, judgment) in [
        (Some(GOOD), false, Some(true)),
        (Some(INADEQUATE), false, Some(false)),
        (None, false, None),
        (
            Some("Partial read sealed as failed; appendix not inspected"),
            true,
            Some(false),
        ),
    ] {
        let f = Chain::new();
        f.ok(&["plan", "set", "demo", "--does", OUTCOME]);
        let task = f.job(
            1,
            "Coverage ledger",
            "Every assigned source accounted for honestly",
        );
        f.ok(&[
            "plan",
            "step",
            "add",
            "demo",
            "Deliver ledger",
            "--task",
            &task,
        ]);
        let hash = f.seal(1, report, failed);
        f.ok(&["plan", "sync", "demo"]);
        let artifact =
            hash.map(|hash| std::fs::read_to_string(f.state.join("artifacts").join(hash)).unwrap());
        assert_eq!(judge_ledger(artifact.as_deref()), judgment);
        let event_path = f.state.join("events/t-0001-1-1.toml");
        if event_path.exists() {
            let event: toml::Value =
                toml::from_str(&std::fs::read_to_string(event_path).unwrap()).unwrap();
            assert!(event.get("usage").is_none(), "unknown usage is not zero");
        }
        let close = [
            "plan",
            "check",
            "demo",
            "close",
            "--task",
            &task,
            "--evidence",
            "Claimed coverage evidence",
        ];
        if report.is_none() || failed {
            f.refused(&close, "terminal acceptance-bearing evidence");
            assert_ne!(f.plan()["steps"][0]["state"].as_str(), Some("done"));
        } else {
            // A no-change seal is a finish fact, never independent acceptance.
            f.refused(&close, "acceptance not established");
            assert_eq!(f.plan()["steps"][0]["state"].as_str(), Some("done"));
            f.accept(
                1,
                judgment == Some(true),
                "Independent ledger grader: S2 appendix inaccessible; S3 endpoint offline",
            );
            if judgment == Some(true) {
                f.ok(&close);
                assert_eq!(f.check()["disposition"]["kind"], "closed");
            } else {
                f.refused(&close, "acceptance not established");
                assert_ne!(f.check()["disposition"]["kind"], "closed");
            }
        }
    }
}
