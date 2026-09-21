//! Crate-side seam for the pi library: the parts that need `crate::`.
//!
//! `src/pi/` itself never names `crate::` because it also compiles into the
//! thin `herdr-pi` binary. This file is compiled only into `herdr-ade`.
//!
//! `thread start` and the ticker call [`check_with`] before a `kind = "pi"`
//! launch; `doctor` prints [`doctor_rows_with`]. Both take the ADE root.
//! There is no per-tick pi work (SPEC-pi v2).

use std::fmt;
use std::path::Path;

use anyhow::Result;

use crate::pi::doctor::{self, CheckReport};
use crate::pi::sh;
use crate::pi::{Env, Layout};

/// `crate::runner::Runner` as a `pi::sh::Runner`.
pub(crate) struct Adapter<'a>(pub &'a dyn crate::runner::Runner);

impl sh::Runner for Adapter<'_> {
    fn run(&self, cmd: &sh::Cmd) -> Result<sh::Output> {
        // Own group, like `sh::RealRunner`: a timeout on `zsh -lic` must
        // reach its children.
        let mut adapted = crate::runner::Cmd::new(cmd.program.clone(), cmd.timeout).own_group();
        adapted = adapted.args(cmd.args.clone());
        for (key, value) in &cmd.env {
            adapted = adapted.env(key.clone(), value.clone());
        }
        for key in &cmd.env_remove {
            adapted = adapted.env_remove(key.clone());
        }
        if let Some(cwd) = &cmd.cwd {
            adapted = adapted.cwd(cwd.clone());
        }
        if let Some(stdin) = &cmd.stdin {
            adapted = adapted.stdin(stdin.clone());
        }
        let output = self.0.run(&adapted)?;
        Ok(sh::Output {
            code: output.code,
            stdout: output.stdout,
            stderr: output.stderr,
            timed_out: output.timed_out,
        })
    }
}

/// The pi folder under this ADE root (SPEC-ADE item 90): `herdr-ade --root`
/// and the pi library agree on one place.
fn layout(root: &Path) -> Layout {
    Layout {
        root: root.join("pi"),
    }
}

#[derive(Debug)]
pub(crate) struct ReadinessError {
    pub(crate) class: crate::contracts::FailureClass,
    message: String,
}

impl fmt::Display for ReadinessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for ReadinessError {}

pub(crate) fn failure_class(error: &anyhow::Error) -> crate::contracts::FailureClass {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ReadinessError>())
        .map_or(crate::contracts::FailureClass::Unknown, |error| error.class)
}

fn core_class(evidence: doctor::FailureEvidence) -> crate::contracts::FailureClass {
    match evidence {
        doctor::FailureEvidence::Provider => crate::contracts::FailureClass::Provider,
        doctor::FailureEvidence::Unknown => crate::contracts::FailureClass::Unknown,
    }
}

/// Readiness for one provider before a `kind = "pi"` launch (SPEC-pi §3.4):
/// every failing row in one refusal. Runs through the plugin's runner from the
/// stable projects root.
pub(crate) fn check_with(
    runner: &dyn crate::runner::Runner,
    root: &Path,
    provider: &str,
) -> Result<CheckReport> {
    let env = Env::from_process()?;
    let rooted = crate::runner::CwdRunner::new(runner, root);
    let report = doctor::check_report(&env, &layout(root), &Adapter(&rooted), provider);
    if report.ok {
        Ok(report)
    } else {
        Err(ReadinessError {
            class: core_class(report.failure_evidence()),
            message: report.error_text(),
        }
        .into())
    }
}

/// Box pi readiness (SPEC-remote §4.1, SPEC-pi §3.4, item 101): the check runs
/// on the box through its own wrapper and login store. A Mac login never
/// counts, and the model is resolved from the box's shared store.
pub(crate) fn check_on_machine(
    runner: &dyn crate::runner::Runner,
    local_root: &Path,
    target: &str,
    provider: &str,
) -> Result<()> {
    let script = format!(
        "HERDR_ADE_ROOT={root} {bin} check {provider}",
        root = crate::remote::quote(crate::contracts::BOX_ROOT),
        bin = crate::remote::quote(crate::contracts::BOX_PI_BIN),
        provider = crate::remote::quote(provider),
    );
    let rooted = crate::runner::CwdRunner::new(runner, local_root);
    let out = crate::remote::ssh(
        &rooted,
        target,
        &script,
        None,
        crate::remote::SSH_START_TIMEOUT,
    )?;
    if out.success() {
        return Ok(());
    }
    let parsed = serde_json::from_str::<serde_json::Value>(&out.stdout).ok();
    let evidence = parsed
        .as_ref()
        .and_then(|value| value.get("checks"))
        .and_then(serde_json::Value::as_array)
        .filter(|checks| {
            checks
                .iter()
                .any(|check| check.get("ok").and_then(serde_json::Value::as_bool) == Some(false))
        })
        .is_some_and(|checks| {
            checks
                .iter()
                .filter(|check| check.get("ok").and_then(serde_json::Value::as_bool) == Some(false))
                .all(|check| {
                    check
                        .get("failure_class")
                        .and_then(serde_json::Value::as_str)
                        == Some("provider")
                })
        });
    Err(ReadinessError {
        class: if evidence {
            crate::contracts::FailureClass::Provider
        } else {
            crate::contracts::FailureClass::Unknown
        },
        message: format!(
            "pi_not_ready on the box for `{provider}`: {}",
            out.error_text()
        ),
    }
    .into())
}

/// The pi doctor rows through the plugin's runner, for `doctor`.
pub(crate) fn doctor_rows_with(
    runner: &dyn crate::runner::Runner,
    root: &Path,
) -> Result<(Vec<doctor::Row>, bool)> {
    let env = Env::from_process()?;
    let providers = crate::pi::recipes::enabled_providers();
    let rooted = crate::runner::CwdRunner::new(runner, root);
    let rows = doctor::doctor_rows_with(&env, &layout(root), &Adapter(&rooted), &providers);
    let ok = doctor::healthy(&rows);
    Ok((rows, ok))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi::sh::Runner as _;
    use crate::runner::fake::{FakeRunner, fail, ok};

    /// Scripted through the plugin's own FakeRunner: the adapter carries the
    /// argv and env across unchanged.
    #[test]
    fn the_adapter_passes_argv_and_env_to_the_plugin_runner() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pi"));
        let env = Env::for_test(dir.path(), &[]);
        let runner = FakeRunner::new();
        runner.on("zsh -lic node --version", ok("v22.19.0\n"));
        let adapter = Adapter(&runner);
        let cmd = sh::Cmd::new("zsh", sh::SHORT)
            .args(["-lic", "node --version"])
            .env("PI_CODING_AGENT_DIR", layout.agent().display().to_string());
        let output = adapter.run(&cmd).unwrap();
        assert_eq!(output.stdout, "v22.19.0\n");
        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].display(), "zsh -lic node --version");
        assert!(calls[0].env.iter().any(|(k, _)| k == "PI_CODING_AGENT_DIR"));
        assert!(calls[0].own_group, "a timeout must reach zsh's children");
        drop(calls);
        let _ = (&env, &layout);
    }

    /// The readiness check runs on the box through `herdr-pi`, never the plugin
    /// binary, and names the provider.
    #[test]
    fn the_box_readiness_check_calls_the_pi_binary_with_the_provider() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new();
        runner.on("ssh", ok("{}"));
        check_on_machine(&runner, dir.path(), "me@box", "opencode-go").unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].cwd.as_deref(), Some(dir.path()));
        assert_eq!(
            calls[0].args.last().unwrap(),
            "sh -c 'PATH=/home/ubuntu/.local/bin:/home/ubuntu/.cargo/bin:/usr/local/bin:/usr/bin:/bin; export PATH\nHERDR_ADE_ROOT=/home/ubuntu/.herdr-ade /home/ubuntu/.local/bin/herdr-pi check opencode-go'"
        );
        drop(calls);
    }

    /// A non-zero exit surfaces the box's own stderr, not a Mac-side guess.
    #[test]
    fn a_box_readiness_refusal_surfaces_the_box_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new();
        runner.on("ssh", fail(1, "error: unrecognized subcommand 'check'\n"));
        let error = check_on_machine(&runner, dir.path(), "me@box", "opencode-go")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("pi_not_ready on the box for `opencode-go`"),
            "{error}"
        );
        assert!(
            error.contains("error: unrecognized subcommand 'check'"),
            "{error}"
        );
    }
}
