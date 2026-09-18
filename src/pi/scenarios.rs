//! FakeRunner scenarios for a pi lane: start, restart and the guard
//! (SPEC-pi v2 §7; SPEC-ADE §1.3).
//!
//! Every command is scripted through `pi::sh::fake`; no herdr, no pi process,
//! no network. The guard itself is TypeScript, so its classification twin in
//! `limits` is asserted here and the shipping file's contract is asserted
//! verbatim; `src/pi/testdata/guard-check.sh` runs the real extension against
//! the isolated pi with a mock provider that answers 429 and 401.

use std::path::Path;

use super::sh::fake::{FakeRunner, ok};
use super::{Env, Layout, doctor, folder, install, launch, limits, roles};

/// A throwaway plugin root with the pinned files in place and the wrapper on
/// the login PATH (a real symlink under the fixture HOME).
fn world(dir: &Path) -> (Env, Layout) {
    let layout = Layout::for_test(dir.join("state/pi"));
    folder::ensure(&layout).unwrap();
    install::write_guard(&layout).unwrap();
    launch::write_wrapper(&layout).unwrap();
    std::fs::create_dir_all(layout.package().join("dist/bundle")).unwrap();
    std::fs::write(layout.package_json(), r#"{"version":"0.85.1"}"#).unwrap();
    std::fs::write(layout.cli_js(), "// cli").unwrap();
    std::fs::write(
        layout.npm().join("package.json"),
        r#"{"dependencies":{"@earendil-works/pi-coding-agent":"0.85.1"}}"#,
    )
    .unwrap();
    let env = Env::for_test(dir, &[("HERDR_BIN_PATH", "/h/herdr")]);
    std::fs::create_dir_all(env.home.join(".local/bin")).unwrap();
    std::os::unix::fs::symlink(layout.wrapper(), env.home.join(".local/bin/pi")).unwrap();
    (env, layout)
}

#[test]
fn scenario_start_is_the_spec_line_and_never_a_trust_flag() {
    let dir = tempfile::tempdir().unwrap();
    let (_env, _layout) = world(dir.path());
    let row = roles::pi_recipes()
        .into_iter()
        .find(|r| r.id == "pi_deepseek_flash")
        .unwrap();

    let start =
        launch::agent_start_args("a5", "w1F:p13", "w1F:p1", 30_000, &row.args, None).unwrap();
    let line = start.join(" ");
    assert_eq!(
        line,
        "agent start a5 --kind pi --pane w1F:p13 --parent w1F:p1 --timeout 30000 -- \
         --provider deepseek --model deepseek-v4-flash --thinking low --no-skills"
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    );
    for forbidden in ["--approve", "-na", "--no-approve", "--session"] {
        assert!(!start.iter().any(|a| a == forbidden), "{forbidden}");
    }
    assert!(row.env.is_empty());
    // A trust dialog never appears because trust is settled by settings.
    assert!(folder::SETTINGS_JSON.contains("\"defaultProjectTrust\": \"never\""));
}

#[test]
fn scenario_restart_uses_the_reported_session_and_the_recipe_stays_clean() {
    let dir = tempfile::tempdir().unwrap();
    let (_env, _layout) = world(dir.path());
    let row = roles::pi_recipes()
        .into_iter()
        .find(|r| r.id == "pi_kimi_k3")
        .unwrap();
    let pane_get = r#"{"result":{"pane":{"agent_session":{"source":"herdr:pi","agent":"pi","kind":"path","value":"/state/pi/agent/sessions/--/lane.jsonl"}}}}"#;
    let session = super::resume::session_from_pane_get(pane_get)
        .unwrap()
        .expect("the extension reported a session");
    let session = Path::new(&session);

    let restart =
        launch::agent_start_args("a5", "w1F:p13", "w1F:p1", 30_000, &row.args, Some(session))
            .unwrap();
    assert_eq!(
        restart.iter().filter(|a| *a == "--session").count(),
        1,
        "exactly one --session"
    );
    assert!(
        restart
            .iter()
            .any(|a| a == "/state/pi/agent/sessions/--/lane.jsonl")
    );
    assert!(!restart.iter().any(|a| a == "-c" || a == "--continue"));
    assert!(!restart.iter().any(|a| a == "--approve"));
    // The recipe itself is unchanged and still valid.
    row.validate().unwrap();
    assert_eq!(
        super::resume::restore_line(session),
        "pi --session /state/pi/agent/sessions/--/lane.jsonl"
    );
}

#[test]
fn scenario_the_guard_reports_a_429_as_waiting_once_and_never_done() {
    let mut throttle = limits::Throttle::new(std::time::Duration::from_secs(600));
    let now = std::time::Instant::now();
    let message = "429 Rate limit reached for gpt-6-astra";
    let class = limits::classify(message, Some(429));
    assert_eq!(class, limits::LimitClass::Limit);
    assert!(throttle.due(class, now));
    assert!(!throttle.due(class, now + std::time::Duration::from_secs(300)));
    assert!(throttle.due(class, now + std::time::Duration::from_secs(601)));

    let line = limits::waiting_line("a5", "openai-codex", class, message);
    assert_eq!(
        line,
        "WAITING a5 openai-codex limit: 429 Rate limit reached for gpt-6-astra"
    );

    // The shipping extension: reports blocked and `ha waiting`; never done.
    let guard = install::GUARD_TS;
    assert!(guard.contains(super::GUARD_MARKER));
    assert!(guard.contains("herdr:blocked"));
    assert!(guard.contains("ha\", [\"waiting\""));
    assert!(guard.contains("HERDR_ADE_LAUNCH"));
    assert!(!guard.contains("\"done\""));
    assert!(!guard.contains("[\"done\""));
    assert!(!guard.contains("agent prompt\"")); // the fallback uses ["agent","prompt",...]
}

#[test]
fn scenario_the_guard_reports_a_401_as_login_and_a_dead_endpoint_as_unreachable() {
    assert_eq!(
        limits::classify("Incorrect API key provided", Some(401)),
        limits::LimitClass::Login
    );
    assert_eq!(
        limits::classify("fetch failed", None),
        limits::LimitClass::Unreachable
    );
    let label = limits::blocked_label("deepseek", limits::LimitClass::Login, Some(401), "bad key");
    assert_eq!(label, "deepseek login HTTP 401: bad key");
}

#[test]
fn scenario_setup_then_check_for_deepseek() {
    let dir = tempfile::tempdir().unwrap();
    let (env, layout) = world(dir.path());
    let runner = FakeRunner::new();
    runner.on("zsh -lic node --version", ok("v22.19.0\n"));
    runner.on("zsh -lic command -v npm", ok("/opt/homebrew/bin/npm\n"));
    runner.on(
        "zsh -lic npm root -g",
        ok("/opt/homebrew/lib/node_modules\n"),
    );
    runner.on(
        "zsh -lic whence -va pi",
        ok(&format!(
            "pi is {}\n",
            env.home.join(".local/bin/pi").display()
        )),
    );
    runner.on("herdr integration status", ok("pi: current\n"));
    runner.on("--version", ok("0.85.1\n"));
    runner.on(
        "auth check --provider deepseek",
        ok(r#"{"status":"ready"}"#),
    );

    let report = doctor::check_report(&env, &layout, &runner, "deepseek");
    assert!(report.ok, "{}", report.error_text());
    assert_eq!(
        runner
            .calls
            .borrow()
            .iter()
            .filter(|c| c.display().contains("auth check"))
            .count(),
        1
    );
}

#[test]
fn scenario_check_refuses_a_missing_login_before_any_start() {
    let dir = tempfile::tempdir().unwrap();
    let (env, layout) = world(dir.path());
    let runner = FakeRunner::new();
    runner.on(
        "zsh -lic whence -va pi",
        ok(&format!(
            "pi is {}\n",
            env.home.join(".local/bin/pi").display()
        )),
    );
    runner.on("herdr integration status", ok("pi: current\n"));
    runner.on(
        "auth check --provider deepseek",
        ok(r#"{"status":"not_ready","reason":"credentials_not_configured"}"#),
    );
    let report = doctor::check_report(&env, &layout, &runner, "deepseek");
    assert!(!report.ok);
    assert!(report.error_text().contains("credentials_not_configured"));
    // The refusal is the check path A1 calls; a start never happens.
    assert_eq!(runner.count("agent start"), 0);
}
