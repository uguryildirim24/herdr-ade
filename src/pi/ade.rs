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
    model: &str,
) -> Result<CheckReport> {
    let env = Env::from_process()?;
    let rooted = crate::runner::CwdRunner::new(runner, root);
    let report = doctor::check_report_model(
        &env,
        &layout(root),
        &Adapter(&rooted),
        provider,
        Some(model),
    );
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
    machine: &crate::remote::MachineDeclaration,
    provider: &str,
    model: &str,
) -> Result<()> {
    let script = crate::remote::with_path(
        &machine.path,
        &format!(
            "HERDR_ADE_ROOT={root} {bin} check {provider} --model {model}",
            root = crate::remote::quote(&machine.root),
            bin = crate::remote::quote(&machine.pi_bin),
            provider = crate::remote::quote(provider),
            model = crate::remote::quote(model),
        ),
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
    let evidence = !out.timed_out
        && out.code != Some(255)
        && parsed
            .as_ref()
            .and_then(|value| value.get("checks"))
            .and_then(serde_json::Value::as_array)
            .filter(|checks| {
                checks.iter().any(|check| {
                    check.get("ok").and_then(serde_json::Value::as_bool) == Some(false)
                })
            })
            .is_some_and(|checks| {
                checks
                    .iter()
                    .filter(|check| {
                        check.get("ok").and_then(serde_json::Value::as_bool) == Some(false)
                    })
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

/// Real model probes run concurrently, each with a rooted runner and
/// independent cache key. Scripted runners keep deterministic call order.
fn doctor_rows_parallel(
    env: &Env,
    layout: &Layout,
    runner: &dyn sh::Runner,
    models: &[(&str, &str)],
    root: &Path,
) -> Vec<doctor::Row> {
    if !layout.wrapper().is_file() {
        return doctor::doctor_rows_with_models(env, layout, runner, models);
    }
    std::thread::scope(|scope| {
        let workers: Vec<_> = models
            .iter()
            .map(|&(provider, model)| {
                scope.spawn(move || {
                    let real = crate::runner::RealRunner;
                    let rooted = crate::runner::CwdRunner::new(&real, root);
                    doctor::provider_row(&Adapter(&rooted), layout, provider, model)
                })
            })
            .collect();
        let mut rows = doctor::doctor_rows_with(env, layout, runner, &[]);
        rows.extend(
            workers
                .into_iter()
                .map(|worker| worker.join().expect("provider probe panicked")),
        );
        rows
    })
}

/// The pi doctor rows through the plugin's runner, for `doctor`.
pub(crate) fn doctor_rows_with(
    runner: &dyn crate::runner::Runner,
    root: &Path,
    models: &[(String, String)],
) -> Result<(Vec<doctor::Row>, bool)> {
    let env = Env::from_process()?;
    let rooted = crate::runner::CwdRunner::new(runner, root);
    let models: Vec<(&str, &str)> = models
        .iter()
        .map(|(provider, model)| (provider.as_str(), model.as_str()))
        .collect();
    let rows = if runner.is_real() {
        doctor_rows_parallel(&env, &layout(root), &Adapter(&rooted), &models, root)
    } else {
        doctor::doctor_rows_with_models(&env, &layout(root), &Adapter(&rooted), &models)
    };
    let ok = doctor::healthy(&rows);
    Ok((rows, ok))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, fail, ok};

    fn box_machine() -> crate::remote::MachineDeclaration {
        crate::remote::MachineDeclaration {
            root: "/home/agent/.herdr-ade".into(),
            path: "/home/agent/.local/bin:/usr/bin:/bin".into(),
            pi_bin: "/home/agent/.local/bin/herdr-pi".into(),
            ..Default::default()
        }
    }

    /// The readiness check runs on the box through `herdr-pi`, never the plugin
    /// binary, and names the provider.
    #[test]
    fn the_box_readiness_check_calls_the_pi_binary_with_the_provider() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new();
        runner.on("ssh", ok("{}"));
        check_on_machine(
            &runner,
            dir.path(),
            "me@box",
            &box_machine(),
            "opencode-go",
            "deepseek-v4.1-flash",
        )
        .unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].cwd.as_deref(), Some(dir.path()));
        assert_eq!(
            calls[0].args.last().unwrap(),
            "sh -c 'PATH=/home/agent/.local/bin:/usr/bin:/bin; export PATH\nHERDR_ADE_ROOT=/home/agent/.herdr-ade /home/agent/.local/bin/herdr-pi check opencode-go --model deepseek-v4.1-flash'"
        );
        drop(calls);
    }

    /// A non-zero exit surfaces the box's own stderr, not a Mac-side guess.
    #[test]
    fn a_box_readiness_refusal_surfaces_the_box_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new();
        runner.on("ssh", fail(1, "error: unrecognized subcommand 'check'\n"));
        let error = check_on_machine(
            &runner,
            dir.path(),
            "me@box",
            &box_machine(),
            "opencode-go",
            "deepseek-v4.1-flash",
        )
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

    #[test]
    fn a_timed_out_box_check_stays_unknown_despite_partial_provider_output() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            crate::runner::Output {
                code: Some(1),
                stdout: r#"{"checks":[{"ok":false,"failure_class":"provider"}]}"#.into(),
                timed_out: true,
                ..Default::default()
            },
        );

        let error = check_on_machine(
            &runner,
            dir.path(),
            "me@box",
            &box_machine(),
            "opencode-go",
            "deepseek-v4.1-flash",
        )
        .unwrap_err();
        assert_eq!(
            failure_class(&error),
            crate::contracts::FailureClass::Unknown
        );
    }
}
