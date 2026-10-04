//! `herdr-rundown`: the Rundown tab. One project's plan card as a calm to-do
//! list: what is running, waiting or needs Rolf, alongside planned steps.
//!
//! `ha open` starts it as the `rundown` plugin pane with `HERDR_RUNDOWN_PROJECT`
//! (the project), `HERDR_RUNDOWN_TITLE` (its display name) and
//! `HERDR_ADE_ROOT` (the projects root). It reads the card through
//! `herdr-ade --json overview` and never writes a record. It never reads
//! `HERDR_PLUGIN_STATE_DIR`; it keeps no state at all.
//! `--print` emits the same picture without terminal controls and exits.
//! `--all` (or an unset project) reads every project, isolating card failures.
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
        let slug = var("HERDR_RUNDOWN_PROJECT").unwrap_or_default();
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
            state.join("goal-check.json"),
            self.root.join("adeherdr/.state/reviews"),
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
        let title = if title.is_empty() {
            reply
                .pointer("/data/result/title")
                .and_then(|value| value.as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(&self.slug)
        } else {
            title
        };
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
    let sources = if source.slug.is_empty() || std::env::args().any(|arg| arg == "--all") {
        let mut sources = Vec::new();
        for entry in std::fs::read_dir(&source.root)? {
            let entry = entry?;
            if entry.path().join("PROJECT.md").exists() {
                sources.push(Source {
                    slug: entry.file_name().to_string_lossy().into_owned(),
                    root: source.root.clone(),
                    ade: source.ade.clone(),
                });
            }
        }
        sources.sort_by(|a, b| a.slug.cmp(&b.slug));
        sources
    } else {
        vec![source]
    };
    let mut panels: Vec<_> = sources
        .into_iter()
        .map(|source| {
            let title = if var("HERDR_RUNDOWN_PROJECT").as_deref() == Some(&source.slug) {
                var("HERDR_RUNDOWN_TITLE").unwrap_or_default()
            } else {
                String::new()
            };
            Panel {
                card: empty_card(if title.is_empty() {
                    source.slug.clone()
                } else {
                    title.clone()
                }),
                source,
                title,
                note: String::new(),
                seen: None,
                fetched: None,
            }
        })
        .collect();
    if std::env::args().any(|arg| arg == "--print") {
        for panel in &mut panels {
            panel.refresh();
        }
        for line in draw(&panels, 80, 0) {
            println!("{}", view::visible(&line));
        }
        let errors = panels
            .iter()
            .filter(|panel| !panel.note.is_empty())
            .map(|panel| panel.note.as_str())
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            bail!(
                "one or more project overviews unavailable: {}",
                errors.join("; ")
            );
        }
        return Ok(());
    }

    // A picture, not a prompt: typed keys are not echoed, the cursor hides,
    // and the alternate screen keeps the pane's scrollback clean.
    stty(&["-echo", "-icanon"]);
    let mut out = std::io::stdout();
    write!(out, "\x1b[?1049h\x1b[?25l")?;
    out.flush()?;

    let mut drawn = Vec::new();
    loop {
        for panel in &mut panels {
            panel.refresh();
        }
        let (rows, cols) = size();
        let lines = draw(&panels, cols, rows);
        if drawn != lines {
            write!(out, "\x1b[H\x1b[2J{}", lines.join("\r\n"))?;
            out.flush()?;
            drawn = lines;
        }
        std::thread::sleep(TICK);
    }
}

fn empty_card(title: String) -> view::Card {
    view::Card {
        title,
        about: String::new(),
        steps: vec![],
        read_error: String::new(),
        plan_unreadable: true,
        activity: Default::default(),
        needs_you_items: vec![],
        harness: Default::default(),
    }
}

struct Panel {
    source: Source,
    title: String,
    card: view::Card,
    note: String,
    seen: Option<Vec<Option<SystemTime>>>,
    fetched: Option<Instant>,
}

impl Panel {
    fn refresh(&mut self) {
        let fingerprint = self.source.fingerprint();
        if self.seen.as_ref() == Some(&fingerprint)
            && self.fetched.is_some_and(|at| at.elapsed() < FULL_REFRESH)
        {
            return;
        }
        match self.source.card(&self.title) {
            Ok(card) => {
                self.note = card.read_error.clone();
                self.card = card;
                if self.note.is_empty() {
                    self.seen = Some(fingerprint);
                    self.fetched = Some(Instant::now());
                }
            }
            Err(error) => {
                self.note = "Overview unavailable; retrying".into();
                // Keep technical details out of the card, but available in --print's stderr.
                if std::env::args().any(|arg| arg == "--print") {
                    eprintln!("overview {}: {error:#}", self.source.slug);
                }
            }
        }
    }
}

fn draw(panels: &[Panel], width: usize, height: usize) -> Vec<String> {
    let harness = panels
        .iter()
        .find(|panel| panel.note.is_empty())
        .map(|panel| &panel.card.harness)
        .cloned()
        .unwrap_or_default();
    let mut lines = vec![view::harness_line(&harness, width, jiff::Timestamp::now())];
    for panel in panels {
        let room = if height == 0 {
            0
        } else {
            (height.saturating_sub(1) / panels.len().max(1)).max(1)
        };
        lines.extend(view::render(&panel.card, width, room, &panel.note));
    }
    if height > 0 {
        lines.truncate(height);
    }
    lines
}
