//! `herdr-rundown`: the Rundown tab. One project's plan card as a calm to-do
//! list: what is running, waiting or needs Rolf, alongside planned steps.
//!
//! `ha open` starts it as the `rundown` plugin pane with `HERDR_RUNDOWN_PROJECT`
//! (the project), `HERDR_RUNDOWN_TITLE` (its display name) and
//! `HERDR_ADE_ROOT` (the projects root). It reads the card through
//! `herdr-ade --json overview` and never writes a record. It never reads
//! `HERDR_PLUGIN_STATE_DIR`; it keeps no state at all.
//!
//! Updating stays cheap: once a second it compares the modification times of
//! the few records a plan's steps are derived from, asks for the card again
//! only when one moved, and asks once a minute regardless.

#[path = "../rundown/view.rs"]
mod view;

// The same bounded command runner as ADE, not a second child-process policy.
#[allow(dead_code)]
#[path = "../runner.rs"]
mod runner;

use runner::{Cmd, RealRunner, Runner};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("HP_BUILD_ID"));
const TICK: Duration = Duration::from_secs(1);
const FULL_REFRESH: Duration = Duration::from_secs(60);

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("herdr-rundown {VERSION}");
        return ExitCode::SUCCESS;
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("herdr-rundown: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

struct Source {
    slug: String,
    root: PathBuf,
    ade: PathBuf,
}

impl Source {
    fn from_env() -> Result<Source> {
        let slug = var("HERDR_RUNDOWN_PROJECT").context("HERDR_RUNDOWN_PROJECT is not set")?;
        let root = var("HERDR_ADE_ROOT")
            .map(PathBuf::from)
            .context("HERDR_ADE_ROOT is not set")?;
        // The herdr-ade built next to this binary, so both come from one commit.
        let ade = std::env::current_exe()?.with_file_name("herdr-ade");
        Ok(Source { slug, root, ade })
    }

    /// The records a plan card's step states come from.
    fn fingerprint(&self) -> Vec<Option<SystemTime>> {
        let dir = self.root.join(&self.slug);
        let state = dir.join(".state");
        [
            dir.join("PROJECT.md"),
            state.join("plan.toml"),
            state.join("tasks"),
            state.join("threads"),
            state.join("reviews"),
            state.join("events"),
            state.join("pile-holds.json"),
            state.join("coordinator-recovery.json"),
            state.join("inbox"),
        ]
        .iter()
        .map(|path| modified(path))
        .collect()
    }

    fn card(&self, title: &str) -> Result<view::Card> {
        let out = RealRunner
            .run(
                &Cmd::new(self.ade.to_string_lossy(), Duration::from_secs(15))
                    .args([
                        "--root",
                        &self.root.to_string_lossy(),
                        "--json",
                        "overview",
                        &self.slug,
                    ])
                    .own_group(),
            )
            .with_context(|| format!("could not run {}", self.ade.display()))?;
        if !out.success() {
            let reply = serde_json::from_str::<serde_json::Value>(&out.stdout).ok();
            let cause = reply
                .as_ref()
                .and_then(|reply| reply["reason"].as_str())
                .map(str::to_string)
                .unwrap_or_else(|| out.error_text());
            if cause.is_empty() {
                bail!("overview exited with {:?}", out.code);
            }
            bail!("{cause}");
        }
        let reply: serde_json::Value = serde_json::from_str(&out.stdout)?;
        if let Some(error) = reply
            .pointer("/data/result/plan/error")
            .and_then(|v| v.as_str())
        {
            bail!("plan read failed: {error}");
        }
        Ok(view::Card::from_view(title, &reply)?)
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Rows and columns of the pane, from `stty size` on the pane's own terminal.
fn size() -> (usize, usize) {
    let out = Command::new("stty")
        .arg("size")
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output();
    let parsed = out.ok().and_then(|out| {
        let text = String::from_utf8(out.stdout).ok()?;
        let mut parts = text.split_whitespace().map(|p| p.parse::<usize>().ok());
        Some((parts.next()??, parts.next()??))
    });
    parsed.unwrap_or((24, 80))
}

fn stty(args: &[&str]) {
    let _ = Command::new("stty")
        .args(args)
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .status();
}

fn run() -> Result<()> {
    let source = Source::from_env()?;
    let title = var("HERDR_RUNDOWN_TITLE").unwrap_or_else(|| source.slug.clone());

    // A picture, not a prompt: typed keys are not echoed, the cursor hides,
    // and the alternate screen keeps the pane's scrollback clean.
    stty(&["-echo", "-icanon"]);
    let mut out = std::io::stdout();
    write!(out, "\x1b[?1049h\x1b[?25l")?;
    out.flush()?;

    let mut card = view::Card {
        title: title.clone(),
        about: String::new(),
        steps: vec![],
        work: String::new(),
        needs_you: String::new(),
        actions: vec![],
    };
    let mut note = String::new();
    let mut seen = None;
    let mut fetched = None::<Instant>;
    let mut drawn = None::<(view::Card, String, (usize, usize))>;
    loop {
        let print = source.fingerprint();
        if seen.as_ref() != Some(&print) || fetched.is_none_or(|at| at.elapsed() >= FULL_REFRESH) {
            match source.card(&title) {
                Ok(fresh) => {
                    card = fresh;
                    note.clear();
                }
                Err(error) => note = format!("{error:#}"),
            }
            // A failed read must retry on the next tick even if no record
            // changed; the minute-long refresh is for healthy cards only.
            if note.is_empty() {
                seen = Some(print);
                fetched = Some(Instant::now());
            }
        }
        let screen = size();
        let state = (card.clone(), note.clone(), screen);
        if drawn.as_ref() != Some(&state) {
            let (rows, cols) = screen;
            let lines = view::render(&card, cols, rows, &note);
            write!(out, "\x1b[H\x1b[2J{}", lines.join("\r\n"))?;
            out.flush()?;
            drawn = Some(state);
        }
        std::thread::sleep(TICK);
    }
}
