//! Fresh-project startup against a CLI fake enforcing the fork's timeout limits.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_herdr-ade");

#[test]
fn fresh_open_starts_and_primes_or_reports_a_hard_failure_with_a_retry() {
    for mode in [
        "ready",
        "timeout",
        "agent_not_ready",
        "start-error",
        "missing-command",
    ] {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let config = home.path().join(".config/herdr-ade");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            format!("[routing]\ndefault = 'fixture'\n[recipes.fixture]\nkind = 'claude'\nargs = ['--dangerously-skip-permissions']\nready_timeout_ms = {}\n", if matches!(mode, "timeout" | "agent_not_ready") { 1 } else { 300000 }),
        )
        .unwrap();
        // Supply an already-running ticker receipt; this test starts no daemon.
        let version = Command::new(BIN).arg("--version").output().unwrap();
        let version = String::from_utf8(version.stdout).unwrap();
        let build = version.trim().strip_prefix("herdr-ade ").unwrap();
        let mut ticker = std::fs::File::create(root.join(".ticker.lock")).unwrap();
        ticker.lock().unwrap();
        write!(
            ticker,
            "{}",
            serde_json::json!({"pid": std::process::id(), "version": build})
        )
        .unwrap();
        ticker.flush().unwrap();

        let herdr = home.path().join("herdr");
        std::fs::write(&herdr, r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/herdr-calls"
case "$1 $2" in
  'agent list') printf '%s\n' '{"result":{"agents":[]}}' ;;
  'pane list') printf '%s\n' '{"result":{"panes":[]}}' ;;
  'workspace create') printf '%s\n' '{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}}' ;;
  'tab list') printf '%s\n' '{"result":{"tabs":[]}}' ;;
  'plugin pane') printf '%s\n' '{"result":{"plugin_pane":{"pane":{"tab_id":"w1:t2"}}}}' ;;
  'agent start')
    if [ "$3" = --help ]; then printf '%s\n' '[possible values: pi, claude, cursor, agy] --parent'; exit 0; fi
    while [ "$#" -gt 0 ]; do
      if [ "$1" = '--timeout' ]; then timeout="$2"; break; fi
      shift
    done
    if [ "$timeout" -le 3000 ] || [ "$timeout" -gt 300000 ]; then
      printf '%s\n' '{"error":{"code":"invalid_agent_timeout","message":"agent start timeout must be greater than 3000ms and at most 300000ms"}}'
      exit 1
    fi
    case "$START_MODE" in
      timeout|agent_not_ready) /bin/sleep 0.01; printf '{"error":{"code":"%s","message":"not ready"}}\n' "$START_MODE"; exit 1 ;;
      start-error) printf '%s\n' '{"error":{"code":"pane_busy","message":"pane is not at an interactive shell"}}'; exit 1 ;;
      missing-command) printf '%s\n' '{"error":{"code":"timeout","message":"not ready"}}'; exit 1 ;;
    esac
    printf '{"result":{"agent":{"name":"hp-demo-coordinator","agent":"claude","pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","agent_status":"idle","cwd":"%s"}}}\n' "$HOME/root/demo" ;;
  'pane read')
    if [ "$START_MODE" = missing-command ]; then printf 'bash: claude: command not found\n$ '; else printf '❯ \n'; fi ;;
  *) printf '%s\n' '{"result":{}}' ;;
esac
"#).unwrap();
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        let run = |args: &[&str]| {
            Command::new(BIN)
                .env_clear()
                .env("HOME", home.path())
                .env("PATH", "/usr/bin:/bin")
                .env("HERDR_BIN_PATH", &herdr)
                .env("HERDR_SOCKET_PATH", home.path().join("fixture.sock"))
                .env("START_MODE", mode)
                .args(["--root", root.to_str().unwrap()])
                .args(args)
                .output()
                .unwrap()
        };
        let new = run(&["new", "demo"]);
        assert!(
            new.status.success(),
            "{}",
            String::from_utf8_lossy(&new.stderr)
        );
        let opened = run(&["open", "demo"]);
        let stdout = String::from_utf8_lossy(&opened.stdout);
        let stderr = String::from_utf8_lossy(&opened.stderr);
        let calls = std::fs::read_to_string(home.path().join("herdr-calls")).unwrap();
        assert!(
            calls.contains(
                "agent start hp-demo-coordinator --kind claude --pane w1:p1 --timeout 3001"
            ),
            "{calls}\nstdout: {stdout}\nstderr: {stderr}"
        );
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.starts_with("agent start ") && !line.contains("--help"))
                .count(),
            1
        );
        assert!(!stdout.contains("priming prompt is pending"), "{stdout}");
        if mode == "ready" {
            assert!(opened.status.success(), "{stdout}\n{stderr}");
            assert!(
                stdout.contains("opened `demo`") && stdout.contains("added the Rundown tab"),
                "{stdout}"
            );
            assert!(
                calls.contains("agent prompt w1:p1 You are the coordinator"),
                "{calls}"
            );
            assert!(root.join("demo/.claude/settings.local.json").exists());
        } else if matches!(mode, "timeout" | "agent_not_ready") {
            assert!(opened.status.success(), "{stdout}\n{stderr}");
            assert!(
                stdout.contains("not ready yet")
                    && stdout.contains("ticker sends the priming prompt"),
                "{stdout}"
            );
            assert!(
                !calls.contains("agent prompt") && !calls.contains("agent wait"),
                "{calls}"
            );
        } else {
            assert!(!opened.status.success(), "{stdout}\n{stderr}");
            assert!(
                stderr.contains("the coordinator did not start")
                    && stderr.contains("retry with `")
                    && stderr.contains(" open demo`"),
                "{stderr}"
            );
            assert!(
                stderr.contains(if mode == "start-error" {
                    "pane_busy"
                } else {
                    "claude: command not found"
                }),
                "{stderr}"
            );
            assert!(
                !calls.contains("agent prompt") && !calls.contains("agent wait"),
                "{calls}"
            );
            assert!(
                !stdout.contains("ticker sends the priming prompt"),
                "{stdout}"
            );
        }
    }
}
