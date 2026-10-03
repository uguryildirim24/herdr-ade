//! The Rundown tab's picture: the plan card turned into a calm, plain to-do
//! list. Pure: JSON in, styled lines out, so the look is tested without a
//! terminal.
//!
//! Only the plan card feeds it (`ha --json plan show`). Prefer plain words,
//! but never hide work: a technical-only row keeps its text without ids, or
//! gets a neutral numbered label.

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
    /// One level only: a subtask's own list is always empty.
    pub(crate) subtasks: Vec<Step>,
}

impl Step {
    fn from_plan(step: &Value, number: &mut usize) -> Step {
        *number += 1;
        let original = step["text"].as_str().unwrap_or_default();
        let mut text = plain(original).trim_end_matches('.').to_string();
        if text.is_empty() {
            text = original
                .split_whitespace()
                .filter(|word| !id_word(word))
                .collect::<Vec<_>>()
                .join(" ");
        }
        if text.is_empty() {
            text = format!("step {number}");
        }
        Step {
            mark: match step["state"].as_str().unwrap_or_default() {
                "done" => Mark::Done,
                "running" => Mark::Now,
                _ => Mark::Later,
            },
            text,
            subtasks: list(&step["subtasks"])
                .iter()
                .map(|sub| Step::from_plan(sub, number))
                .map(|sub| Step {
                    subtasks: Vec::new(),
                    ..sub
                })
                .collect(),
        }
    }
}

fn list(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// One project's rundown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Card {
    pub(crate) title: String,
    /// Ways to say what the project is, best first. The tab shows the first
    /// that fits beside the name on one line, or none.
    pub(crate) about: Vec<String>,
    pub(crate) steps: Vec<Step>,
}

impl Card {
    /// Builds the card from `ha --json plan show` output: the whole reply or
    /// just its `data.result`.
    pub(crate) fn from_plan(title: &str, reply: &Value) -> Card {
        let plan = reply.pointer("/data/result").unwrap_or(reply);
        let text = |key: &str| without_attribution(plan[key].as_str().unwrap_or_default());
        let mut about: Vec<String> = Vec::new();
        for sentence in [text("goal"), text("does")] {
            let clause = sentence
                .split([':', ';', '—'])
                .next()
                .unwrap_or_default()
                .to_string();
            for line in [sentence, clause] {
                let line = line.trim().trim_end_matches(['.', ',']).trim().to_string();
                if !line.is_empty() && !technical(&line) && !about.contains(&line) {
                    about.push(line);
                }
            }
        }
        let mut number = 0;
        let steps = list(&plan["steps"])
            .iter()
            .map(|step| Step::from_plan(step, &mut number))
            .collect();
        Card {
            title: title.trim().to_string(),
            about,
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
    id_word(word)
}

/// Record ids and hashes are removed even from a technical-only fallback.
fn id_word(word: &str) -> bool {
    let lower = word
        .trim_matches(|c: char| ",.;:!?()[]\"'`".contains(c))
        .to_ascii_lowercase();
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
    if let Some((workspace, pane)) = lower.split_once(":p")
        && workspace.starts_with('w')
        && workspace.len() > 1
        && workspace[1..].chars().all(|c| c.is_ascii_digit())
        && !pane.is_empty()
        && pane.chars().all(|c| c.is_ascii_digit())
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
    let words: Vec<_> = text.split_whitespace().collect();
    words.iter().enumerate()
        .filter(|(i, word)| {
            !technical_word(word)
                // Remove a preposition with its stripped object, not a dangling "in".
                && !(matches!(word.to_ascii_lowercase().as_str(), "in" | "on" | "at" | "from" | "to" | "under")
                    && words.get(i + 1).is_some_and(|next| technical_word(next)))
        })
        .map(|(_, word)| *word)
        .collect::<Vec<_>>()
        .join(" ")
}

// ------------------------------------------------------------- drawing

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";

/// A soft palette: warm text on the terminal's own background.
const TITLE: Rgb = Rgb(245, 224, 220);
const TEXT: Rgb = Rgb(205, 214, 244);
const QUIET: Rgb = Rgb(147, 153, 178);
const FAINT: Rgb = Rgb(88, 91, 112);
const FRAME: Rgb = Rgb(69, 71, 90);
const SURFACE: Rgb = Rgb(36, 39, 58);
const TRACK: Rgb = Rgb(49, 50, 68);
const INK: Rgb = Rgb(30, 30, 46);
const GREEN: Rgb = Rgb(166, 227, 161);
const TEAL: Rgb = Rgb(148, 226, 213);
const AMBER: Rgb = Rgb(249, 226, 175);
const ACCENT: Rgb = Rgb(203, 166, 247);
const SKY: Rgb = Rgb(137, 180, 250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn fg(self) -> String {
        format!("\x1b[38;2;{};{};{}m", self.0, self.1, self.2)
    }

    fn bg(self) -> String {
        format!("\x1b[48;2;{};{};{}m", self.0, self.1, self.2)
    }

    /// The colour `t` (0 to 1) of the way from `self` to `to`.
    fn toward(self, to: Rgb, t: f32) -> Rgb {
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Rgb(mix(self.0, to.0), mix(self.1, to.1), mix(self.2, to.2))
    }
}

/// The panel never grows past this, so a wide pane keeps a compact list.
const MAX_PANEL: usize = 76;
/// Space between the panel's frame and what is inside it.
const PAD: usize = 3;

/// The styled lines for a `width` × `height` pane: one framed panel with the
/// project's name and what it is, a progress bar, and the steps as a
/// to-do list joined by a thin line. `note` is a quiet line under the panel
/// (for example when the last refresh failed).
pub(crate) fn render(card: &Card, width: usize, height: usize, note: &str) -> Vec<String> {
    // Too narrow for the frame and its progress count: show just the name
    // rather than printing a frame wider than the terminal.
    if width < 28 {
        return vec![cut(
            if card.title.is_empty() {
                "Rundown"
            } else {
                &card.title
            },
            width,
        )];
    }
    let mut panel = width.saturating_sub(4).clamp(28, MAX_PANEL);
    // Equal margins left and right, including at 29 and 31 columns.
    if (width - panel) % 2 == 1 {
        panel -= 1;
    }
    let inner = panel - 2 - PAD * 2;
    let indent = " ".repeat(width.saturating_sub(panel) / 2);

    let title = if card.title.is_empty() {
        "Rundown"
    } else {
        &card.title
    };
    let about = match card.about.iter().find(|a| len(a) <= inner) {
        Some(about) => about.clone(),
        None => card
            .about
            .first()
            .map(|a| cut(a, inner))
            .unwrap_or_default(),
    };
    let mut header = vec![String::new(), shine(&cut(title, inner))];
    if !about.is_empty() {
        header.push(format!("{}{about}", QUIET.fg()));
    }
    header.push(String::new());
    let mut body = vec![String::new()];
    if card.steps.is_empty() {
        body.push(format!("{}No steps yet{RESET}", QUIET.fg()));
        body.push(String::new());
    } else {
        body.push(progress(card, inner));
        body.push(String::new());
        body.push(String::new());
        // A blank line (carrying the joining line) between steps while the
        // pane has the height; a short pane packs them. Subtasks sit right
        // under their step, the joining line running past them.
        let fixed = header.len() + body.len() + 3 + usize::from(!note.is_empty());
        let subtasks: usize = card.steps.iter().map(|s| s.subtasks.len()).sum();
        let spaced = height == 0 || fixed + card.steps.len() * 2 - 1 + subtasks <= height;
        let joint = |step: &Step| {
            if step.mark == Mark::Done {
                GREEN.toward(INK, 0.45)
            } else {
                FAINT
            }
        };
        for (i, step) in card.steps.iter().enumerate() {
            if spaced && i > 0 {
                body.push(format!(" {}│{RESET}", joint(&card.steps[i - 1]).fg()));
            }
            body.push(row(step, inner));
            let joined = spaced && i + 1 < card.steps.len();
            for sub in &step.subtasks {
                body.push(sub_row(sub, inner, joined.then_some(joint(step))));
            }
        }
        body.push(String::new());
    }

    let rule = "─".repeat(panel - 2);
    let frame = FRAME.fg();
    let mut lines = vec![format!("{indent}{frame}╭{rule}╮{RESET}")];
    let pad = " ".repeat(PAD);
    let band = SURFACE.bg();
    for content in header {
        let fill = " ".repeat(inner.saturating_sub(len(&visible(&content))));
        lines.push(format!(
            "{indent}{frame}│{band}{pad}{content}{band}{fill}{pad}{RESET}{frame}│{RESET}"
        ));
    }
    for content in body {
        let fill = " ".repeat(inner.saturating_sub(len(&visible(&content))));
        lines.push(format!(
            "{indent}{frame}│{RESET}{pad}{content}{fill}{pad}{frame}│{RESET}"
        ));
    }
    lines.push(format!("{indent}{frame}╰{rule}╯{RESET}"));
    let frame_lines = lines.len();
    if !note.is_empty() {
        lines.push(format!(
            "{indent}   {}{}{RESET}",
            QUIET.fg(),
            cut(note, panel - 3)
        ));
    }
    // The frame sits in the middle of the pane; the note hangs below it, so
    // the frame never moves when the note comes and goes.
    if height > frame_lines {
        let top = (height - frame_lines) / 2;
        lines.splice(0..0, std::iter::repeat_n(String::new(), top));
    }
    if height > 0 && lines.len() > height {
        lines.truncate(height.saturating_sub(1));
        lines.push(format!("{indent}   {}…{RESET}", QUIET.fg()));
    }
    lines
}

/// The project's name in bold, its letters shading from violet to sky.
fn shine(title: &str) -> String {
    let count = len(title).max(2) - 1;
    let mut out = String::from(BOLD);
    for (i, c) in title.chars().enumerate() {
        out.push_str(&ACCENT.toward(SKY, i as f32 / count as f32).fg());
        out.push(c);
    }
    out.push_str(RESET);
    out
}

/// The marks, only from glyphs that Rolf's terminal font (JetBrainsMono
/// Nerd Font Mono) carries dead center in its upright weights: a glyph it
/// lacks comes from a fallback font and sits off center. Never italic.
const DONE: char = '✶';
const NOW: char = '◉';
const LATER: char = '◌';

/// One step: a coloured box with its mark, then a few words. A finished
/// step gets a star, the one under way a filled ring, and one still to do
/// an empty dotted circle.
fn row(step: &Step, width: usize) -> String {
    let text = cut(&step.text, width.saturating_sub(5));
    match step.mark {
        Mark::Done => format!(
            "{}  {}{text}{RESET}",
            tile(GREEN, INK, true, DONE),
            QUIET.fg()
        ),
        Mark::Now => format!(
            "{}  {BOLD}{}{text}{RESET}",
            tile(AMBER, INK, true, NOW),
            AMBER.fg()
        ),
        Mark::Later => format!(
            "{}  {}{text}{RESET}",
            tile(TRACK, QUIET, false, LATER),
            TEXT.fg()
        ),
    }
}

/// One subtask, under its step's words: the same mark without its box, so
/// it reads smaller. `line` carries the joining line down to the next step.
fn sub_row(sub: &Step, width: usize, line: Option<Rgb>) -> String {
    let lead = match line {
        Some(color) => format!(" {}│{RESET}   ", color.fg()),
        None => " ".repeat(5),
    };
    let text = cut(&sub.text, width.saturating_sub(8));
    let (mark, color, words) = match sub.mark {
        Mark::Done => (DONE, GREEN, QUIET),
        Mark::Now => (NOW, AMBER, AMBER),
        Mark::Later => (LATER, FAINT, TEXT),
    };
    format!(
        "{lead}{}{mark}{RESET}  {}{text}{RESET}",
        color.fg(),
        words.fg()
    )
}

/// A square box with `mark` in its middle. A cell is about twice as tall as
/// it is wide, so the box is the mark's cell plus half a cell each side:
/// `▐` and `▌` drawn in the box colour. Two cells wide, one row tall.
fn tile(color: Rgb, ink: Rgb, bold: bool, mark: char) -> String {
    let weight = if bold { BOLD } else { "" };
    format!(
        "{edge}▐{RESET}{}{}{weight}{mark}{RESET}{edge}▌{RESET}",
        color.bg(),
        ink.fg(),
        edge = color.fg()
    )
}

/// A chunky bar that warms from teal to green as it fills, and the count.
fn progress(card: &Card, width: usize) -> String {
    let total = card.steps.len();
    let done = card.count(Mark::Done);
    let label = format!("{done} of {total}");
    let bar = width.saturating_sub(len(&label) + 3).min(44);
    let filled = (bar * done + total / 2) / total.max(1);
    let mut out = String::new();
    for i in 0..bar {
        let color = if i < filled {
            TEAL.toward(GREEN, i as f32 / bar.max(2) as f32)
        } else {
            TRACK
        };
        out.push_str(&color.fg());
        out.push('█');
    }
    let count = if done == total { GREEN } else { TITLE };
    format!("{out}{RESET}   {BOLD}{}{label}{RESET}", count.fg())
}

fn len(text: &str) -> usize {
    text.chars().count()
}

/// `text` on one line of at most `width` characters: cut at a word boundary
/// and ended with "…" when it is too long.
fn cut(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if len(text) <= width {
        return text.to_string();
    }
    let room = width - 1;
    let mut out = String::new();
    for word in text.split_whitespace() {
        let next = if out.is_empty() {
            len(word)
        } else {
            len(&out) + 1 + len(word)
        };
        if next > room {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        out = text.chars().take(room).collect();
    }
    format!("{}…", out.trim_end_matches([',', ';', ':', '.', '-', ' ']))
}

/// The text a person sees: every escape sequence removed.
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

    fn screen(card: &Card, width: usize) -> String {
        render(card, width, 0, "")
            .iter()
            .map(|l| visible(l))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn path_only_steps_and_subtasks_survive_with_plan_counts() {
        // The plan-show result, including text that the plain-words filter empties.
        let plan = json!({"steps": [
            {"id": "s-1", "state": "done", "text": "src/rundown/view.rs"},
            {"id": "s-2", "state": "left", "text": "t-0508", "subtasks": [
                {"id": "s-3", "state": "left", "text": "notes.md"},
                {"id": "s-4", "state": "left", "text": "job-0001"},
            ]},
        ]});
        let card = Card::from_plan("Demo", &plan);
        let steps = plan["steps"].as_array().unwrap();
        assert_eq!(card.steps.len(), steps.len());
        assert_eq!(
            card.count(Mark::Done),
            steps.iter().filter(|s| s["state"] == "done").count()
        );
        assert_eq!(card.steps[0].text, "src/rundown/view.rs");
        assert_eq!(card.steps[1].text, "step 2");
        assert_eq!(card.steps[1].subtasks.len(), 2);
        assert_eq!(card.steps[1].subtasks[0].text, "notes.md");
        assert_eq!(card.steps[1].subtasks[1].text, "step 4");
        let text = screen(&card, 80);
        for row in [
            "src/rundown/view.rs",
            "step 2",
            "notes.md",
            "step 4",
            "1 of 2",
        ] {
            assert!(text.contains(row), "{row}: {text}");
        }
    }
}
