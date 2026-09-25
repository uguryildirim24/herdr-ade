//! `herdr-rundown`: the Rundown tab. One project's plan card as a calm to-do
//! list: what the project is, its steps, and which are done.
//!
//! `ha open` starts it as the `rundown` plugin pane with `HERDR_RUNDOWN_PROJECT`
//! (the project), `HERDR_RUNDOWN_TITLE` (its display name) and
//! `HERDR_ADE_ROOT` (the projects root). It reads the card through
//! `herdr-ade --json plan show` and never writes a record. It never reads
//! `HERDR_PLUGIN_STATE_DIR`; it keeps no state at all.
//!
//! Updating stays cheap: once a second it compares the modification times of
//! the few records a plan's steps are derived from, asks for the card again
//! only when one moved, and asks once a minute regardless.

#[path = "../rundown/view.rs"]
mod view;

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
        let ade = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("herdr-ade")))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("herdr-ade"));
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
            state.join("rounds"),
            state.join("events"),
        ]
        .iter()
        .map(|path| modified(path))
        .collect()
    }

    fn card(&self, title: &str) -> Result<view::Card> {
        let out = Command::new(&self.ade)
            .arg("--root")
            .arg(&self.root)
            .args(["--json", "plan", "show", &self.slug])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .with_context(|| format!("could not run {}", self.ade.display()))?;
        if !out.status.success() {
            bail!("plan show exited with {}", out.status);
        }
        let reply: serde_json::Value = serde_json::from_slice(&out.stdout)?;
        Ok(view::Card::from_plan(title, &reply))
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

    let mut card: Option<view::Card> = None;
    let mut note = String::new();
    let mut seen = None;
    let mut fetched = None::<Instant>;
    let mut drawn = None::<(Option<view::Card>, String, (usize, usize))>;
    loop {
        let print = source.fingerprint();
        if seen.as_ref() != Some(&print) || fetched.is_none_or(|at| at.elapsed() >= FULL_REFRESH) {
            match source.card(&title) {
                Ok(fresh) => {
                    card = Some(fresh);
                    note.clear();
                }
                Err(_) if card.is_some() => {
                    note = "Could not check for changes just now; trying again.".into()
                }
                Err(_) => note = "Getting the plan…".into(),
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
            let lines = match &card {
                Some(card) => view::render(card, cols, rows, &note),
                None => vec![String::new(), format!("   {note}")],
            };
            write!(out, "\x1b[H\x1b[2J{}", lines.join("\r\n"))?;
            out.flush()?;
            drawn = Some(state);
        }
        std::thread::sleep(TICK);
    }
}
