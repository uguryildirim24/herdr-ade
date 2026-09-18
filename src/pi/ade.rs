//! Crate-side seam for the pi library: the parts that need `crate::`.
//!
//! `src/pi/` itself never names `crate::` because it also compiles into the
//! thin `herdr-pi` binary. This file is registered from `src/main.rs` inside
//! the `ade-pi` markers and is compiled only into the `herdr-ade` binary.
//!
//! A1 uses [`check_with`] and [`doctor_rows_with`] when it has a
//! `crate::runner::Runner` (the real one or a `FakeRunner`); plain
//! [`crate::pi::check`] uses the process environment and the real runner.
//!
//! There is no `tick` entry point: SPEC-pi v2 gives the pi library no
//! per-tick work. The guard is event-driven inside the pi process, and the
//! pre-launch check runs at launch. If the round wants a call site anyway,
//! add the function here; see the report's "Left for the reviewer".

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

use crate::pi::sh;
use crate::pi::{CheckReport, Env, Layout, doctor};

/// `crate::runner::Runner` as a `pi::sh::Runner`.
pub struct Adapter<'a>(pub &'a dyn crate::runner::Runner);

impl sh::Runner for Adapter<'_> {
    fn run(&self, cmd: &sh::Cmd) -> Result<sh::Output> {
        let mut adapted = crate::runner::Cmd::new(cmd.program.clone(), cmd.timeout);
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

/// The process layout and environment, once.
pub fn from_process() -> Result<(Env, Layout)> {
    let env = Env::from_process()?;
    let layout = Layout::from_env(&env)?;
    Ok((env, layout))
}

/// Readiness for one provider through a plugin runner (A1's pre-launch
/// refusal, scripted in tests). Equivalent to `crate::pi::check` but without
/// touching the process's command execution.
pub fn check_with(runner: &dyn crate::runner::Runner, provider: &str) -> Result<CheckReport> {
    let (env, layout) = from_process()?;
    doctor::check_with(&layout, &env, &Adapter(runner), provider)
}

/// The pi doctor rows through a plugin runner, for A1's `doctor`.
pub fn doctor_rows_with(runner: &dyn crate::runner::Runner) -> Result<(Vec<doctor::Row>, bool)> {
    let (env, layout) = from_process()?;
    let providers = crate::pi::roles::enabled_providers();
    let rows = doctor::doctor_rows_with(&env, &layout, &Adapter(runner), &providers);
    let ok = doctor::healthy(&rows);
    Ok((rows, ok))
}

/// The npm prefix a lane's pi process must live under (SPEC-pi v2 §3.4):
/// `herdr pane process-info` output is checked against this.
pub fn process_prefix() -> Result<PathBuf> {
    let (_env, layout) = from_process()?;
    Ok(layout.npm())
}

/// A timeout for the adapter's callers that do not set one.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi::sh::Runner as _;
    use crate::runner::fake::{FakeRunner, ok};

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
        let cmd = sh::Cmd::new("zsh", DEFAULT_TIMEOUT)
            .args(["-lic", "node --version"])
            .env("PI_CODING_AGENT_DIR", layout.agent().display().to_string());
        let output = adapter.run(&cmd).unwrap();
        assert_eq!(output.stdout, "v22.19.0\n");
        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].display(), "zsh -lic node --version");
        assert!(calls[0].env.iter().any(|(k, _)| k == "PI_CODING_AGENT_DIR"));
        drop(calls);
        let _ = (&env, &layout);
    }
}
