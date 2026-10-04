//! Snapshots consume the built overview command, not a hand-written reply.
#[path = "../src/rundown/view.rs"]
mod view;

use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

fn write(path: impl AsRef<Path>, text: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn ade(home: &Path, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_herdr-ade"))
        .env_clear()
        .env("HOME", home)
        .env("HERDR_BIN_PATH", home.join("herdr"))
        .args(["--root", home.join("root").to_str().unwrap(), "--json"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn record(home: &Path, project: &str, file: &str, value: Value) {
    write(
        home.join("root").join(project).join(".state").join(file),
        toml::to_string(&value).unwrap(),
    );
}

fn fixture(home: &Path) -> jiff::Timestamp {
    use std::os::unix::fs::PermissionsExt;
    write(
        home.join("herdr"),
        "#!/bin/sh\nprintf '%s\\n' '{\"result\":{\"agents\":[],\"panes\":[]}}'\n",
    );
    std::fs::set_permissions(home.join("herdr"), std::fs::Permissions::from_mode(0o755)).unwrap();
    for project in ["adeherdr", "quiet"] {
        ade(home, &["new", project]);
    }
    let now = jiff::civil::date(2026, 10, 4)
        .at(12, 0, 0, 0)
        .to_zoned(jiff::tz::TimeZone::system())
        .unwrap()
        .timestamp();
    let at = |seconds: i64| {
        jiff::Timestamp::from_second(now.as_second() - seconds)
            .unwrap()
            .to_string()
    };
    let mut steps = Vec::new();
    for (index, seconds) in [60, 3600, 7200, 10800, 14400, 18000, 21600, 86400, 90000]
        .into_iter()
        .enumerate()
    {
        let id = format!("job-{:04}", index + 1);
        record(
            home,
            "adeherdr",
            &format!("tasks/{id}.toml"),
            json!({"id":id, "title":"private task identity", "authority":["request:fixture"], "acceptance":["change works"], "installed":[{"at":at(seconds), "command":"installation verified"}]}),
        );
        steps.push(json!({"id":format!("s-{index}"), "text":format!("Finished change {}", index + 1), "state":"left", "tasks":[id]}));
    }
    write(home.join("socket"), "");
    write(
        home.join("root/adeherdr/.state/coordinator.json"),
        json!({"socket":home.join("socket"), "pane_id":"w1:p1"}).to_string(),
    );
    record(
        home,
        "adeherdr",
        "threads/t-0040.toml",
        json!({"id":"t-0040", "status":"open", "attempt":1, "machine":"oci", "last_group":"working", "last_state":"working", "last_state_change":at(2400), "last_observed":now.to_string()}),
    );
    record(
        home,
        "adeherdr",
        "tasks/job-0010.toml",
        json!({"id":"job-0010", "title":"Visible changes", "authority":["request:fixture"], "acceptance":["change works"], "attempts":["t-0040"]}),
    );
    steps.push(json!({"id":"s-now", "text":"Make tomorrow’s changes visible", "state":"left", "tasks":["job-0010"]}));
    record(
        home,
        "adeherdr",
        "plan.toml",
        json!({"schema":1, "revision":1, "next_step":20, "goal":"A screen you open.", "kind":"screen", "what_you_get":"A screen you open.", "does":"See what changed without asking.", "steps":steps}),
    );
    write(
        home.join("root/adeherdr/.state/inbox/login.md"),
        "+++\nid = \"login\"\nkind = \"login\"\nsubject = \"t-0099\"\nsummary = \"Sign in through your browser\"\ncreated = \"2026-10-04T10:00:00Z\"\n+++\nSign in through your browser\n",
    );
    write(home.join("root/adeherdr/.state/goal-check.json"), json!({"waits":[
        [{"kind":"wait","tasks":["job-0010"],"party":"Rolf","condition":"Choose the next direction"},"evidence"],
        [{"kind":"wait","tasks":[],"party":"harness","condition":"Internal retry"},"evidence"]
    ]}).to_string());
    now
}

fn snapshot(name: &str, text: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    if std::env::var_os("UPDATE_RUNDOWN_SNAPSHOTS").is_some() {
        write(&path, text);
    }
    assert_eq!(text, std::fs::read_to_string(path).unwrap());
}

#[test]
fn real_overview_activity_and_quiet_card_snapshots() {
    let home = tempfile::tempdir().unwrap();
    let now = fixture(home.path());
    for (project, snapshot_name) in [
        ("adeherdr", "rundown-active.txt"),
        ("quiet", "rundown-quiet.txt"),
    ] {
        let reply = ade(home.path(), &["overview", project]);
        let card = view::Card::from_view("", &reply).unwrap();
        let text = view::render_at(&card, 80, 0, "", now)
            .iter()
            .map(|line| view::visible(line))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        snapshot(snapshot_name, &text);
        if project == "adeherdr" {
            assert!(text.contains("for 40 min"), "{text}");
            assert!(text.contains("Sign in through your browser"), "{text}");
            assert!(text.contains("Choose the next direction"));
            assert!(!text.contains("Internal retry"));
            assert!(text.contains("and 1 more"));
            assert!(text.contains("9 of 10"));
            for id in ["t-0040", "t-0099", "job-", "private task identity", "oci"] {
                assert!(!text.contains(id));
            }
            for width in 1..100 {
                let lines = view::render_at(&card, width, 0, "", now);
                assert!(
                    lines
                        .iter()
                        .all(|line| view::visible(line).chars().count() <= width),
                    "width {width}"
                );
            }
        } else {
            for heading in ["Since yesterday", "Working on now", "Needs you"] {
                assert!(!text.contains(heading));
            }
        }
    }
}

fn landed_review(at: &str) -> Value {
    json!({
        "id":"review-1", "repo":"/fixture/repo", "integration":"main", "base":"base",
        "members":[{"thread":"t-0001","attempt":1,"event":"seal","sha":"candidate","branch":"work","artifact":"report"}],
        "gates":[], "selected_gates":[], "phase":"complete", "verdict_event":"seal",
        "reviewer_after":"", "checked_event":"seal", "retry_generation":0,"moved":0,
        "install_required":true,"fast_forward":true,"push":true,"install":true,
        "merged_at":at,"installed_at":at,"install_result":"",
        "close":true,"prune":true,"attention":""
    })
}

#[test]
fn review_install_dates_and_after_install_results_are_read_only_facts() {
    let home = tempfile::tempdir().unwrap();
    ade(home.path(), &["new", "adeherdr"]);
    let at = "2026-10-04T05:08:00Z";
    let mut review = landed_review(at);
    let path = home
        .path()
        .join("root/adeherdr/.state/reviews/review-1.toml");
    record(
        home.path(),
        "adeherdr",
        "threads/t-0001.toml",
        json!({"id":"t-0001","status":"resolved","attempt":1,"merged_sha":"candidate","merged_review":"review-1"}),
    );
    record(
        home.path(),
        "adeherdr",
        "plan.toml",
        json!({"schema":1,"revision":1,"steps":[{"id":"s-1","text":"Make the screen readable","threads":["t-0001"]}]}),
    );
    for (notice, expected) in [
        ("", "not run yet"),
        (
            "REVIEW adeherdr/review-1: ticker first full pass: PASS; JOURNEY scratch-id: PASS launch (1.0s): okay; PASS Rundown renders (0.1s): okay",
            "self-check passed",
        ),
        (
            "REVIEW adeherdr/review-1: ticker first full pass: PASS; JOURNEY scratch-id: FAIL Rundown renders (0.1s): private-hash",
            "self-check failed: after-install checks did not pass",
        ),
        (
            "REVIEW adeherdr/review-1: ticker first full pass: FAIL; JOURNEY scratch-id: PASS Rundown renders (0.1s): okay",
            "self-check failed: after-install checks did not pass",
        ),
        (
            "REVIEW adeherdr/review-1: FAIL; JOURNEY old: FAIL launch; REVIEW adeherdr/review-1: ticker first full pass: PASS; JOURNEY new: PASS Rundown renders (0.1s): okay",
            "self-check passed",
        ),
    ] {
        review["install_result"] =
            json!(format!("installed on mac; Rundown renders: OK; {notice}"));
        record(
            home.path(),
            "adeherdr",
            "reviews/review-1.toml",
            review.clone(),
        );
        let bytes = std::fs::read(&path).unwrap();
        let reply = ade(home.path(), &["overview", "adeherdr"]);
        let result = &reply["data"]["result"];
        assert_eq!(result["harness"]["check"], expected);
        assert_eq!(result["harness"]["updated_at"], at);
        assert_eq!(
            result["activity"]["recent"],
            json!([{"text":"Make the screen readable","at":at}])
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    review["install"] = json!(false);
    record(
        home.path(),
        "adeherdr",
        "reviews/review-1.toml",
        review.clone(),
    );
    let pending = ade(home.path(), &["overview", "adeherdr"]);
    assert_eq!(pending["data"]["result"]["activity"]["recent"], json!([]));
    assert_eq!(
        pending["data"]["result"]["plan"]["steps"][0]["state"],
        "running"
    );
    // Old reviews retain their done count, but an absent date is never now.
    review["install"] = json!(true);
    review.as_object_mut().unwrap().remove("installed_at");
    review.as_object_mut().unwrap().remove("merged_at");
    record(home.path(), "adeherdr", "reviews/review-1.toml", review);
    let old = ade(home.path(), &["overview", "adeherdr"]);
    assert_eq!(old["data"]["result"]["activity"]["recent"], json!([]));
    assert_eq!(old["data"]["result"]["plan"]["steps"][0]["state"], "done");
}

fn git(repo: &Path, args: &[&str], at: &str) -> String {
    let output = Command::new("git")
        .env_clear()
        .env("HOME", repo.parent().unwrap())
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", at)
        .args([
            "-C",
            repo.to_str().unwrap(),
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn undated_mac_shaped_reviews_use_the_integration_reflog_without_writing_records() {
    let home = tempfile::tempdir().unwrap();
    let repo = home.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let old = "2020-01-01T00:00:00Z";
    git(&repo, &["init", "--initial-branch=main"], old);
    git(&repo, &["config", "core.logAllRefUpdates", "true"], old);
    git(&repo, &["commit", "--allow-empty", "-m", "base"], old);
    git(&repo, &["checkout", "-b", "candidate"], old);
    git(
        &repo,
        &["commit", "--allow-empty", "-m", "reviewed work"],
        old,
    );
    let candidate = git(&repo, &["rev-parse", "HEAD"], old);
    git(&repo, &["checkout", "main"], old);
    let merged = jiff::Timestamp::from_second(jiff::Timestamp::now().as_second() - 3600).unwrap();
    git(
        &repo,
        &["merge", "--ff-only", "candidate"],
        &merged.to_string(),
    );
    assert_eq!(
        git(&repo, &["show", "-s", "--format=%cI", &candidate], old)
            .parse::<jiff::Timestamp>()
            .unwrap(),
        old.parse::<jiff::Timestamp>().unwrap()
    );
    ade(home.path(), &["new", "adeherdr"]);
    let mut review = landed_review("");
    review.as_object_mut().unwrap().remove("merged_at");
    review.as_object_mut().unwrap().remove("installed_at");
    review["repo"] = json!(repo);
    review["verdict"] = json!({"verdict":"MERGE","review":"review-1","candidate":candidate});
    record(
        home.path(),
        "adeherdr",
        "reviews/review-1.toml",
        review.clone(),
    );
    record(
        home.path(),
        "adeherdr",
        "threads/t-0001.toml",
        json!({"id":"t-0001","status":"resolved","attempt":1}),
    );
    record(
        home.path(),
        "adeherdr",
        "tasks/job-0001.toml",
        json!({"id":"job-0001","title":"Make yesterday’s work visible","authority":["request:fixture"],"acceptance":["Rolf can see what landed"],"attempts":["t-0001"],"installed":[]}),
    );
    record(
        home.path(),
        "adeherdr",
        "plan.toml",
        json!({"schema":1,"revision":1,"steps":[{"id":"s-1","text":"Make yesterday’s work visible","tasks":["job-0001"]}]}),
    );
    let state = home.path().join("root/adeherdr/.state");
    let facts: Vec<_> = [
        "reviews/review-1.toml",
        "tasks/job-0001.toml",
        "threads/t-0001.toml",
        "plan.toml",
    ]
    .into_iter()
    .map(|file| (state.join(file), std::fs::read(state.join(file)).unwrap()))
    .collect();
    let result = ade(home.path(), &["overview", "adeherdr"])["data"]["result"].clone();
    assert_eq!(
        result["activity"]["recent"],
        json!([{"text":"Make yesterday’s work visible","at":merged.to_string()}])
    );
    assert_eq!(result["harness"]["updated_at"], "");
    assert_eq!(result["plan"]["steps"][0]["state"], "done");
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-rundown"))
        .env_clear()
        .env("HOME", home.path())
        .env("HERDR_ADE_ROOT", home.path().join("root"))
        .arg("--print")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Since yesterday"), "{text}");
    assert!(text.contains("Make yesterday’s work visible"));
    assert!(text.contains("1 of 1"));
    assert!(
        text.contains(
            &merged
                .to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        )
    );
    assert!(text.contains("Harness update time unknown"));
    for (path, before) in facts {
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    // The stored merge date takes precedence; a later install is not the date
    // of the work. This does not change any plan marks.
    review["merged_at"] = json!(old);
    review["installed_at"] = json!(merged.to_string());
    record(
        home.path(),
        "adeherdr",
        "reviews/review-1.toml",
        review.clone(),
    );
    assert_eq!(
        ade(home.path(), &["overview", "adeherdr"])["data"]["result"]["activity"]["recent"][0]["at"],
        old
    );
    review.as_object_mut().unwrap().remove("merged_at");
    review.as_object_mut().unwrap().remove("installed_at");

    // No exact new-value match: never substitute a member SHA, HEAD, commit
    // date, or an entry on another branch.
    review["verdict"]["candidate"] = json!("unmatched");
    review["members"][0]["sha"] = json!(candidate);
    record(
        home.path(),
        "adeherdr",
        "reviews/review-1.toml",
        review.clone(),
    );
    assert_eq!(
        ade(home.path(), &["overview", "adeherdr"])["data"]["result"]["activity"]["recent"],
        json!([])
    );

    // Historical lane landing markers also identify the review's merged tip.
    review.as_object_mut().unwrap().remove("verdict");
    record(
        home.path(),
        "adeherdr",
        "reviews/review-1.toml",
        review.clone(),
    );
    record(
        home.path(),
        "adeherdr",
        "threads/t-0001.toml",
        json!({"id":"t-0001","status":"resolved","attempt":1,"merged_sha":candidate,"merged_review":"review-1"}),
    );
    assert_eq!(
        ade(home.path(), &["overview", "adeherdr"])["data"]["result"]["activity"]["recent"][0]["at"],
        merged.to_string()
    );
    review["integration"] = json!("absent-branch");
    record(
        home.path(),
        "adeherdr",
        "reviews/review-1.toml",
        review.clone(),
    );
    assert_eq!(
        ade(home.path(), &["overview", "adeherdr"])["data"]["result"]["activity"]["recent"],
        json!([])
    );
    review["integration"] = json!("main");
    record(home.path(), "adeherdr", "reviews/review-1.toml", review);
    git(
        &repo,
        &["reflog", "expire", "--expire=all", "refs/heads/main"],
        old,
    );
    let expired = ade(home.path(), &["overview", "adeherdr"])["data"]["result"].clone();
    assert_eq!(expired["activity"]["recent"], json!([]));
    assert_eq!(expired["plan"]["steps"][0]["state"], "done");
}

#[test]
fn print_is_terminal_free_and_a_failed_project_does_not_hide_others() {
    let home = tempfile::tempdir().unwrap();
    for project in ["alpha", "broken", "charlie"] {
        ade(home.path(), &["new", project]);
    }
    write(
        home.path().join("root/broken/.state/plan.toml"),
        "broken = [",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-rundown"))
        .env_clear()
        .env("HOME", home.path())
        .env("HERDR_ADE_ROOT", home.path().join("root"))
        .arg("--print")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Alpha") && text.contains("Charlie"), "{text}");
    assert!(text.contains("Overview unavailable; retrying"));
    assert_eq!(text.matches("Harness update time unknown").count(), 1);
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn harness_line_uses_local_time_and_never_invents_a_pass() {
    let now = jiff::Timestamp::now();
    let card = |harness: Value| {
        view::Card::from_view("Demo", &json!({"plan":{"schema":1,"revision":1,"steps":[]},"work":"", "needs_you":"", "actions":[], "harness":harness})).unwrap()
    };
    let unknown = card(json!({}));
    assert_eq!(
        view::harness_line(&unknown.harness, 100, now),
        "Harness update time unknown · not run yet"
    );
    for result in [
        "self-check passed",
        "self-check failed: could not open the rundown",
        "not run yet",
    ] {
        let card = card(json!({"updated_at":now.to_string(), "check":result}));
        let line = view::harness_line(&card.harness, 120, now);
        assert!(line.contains(result));
        assert!(line.contains("less than a min ago"));
    }
}
