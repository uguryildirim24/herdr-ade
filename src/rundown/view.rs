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

    /// One step holding four subtasks in mixed states, between two others.
    fn with_subtasks() -> Card {
        Card::from_plan(
            "Herdr ADE",
            &json!({"data": {"result": {
                "goal": "A calm harness for coding agents.",
                "does": "",
                "steps": [
                    {"id": "s-1", "state": "done", "text": "Cut unused parts."},
                    {"id": "s-2", "state": "running", "text": "Subtasks in the plan", "subtasks": [
                        {"id": "s-5", "state": "done", "text": "Store them under their step."},
                        {"id": "s-6", "state": "done", "text": "Add them from the command line"},
                        {"id": "s-7", "state": "running", "text": "Show them in the tab t-0540 src/rundown"},
                        {"id": "s-8", "state": "left", "text": "Check how they look on a narrow pane"},
                    ]},
                    {"id": "s-3", "state": "left", "text": "One reviewer for the pile"},
                ],
            }}}),
        )
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
                    ("running", "Fix how jobs come in"),
                    ("left", "Switch the sorting on"),
                ],
            ),
        );
        let text = screen(&card, 80);
        assert!(text.contains("Venator"), "{text}");
        assert!(
            text.contains("Rolf's job pipeline: bring in the right postings"),
            "{text}"
        );
        assert!(text.contains("▐✶▌  Tidy the dashboard screens "), "{text}");
        assert!(text.contains("▐◉▌  Fix how jobs come in"), "{text}");
        assert!(text.contains("▐◌▌  Switch the sorting on"), "{text}");
        assert!(text.contains("█   1 of 3"), "{text}");
        for word in [
            "2026",
            "s-1",
            "t-0001",
            "running",
            "left",
            "dashboard shows",
        ] {
            assert!(!text.contains(word), "`{word}` shows: {text}");
        }
    }

    #[test]
    fn a_goal_with_a_file_name_gives_way_to_its_plain_first_part() {
        let card = Card::from_plan(
            "Elicio",
            &reply(
                "Design the earpiece: release state S0 of docs/fab/plan-v2.md section 10.",
                "Photo-style pictures and a 3D model of the finished earpiece.",
                &[("done", "The shell shape is finished.")],
            ),
        );
        assert_eq!(
            card.about,
            [
                "Design the earpiece",
                "Photo-style pictures and a 3D model of the finished earpiece",
            ]
        );
        let text = screen(&card, 80);
        assert!(!text.contains("docs/"), "{text}");
        assert!(text.contains("1 of 1"), "{text}");
    }

    #[test]
    fn a_project_without_a_plan_card_shows_its_goal_only() {
        let card = Card::from_plan("Somebody", &reply("A private writing model.", "", &[]));
        let text = screen(&card, 60);
        assert!(text.contains("A private writing model"), "{text}");
        assert!(text.contains("No steps yet"), "{text}");
        assert!(!text.contains(" of "), "{text}");
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
            "Fix so the tab shows."
        );
    }

    #[test]
    fn every_line_fits_and_a_long_step_is_cut_at_a_word() {
        let long = "Make the long step read well on one line even when the pane is narrow";
        let card = Card::from_plan(
            "Demo",
            &reply(
                "A demo project whose goal is also much too long to fit beside anything at all here",
                "",
                &[("left", long), ("done", "Last")],
            ),
        );
        for width in [29, 31, 40, 80, 90, 160, 200] {
            let lines: Vec<String> = render(&card, width, 0, "")
                .iter()
                .map(|l| visible(l))
                .collect();
            assert!(
                lines.iter().all(|l| l.chars().count() <= width),
                "{width}: {lines:#?}"
            );
            let step = lines.iter().find(|l| l.contains("Make")).unwrap();
            assert!(!step.contains(long), "{step}");
            assert!(step.trim_end_matches([' ', '│']).ends_with('…'), "{step}");
        }
        assert_eq!(cut("One reviewer for the pile", 14), "One reviewer…");
        assert_eq!(cut("Short", 14), "Short");
        assert_eq!(visible(&render(&card, 16, 20, "")[0]), "Demo");
        assert_eq!(render(&card, 0, 20, ""), [""]);
        let six = Card::from_plan(
            "Demo",
            &reply(
                "A demo.",
                "",
                &[
                    ("done", "One"),
                    ("done", "Two"),
                    ("running", "Three"),
                    ("left", "Four"),
                    ("left", "Five"),
                    ("left", "Six"),
                ],
            ),
        );
        // 11 rows around the list: 6 packed steps fit in 20, spaced do not.
        let packed: Vec<String> = render(&six, 80, 20, "")
            .iter()
            .map(|l| visible(l))
            .collect();
        assert!(packed.iter().any(|l| l.contains("Six")), "{packed:#?}");
        assert!(!packed.iter().any(|l| l.trim() == "…"), "{packed:#?}");
        let spaced: Vec<String> = render(&six, 80, 23, "")
            .iter()
            .map(|l| visible(l))
            .collect();
        assert!(spaced.iter().any(|l| l.contains("Six")), "{spaced:#?}");
        assert!(
            spaced.iter().any(|l| l.trim_start().starts_with("│    │")),
            "{spaced:#?}"
        );
        let short = render(&card, 80, 8, "");
        assert_eq!(short.len(), 8);
        assert_eq!(visible(short.last().unwrap()).trim(), "…");
    }

    #[test]
    fn the_panel_sits_in_the_middle_of_the_pane() {
        let card = Card::from_plan("Demo", &reply("A demo.", "", &[("left", "One")]));
        for (width, height) in [(80, 60), (90, 41), (161, 30)] {
            for note in ["", "Trying again."] {
                let lines: Vec<String> = render(&card, width, height, note)
                    .iter()
                    .map(|l| visible(l))
                    .collect();
                let top = lines.iter().position(|l| l.contains('╭')).unwrap();
                let bottom = lines.iter().position(|l| l.contains('╰')).unwrap();
                let below = height - 1 - bottom;
                assert!(below == top || below == top + 1, "{top} {below}");
                let left = lines[top].chars().take_while(|c| *c == ' ').count();
                let right = width - left - lines[top].trim().chars().count();
                assert_eq!(left, right, "{width}");
            }
        }
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

    #[test]
    fn subtasks_sit_under_their_step_with_smaller_marks() {
        let card = with_subtasks();
        assert_eq!(card.count(Mark::Done), 1);
        let text = screen(&card, 80);
        assert!(text.contains("1 of 3"), "{text}");
        for line in [
            "▐◉▌  Subtasks in the plan",
            " │   ✶  Store them under their step ",
            " │   ✶  Add them from the command line ",
            " │   ◉  Show them in the tab ",
            " │   ◌  Check how they look on a narrow pane ",
            " │\n",
            "▐◌▌  One reviewer for the pile",
        ] {
            assert!(text.contains(line), "`{line}` missing: {text}");
        }
        for word in ["t-0540", "src/", "s-5", "running", "left"] {
            assert!(!text.contains(word), "`{word}` shows: {text}");
        }
        // The last step's subtasks have no line running past them.
        let last = Card {
            steps: card.steps[1..2].to_vec(),
            ..card.clone()
        };
        assert!(screen(&last, 80).contains("│        ✶  Store them"));
        // 11 rows around the list, 5 for steps and joins, 4 for subtasks: a
        // pane of 19 packs them and drops the joining line, one of 20 fits.
        let packed: Vec<String> = render(&card, 80, 19, "")
            .iter()
            .map(|l| visible(l))
            .collect();
        assert!(
            packed.iter().any(|l| l.contains("One reviewer")),
            "{packed:#?}"
        );
        assert!(!packed.iter().any(|l| l.contains("│   ✶")), "{packed:#?}");
        let spaced: Vec<String> = render(&card, 80, 23, "")
            .iter()
            .map(|l| visible(l))
            .collect();
        assert!(
            spaced.iter().any(|l| l.contains("│    │   ✶")),
            "{spaced:#?}"
        );
        assert!(!spaced.iter().any(|l| l.trim() == "…"), "{spaced:#?}");
        for width in [29, 31, 40, 80, 90, 160, 200] {
            let lines: Vec<String> = render(&card, width, 0, "")
                .iter()
                .map(|l| visible(l))
                .collect();
            assert!(
                lines.iter().all(|l| l.chars().count() <= width),
                "{width}: {lines:#?}"
            );
            let sub = lines.iter().find(|l| l.contains("Store")).unwrap();
            assert!(sub.contains("✶  Store"), "{width}: {sub}");
        }
    }

    #[test]
    fn every_mark_is_one_the_terminal_font_centers() {
        // Glyphs JetBrainsMono Nerd Font Mono (Rolf's Ghostty font) carries
        // dead center in its cell in the upright weights. A mark outside this
        // set comes from a fallback font and sits off center.
        const CENTERED: &str = "✶◉◌◎●○◆◇▪▫";
        // Drawing, not marks: the frame, box halves, bar and cut ellipsis.
        const DRAWING: &str = "─│╭╮╰╯▐▌█…";
        let card = with_subtasks();
        let mut seen = std::collections::BTreeSet::new();
        for width in [31, 80] {
            for line in render(&card, width, 0, "") {
                assert!(!line.contains("\x1b[3m"), "italic: {line:?}");
                for c in visible(&line).chars().filter(|c| !c.is_ascii()) {
                    if !DRAWING.contains(c) {
                        assert!(CENTERED.contains(c), "`{c}` is not a centered mark");
                        seen.insert(c);
                    }
                }
            }
        }
        assert_eq!(seen.into_iter().collect::<String>(), "◉◌✶");
    }

    /// The proof captures for the report: `cargo test -- --ignored --nocapture
    /// rundown_captures`.
    #[test]
    #[ignore]
    fn rundown_captures() {
        let plain = Card::from_plan(
            "Herdr ADE",
            &reply(
                "A calm harness for coding agents.",
                "",
                &[
                    ("done", "Cut unused parts"),
                    ("running", "Subtasks in the plan"),
                    ("left", "One reviewer for the pile"),
                ],
            ),
        );
        for (name, card, width, height) in [
            ("no subtasks, 80 x 24", &plain, 80, 24),
            ("four subtasks, 80 x 30", &with_subtasks(), 80, 30),
            ("four subtasks, 31 x 30", &with_subtasks(), 31, 30),
        ] {
            println!("--- {name}");
            for line in render(card, width, height, "") {
                println!("{}", visible(&line));
            }
        }
    }
}
