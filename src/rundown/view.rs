//! The Rundown tab's picture: the plan card turned into a calm, plain to-do
//! list. Pure: JSON in, styled lines out, so the look is tested without a
//! terminal.
//!
//! Only the plan card feeds it (`ha --json plan show`). Text that still reads
//! like a file name, an id or a hash is left out, because the tab is for Rolf
//! and never shows developer wording.

use serde_json::Value;

/// Where a step stands, in the tab's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Done,
    Now,
    Later,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) mark: Mark,
    pub(crate) text: String,
}

/// One project's rundown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Card {
    pub(crate) title: String,
    /// The one line saying what the project is.
    pub(crate) headline: String,
    /// What Rolf has at the end, when the headline did not already say it.
    pub(crate) finish: String,
    pub(crate) steps: Vec<Step>,
}

impl Card {
    /// Builds the card from `ha --json plan show` output: the whole reply or
    /// just its `data.result`.
    pub(crate) fn from_plan(title: &str, reply: &Value) -> Card {
        let plan = reply.pointer("/data/result").unwrap_or(reply);
        let text = |key: &str| plan[key].as_str().unwrap_or_default().trim().to_string();
        let goal = without_attribution(&text("goal"));
        let does = text("does");
        let (headline, finish) = if !goal.is_empty() && !technical(&goal) {
            (goal, plain(&does))
        } else if !does.is_empty() && !technical(&does) {
            (does, String::new())
        } else {
            (plain(&goal), plain(&does))
        };
        let steps = plan["steps"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|step| Step {
                mark: match step["state"].as_str().unwrap_or_default() {
                    "done" => Mark::Done,
                    "running" => Mark::Now,
                    _ => Mark::Later,
                },
                text: plain(step["text"].as_str().unwrap_or_default()),
            })
            .filter(|step| !step.text.is_empty())
            .collect();
        Card {
            title: title.trim().to_string(),
            headline,
            finish,
            steps,
        }
    }

    fn count(&self, mark: Mark) -> usize {
        self.steps.iter().filter(|s| s.mark == mark).count()
    }
}

/// Drops a trailing "(Rolf, 2026-09-25)" style attribution: who said it and
/// when is record keeping, not what the project is.
fn without_attribution(text: &str) -> String {
    let trimmed = text.trim_end();
    let (body, stop) = match trimmed.strip_suffix('.') {
        Some(body) => (body.trim_end(), "."),
        None => (trimmed, ""),
    };
    if let Some(inner) = body.strip_suffix(')')
        && let Some(open) = inner.rfind('(')
        && has_date(&inner[open + 1..])
    {
        return format!("{}{stop}", inner[..open].trim_end());
    }
    text.trim().to_string()
}

fn has_date(text: &str) -> bool {
    text.as_bytes().windows(10).any(|w| {
        w.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        })
    })
}

/// True when a word reads like a path, a file, an id, a hash or code.
fn technical_word(word: &str) -> bool {
    let word = word.trim_matches(|c: char| ",.;:!?()[]\"'".contains(c));
    if word.is_empty() {
        return false;
    }
    if word.contains(['/', '\\', '`', '_', '<', '>', '{', '}', '='])
        || word.contains("::")
        || word.starts_with("--")
    {
        return true;
    }
    const FILES: &[&str] = &[
        ".md", ".rs", ".toml", ".json", ".jsonl", ".py", ".ts", ".js", ".sh", ".yaml", ".yml",
        ".txt", ".lock", ".html",
    ];
    let lower = word.to_ascii_lowercase();
    if FILES
        .iter()
        .any(|ext| lower.ends_with(ext) && lower.len() > ext.len())
    {
        return true;
    }
    // Record ids: s-15, t-0508, job-0098, r198, w1:p2.
    if let Some((head, tail)) = lower.split_once('-')
        && (1..=4).contains(&head.len())
        && head.chars().all(|c| c.is_ascii_lowercase())
        && !tail.is_empty()
        && tail.chars().all(|c| c.is_ascii_digit())
    {
        return true;
    }
    if lower.len() >= 3
        && lower.starts_with(['r', 'w'])
        && lower[1..].chars().all(|c| c.is_ascii_digit())
    {
        return true;
    }
    // Commit hashes: seven or more hex characters mixing digits and letters.
    lower.len() >= 7
        && lower.chars().all(|c| c.is_ascii_hexdigit())
        && lower.chars().any(|c| c.is_ascii_digit())
        && lower.chars().any(|c| c.is_ascii_alphabetic())
}

fn technical(text: &str) -> bool {
    text.split_whitespace().any(technical_word)
}

/// The sentence without its technical words.
fn plain(text: &str) -> String {
    let text = without_attribution(text);
    if !technical(&text) {
        return text;
    }
    text.split_whitespace()
        .filter(|word| !technical_word(word))
        .collect::<Vec<_>>()
        .join(" ")
}

// ------------------------------------------------------------- drawing

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[38;5;71m";
const AMBER: &str = "\x1b[38;5;179m";
const GREY: &str = "\x1b[38;5;245m";
const TRACK: &str = "\x1b[38;5;238m";

/// The left margin and the gap between a mark and its text.
const MARGIN: usize = 3;
const MARK_GAP: usize = 3;
/// Lines stay readable on a wide screen.
const MAX_TEXT: usize = 76;

/// The styled lines for a `width` × `height` pane. `note` is a quiet line at
/// the bottom (for example when the last refresh failed).
pub(crate) fn render(card: &Card, width: usize, height: usize, note: &str) -> Vec<String> {
    let pad = " ".repeat(MARGIN);
    let text_width = width.saturating_sub(MARGIN * 2).clamp(20, MAX_TEXT);
    let mut lines = vec![String::new()];

    let title = if card.title.is_empty() {
        "Rundown"
    } else {
        &card.title
    };
    lines.push(format!("{pad}{BOLD}{title}{RESET}"));
    for line in wrap(&card.headline, text_width) {
        lines.push(format!("{pad}{line}"));
    }
    lines.push(String::new());

    if card.steps.is_empty() {
        lines.push(format!("{pad}{GREY}No steps written down yet.{RESET}"));
    } else {
        lines.push(format!("{pad}{}", progress(card, text_width)));
        lines.push(String::new());
        let hang = " ".repeat(MARGIN + 1 + MARK_GAP);
        let gap = " ".repeat(MARK_GAP);
        for step in &card.steps {
            let (mark, style) = match step.mark {
                Mark::Done => (format!("{GREEN}✓{RESET}"), GREY),
                Mark::Now => (format!("{AMBER}{BOLD}▸{RESET}"), BOLD),
                Mark::Later => (format!("{GREY}○{RESET}"), ""),
            };
            let rows = wrap(&step.text, text_width.saturating_sub(1 + MARK_GAP).max(10));
            for (i, row) in rows.iter().enumerate() {
                if i == 0 {
                    lines.push(format!("{pad}{mark}{gap}{style}{row}{RESET}"));
                } else {
                    lines.push(format!("{hang}{style}{row}{RESET}"));
                }
            }
        }
    }

    if !card.finish.is_empty() {
        lines.push(String::new());
        let finish = format!("At the end: {}", card.finish);
        for line in wrap(&finish, text_width) {
            lines.push(format!("{pad}{GREY}{line}{RESET}"));
        }
    }
    if !note.is_empty() {
        lines.push(String::new());
        lines.push(format!("{pad}{DIM}{note}{RESET}"));
    }

    if height > 0 && lines.len() > height {
        lines.truncate(height.saturating_sub(1));
        lines.push(format!("{pad}{GREY}…{RESET}"));
    }
    lines
}

/// `━━━━━━━━──── 5 of 7 done`, or a finished line when every step is done.
fn progress(card: &Card, width: usize) -> String {
    let total = card.steps.len();
    let done = card.count(Mark::Done);
    let now = card.count(Mark::Now);
    let label = if done == total && total == 1 {
        format!("{GREEN}Done{RESET}")
    } else if done == total {
        format!("{GREEN}All {total} steps done{RESET}")
    } else if now > 0 {
        format!("{done} of {total} done {GREY}·{RESET} {AMBER}{now} under way{RESET}")
    } else {
        format!("{done} of {total} done")
    };
    let bar_width = width.saturating_sub(24).clamp(10, 36);
    let filled = (bar_width * done + total / 2) / total.max(1);
    format!(
        "{GREEN}{}{TRACK}{}{RESET}  {label}",
        "━".repeat(filled),
        "━".repeat(bar_width - filled)
    )
}

/// Word wrap by characters. A word longer than the width is cut.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut len = 0;
    for word in text.split_whitespace() {
        let mut word: String = word.to_string();
        let mut word_len = word.chars().count();
        while word_len > width {
            if len > 0 {
                rows.push(std::mem::take(&mut row));
                len = 0;
            }
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            word_len -= width;
            rows.push(head);
        }
        if len > 0 && len + 1 + word_len > width {
            rows.push(std::mem::take(&mut row));
            len = 0;
        }
        if len > 0 {
            row.push(' ');
            len += 1;
        }
        row.push_str(&word);
        len += word_len;
    }
    if len > 0 || rows.is_empty() {
        rows.push(row);
    }
    rows
}

/// The text a person sees: every escape sequence removed.
#[cfg(test)]
pub(crate) fn visible(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(goal: &str, does: &str, steps: &[(&str, &str)]) -> Value {
        json!({
            "outcome": "shown",
            "data": {"result": {
                "present": !steps.is_empty(),
                "goal": goal,
                "does": does,
                "steps": steps.iter().map(|(state, text)| json!({
                    "id": "s-1", "state": state, "text": text, "threads": ["t-0001"],
                })).collect::<Vec<_>>(),
            }},
        })
    }

    fn screen(card: &Card, width: usize) -> String {
        render(card, width, 0, "")
            .iter()
            .map(|l| visible(l))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_plan_card_reads_as_a_plain_to_do_list() {
        let card = Card::from_plan(
            "Venator",
            &reply(
                "Rolf's job pipeline: bring in the right postings (Rolf, 2026-09-23).",
                "Your job dashboard shows postings sorted into skip, review and look first.",
                &[
                    ("done", "Tidy the dashboard screens."),
                    ("running", "Fix how jobs come in."),
                    ("left", "Switch the sorting on."),
                ],
            ),
        );
        let text = screen(&card, 80);
        assert!(text.contains("Venator"), "{text}");
        assert!(
            text.contains("Rolf's job pipeline: bring in the right postings."),
            "{text}"
        );
        assert!(text.contains("✓   Tidy the dashboard screens."), "{text}");
        assert!(text.contains("▸   Fix how jobs come in."), "{text}");
        assert!(text.contains("○   Switch the sorting on."), "{text}");
        assert!(text.contains("1 of 3 done · 1 under way"), "{text}");
        assert!(text.contains("At the end: Your job dashboard"), "{text}");
        for word in ["2026", "s-1", "t-0001", "running", "left"] {
            assert!(!text.contains(word), "`{word}` shows: {text}");
        }
    }

    #[test]
    fn a_goal_with_a_file_name_gives_way_to_what_rolf_gets() {
        let card = Card::from_plan(
            "Elicio",
            &reply(
                "Design the earpiece: release state S0 of docs/fab/plan-v2.md section 10.",
                "Photo-style pictures and a 3D model of the finished earpiece.",
                &[("done", "The shell shape is finished.")],
            ),
        );
        assert_eq!(
            card.headline,
            "Photo-style pictures and a 3D model of the finished earpiece."
        );
        assert!(card.finish.is_empty());
        let text = screen(&card, 80);
        assert!(!text.contains("docs/"), "{text}");
        assert!(text.contains("━━  Done"), "{text}");
    }

    #[test]
    fn a_project_without_a_plan_card_shows_its_goal_only() {
        let card = Card::from_plan("Somebody", &reply("A private writing model.", "", &[]));
        let text = screen(&card, 60);
        assert!(text.contains("A private writing model."), "{text}");
        assert!(text.contains("No steps written down yet."), "{text}");
        assert!(!text.contains("At the end"), "{text}");
    }

    #[test]
    fn ids_hashes_and_paths_never_show() {
        for word in [
            "s-15",
            "t-0508",
            "job-0098",
            "b655bbc",
            "src/bin/herdr-pi.rs",
            "PROJECT.md",
            "`ha",
            "r198",
            "HERDR_PLUGIN_STATE_DIR",
            "--json",
        ] {
            assert!(technical_word(word), "{word}");
        }
        for word in [
            "3D",
            "Photo-style",
            "attention-like",
            "fifty",
            "Jev",
            "decade",
        ] {
            assert!(!technical_word(word), "{word}");
        }
        assert_eq!(
            plain("Fix t-0508 so the tab in src/rundown shows."),
            "Fix so the tab in shows."
        );
    }

    #[test]
    fn long_steps_wrap_under_their_text_and_a_short_pane_is_cut() {
        let long = "word ".repeat(40);
        let card = Card::from_plan(
            "Demo",
            &reply("A demo.", "", &[("left", &long), ("left", "Last.")]),
        );
        let lines: Vec<String> = render(&card, 40, 0, "")
            .iter()
            .map(|l| visible(l))
            .collect();
        let first = lines.iter().position(|l| l.contains("○")).unwrap();
        assert!(lines[first + 1].starts_with("       word"), "{lines:#?}");
        assert!(lines.iter().all(|l| l.chars().count() <= 40), "{lines:#?}");
        let cut = render(&card, 40, 6, "");
        assert_eq!(cut.len(), 6);
        assert_eq!(visible(cut.last().unwrap()).trim(), "…");
    }

    #[test]
    fn a_trailing_attribution_is_dropped_but_other_brackets_stay() {
        assert_eq!(
            without_attribution("Keep going (Rolf, 2026-09-25)."),
            "Keep going."
        );
        assert_eq!(
            without_attribution("A lab (for flies) on the connectome"),
            "A lab (for flies) on the connectome"
        );
    }
}
