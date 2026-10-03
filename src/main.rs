mod output;

// Route command prose through one result renderer. The default renderer keeps
// streaming the same bytes; `--json` collects them into the result record.
macro_rules! print {
    ($($arg:tt)*) => { $crate::output::write_stdout(format_args!($($arg)*)) };
}
macro_rules! println {
    () => { $crate::output::write_stdout(format_args!("\n")) };
    ($($arg:tt)*) => {{
        $crate::output::write_stdout(format_args!($($arg)*));
        $crate::output::write_stdout(format_args!("\n"));
    }};
}
macro_rules! eprintln {
    () => { $crate::output::write_stderr(format_args!("\n")) };
    ($($arg:tt)*) => {{
        $crate::output::write_stderr(format_args!($($arg)*));
        $crate::output::write_stderr(format_args!("\n"));
    }};
}

mod actions;
mod adapters;
mod adopt;
mod ask;
mod awake;
mod branches;
mod build;
mod cli;
mod config;
mod contracts;
mod coordinator;
mod doctor;
mod events;
mod git;
mod handoff;
mod harness;
mod herdr;
mod hook;
mod inbox;
mod lane;
mod launch;
mod lifecycle;
mod note;
mod ops;
mod overview;
mod paths;
mod record_cache;
mod recovery;
mod routing;
// Shared with the `herdr-pi` binary: setup, install and login run only there.
mod claude_trust;
mod gate_paths;
#[allow(dead_code)] // Shared with herdr-pi; each binary uses a different subset.
mod pi;
#[path = "pi/ade.rs"]
mod pi_ade;
mod plan;
mod project;
mod project_view;
mod prompt;
mod refusal;
mod remote;
mod repo;
mod review;
mod rundown;
mod runner;
#[cfg(test)]
mod scenarios;
mod steps;
mod task;
#[cfg(test)]
mod testkit;
mod thread;
mod threads;
mod ticker;
mod usage;
mod worktrees;

/// Crate version plus a build identifier (short git hash and build time), so a
/// rebuilt binary always differs from the one a running ticker was started from.
pub(crate) const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("HP_BUILD_ID"));

/// A herdr server that was not started from a login shell hands its plugins a
/// minimal `PATH`, so `gh`, `rsync` or the agent CLI may be missing for the
/// ticker although they work in the user's terminal. The usual install folders
/// are appended (never prepended: what the user's `PATH` resolves still wins).
fn extend_path() {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let mut extra: Vec<std::path::PathBuf> =
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
            .iter()
            .map(Into::into)
            .collect();
    if let Some(home) = home {
        extra.push(home.join(".local/bin"));
        extra.push(home.join(".cargo/bin"));
    }
    for dir in extra {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    if let Ok(joined) = std::env::join_paths(dirs) {
        // SAFETY: first thing in `main`, before any thread exists.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

fn main() {
    extend_path();
    if let Err(error) = cli::run() {
        let _ = output::finish_error(&format!("{error:#}"));
        std::process::exit(1);
    }
}
