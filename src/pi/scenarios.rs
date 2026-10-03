//! FakeRunner scenarios for pi setup and readiness
//! (SPEC-pi v2 §7; SPEC-ADE §1.3).
//!
//! Every command is scripted through `pi::sh::fake`; no herdr, no pi process,
//! no network.

use std::path::Path;

use super::sh::fake::{FakeRunner, ok};
use super::{Env, Layout, doctor, folder, install, launch, sh};

/// A throwaway plugin root with the pinned files in place and the wrapper on
/// the login PATH (a real symlink under the fixture HOME).
fn world(dir: &Path) -> (Env, Layout) {
    let layout = Layout::for_test(dir.join("state/pi"));
    folder::ensure(&layout).unwrap();
    install::write_guard(&layout).unwrap();
    launch::write_wrapper(&layout).unwrap();
    std::fs::create_dir_all(layout.package().join("dist/bundle")).unwrap();
    std::fs::write(layout.package_json(), r#"{"version":"0.99.1"}"#).unwrap();
    std::fs::write(layout.cli_js(), "// cli").unwrap();
    std::fs::write(
        layout.npm().join("package.json"),
        r#"{"dependencies":{"@earendil-works/pi-coding-agent":"0.99.1"}}"#,
    )
    .unwrap();
    let env = Env::for_test(dir, &[("HERDR_BIN_PATH", "/h/herdr")]);
    std::fs::create_dir_all(env.home.join(".local/bin")).unwrap();
    std::os::unix::fs::symlink(layout.wrapper(), env.home.join(".local/bin/pi")).unwrap();
    (env, layout)
}

/// Script the login shell's own probes, the same shell `doctor` runs. The
/// Mac's shell is zsh and the box's is bash, so a scenario that hard-coded
/// zsh would fail on the box; the probe and the script must agree with the
/// shell actually in use.
fn script_login_shell(runner: &FakeRunner, env: &Env) {
    let shell = sh::shell();
    let probe = doctor::path_probe(&shell);
    let link = env.home.join(".local/bin/pi");
    runner.on(&format!("{shell} -lic node --version"), ok("v22.19.0\n"));
    runner.on(
        &format!("{shell} -lic command -v npm"),
        ok("/opt/homebrew/bin/npm\n"),
    );
    runner.on(
        &format!("{shell} -lic npm root -g"),
        ok("/opt/homebrew/lib/node_modules\n"),
    );
    // `type -a` and `whence -va` answer `pi is <path>`; POSIX `command -v`
    // prints the bare path.
    let resolution = if probe == "command -v pi" {
        format!("{}\n", link.display())
    } else {
        format!("pi is {}\n", link.display())
    };
    runner.on(&format!("{shell} -lic {probe}"), ok(&resolution));
}

#[test]
fn scenario_setup_then_check_for_kimi() {
    let dir = tempfile::tempdir().unwrap();
    let (env, layout) = world(dir.path());
    let runner = FakeRunner::new();
    script_login_shell(&runner, &env);
    runner.on("herdr integration status", ok("pi: current\n"));
    runner.on("--version", ok("0.99.1\n"));
    runner.on(
        "auth check --provider kimi-coding",
        ok(r#"{"status":"ready"}"#),
    );
    runner.on("--print Reply OK.", ok("OK\n"));

    let report = doctor::check_report_model(&env, &layout, &runner, "kimi-coding", None);
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
    script_login_shell(&runner, &env);
    runner.on("herdr integration status", ok("pi: current\n"));
    runner.on(
        "auth check --provider kimi-coding",
        ok(r#"{"status":"not_ready","reason":"credentials_not_configured"}"#),
    );
    let report = doctor::check_report_model(&env, &layout, &runner, "kimi-coding", None);
    assert!(!report.ok);
    assert!(report.error_text().contains("credentials_not_configured"));
    // The refusal is the check path A1 calls; a start never happens.
    assert_eq!(runner.count("agent start"), 0);
}
