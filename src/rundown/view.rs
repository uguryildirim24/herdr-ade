//! The Rundown tab's picture: the plan card turned into a calm to-do list.
//! Pure: JSON in, styled lines out, so the look is tested without a terminal.
//!
//! The shared project view feeds it (`ha --json overview`). Step counts are
//! planning facts; current process activity is a separate explanation.

use serde::Deserialize;
use serde_json::Value;

/// Where a step stands, in the tab's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) enum Mark {
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "running")]
    Started,
    #[serde(rename = "left")]
    Later,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Step {
    #[serde(rename = "state")]
    pub(crate) mark: Mark,
    pub(crate) text: String,
    /// One level only: a subtask's own list is always empty.
    #[serde(default)]
    pub(crate) subtasks: Vec<Step>,
    #[serde(default)]
    failed_check_hold: Option<Hold>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Hold {
    message: String,
}

#[derive(Deserialize)]
struct PlanView {
    #[serde(rename = "schema")]
    _schema: u32,
    #[serde(rename = "revision")]
    _revision: u64,
    #[serde(default)]
    goal: String,
    #[serde(default)]
    does: String,
    #[serde(default)]
    what_you_get: String,
    steps: Vec<Step>,
}

/// One project's rundown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Card {
    pub(crate) title: String,
    /// Authored outcome, cut only to fit the panel.
    pub(crate) about: String,
    pub(crate) steps: Vec<Step>,
    pub(crate) work: String,
    pub(crate) needs_you: String,
    pub(crate) actions: Vec<String>,
}

#[derive(Deserialize)]
struct ProjectView {
    plan: PlanView,
    work: String,
    needs_you: String,
    actions: Vec<String>,
}

impl Card {
    /// Builds the card from the existing overview command's typed result.
    pub(crate) fn from_view(title: &str, reply: &Value) -> serde_json::Result<Card> {
        let view: ProjectView =
            serde_json::from_value(reply.pointer("/data/result").unwrap_or(reply).clone())?;
        let plan = view.plan;
        let about = [plan.does, plan.goal, plan.what_you_get]
            .into_iter()
            .find(|text| !text.is_empty())
            .unwrap_or_default();
        Ok(Card {
            title: title.trim().to_string(),
            about,
            steps: plan.steps,
            work: view.work,
            needs_you: view.needs_you,
            actions: view.actions,
        })
    }

    fn count(&self, mark: Mark) -> usize {
        self.steps.iter().filter(|s| s.mark == mark).count()
    }
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

/// Reserve the current-work and error slots before spending space on history.
pub(crate) fn render(card: &Card, width: usize, height: usize, note: &str) -> Vec<String> {
    let framed = width >= 28 && (height == 0 || height >= 10);
    let panel = width.saturating_sub(4).min(MAX_PANEL);
    let inner = if framed {
        panel.saturating_sub(2 + PAD * 2)
    } else {
        width
    };
    let room = if height == 0 {
        usize::MAX
    } else {
        height.saturating_sub(if framed { 2 } else { 0 })
    };
    let title = if card.title.is_empty() {
        "Rundown"
    } else {
        &card.title
    };
    let mut content = vec![shine(&cut(title, inner))];
    if !note.is_empty() {
        content.push(format!("{}{}{RESET}", AMBER.fg(), cut(note, inner)));
    }
    if !card.needs_you.is_empty() {
        content.push(format!(
            "{}{}{RESET}",
            AMBER.fg(),
            cut(&format!("Needs you: {}", card.needs_you), inner)
        ));
    }
    content.push(format!("{}{}{RESET}", TEXT.fg(), cut(&card.work, inner)));
    content.push(progress(card, inner));
    if !card.about.is_empty() && content.len() + 2 < room {
        content.push(format!("{}{}{RESET}", QUIET.fg(), cut(&card.about, inner)));
    }
    let mut work: Vec<_> = card
        .actions
        .iter()
        .map(|a| format!("{}{}{RESET}", TEXT.fg(), cut(a, inner)))
        .collect();
    let mut steps: Vec<_> = card
        .steps
        .iter()
        .flat_map(|s| std::iter::once((s, false)).chain(s.subtasks.iter().map(|sub| (sub, true))))
        .collect();
    let step_count = steps.len();
    if content.len() + work.len() + steps.len() > room {
        steps.sort_by_key(|(s, _)| s.mark == Mark::Done);
    }
    work.extend(steps.into_iter().map(|(s, sub)| {
        if sub {
            sub_row(s, inner)
        } else {
            row(s, inner)
        }
    }));
    let available = room.saturating_sub(content.len());
    let shown = if work.len() > available {
        available.saturating_sub(1)
    } else {
        work.len()
    };
    content.extend(work.iter().take(shown).cloned());
    if shown < work.len() && available > 0 {
        let omitted_steps = step_count.saturating_sub(shown.saturating_sub(card.actions.len()));
        let omitted_work = card.actions.len().saturating_sub(shown);
        let label = if omitted_work == 0 {
            format!("{omitted_steps} more steps")
        } else {
            format!("{omitted_steps} more steps · {omitted_work} work items")
        };
        content.push(format!("{}{}{RESET}", QUIET.fg(), cut(&label, inner)));
    }
    content.truncate(room);
    if !framed {
        return content;
    }
    let indent = " ".repeat(width.saturating_sub(panel) / 2);
    let rule = "─".repeat(panel - 2);
    let frame = FRAME.fg();
    let pad = " ".repeat(PAD);
    let mut lines = vec![format!("{indent}{frame}╭{rule}╮{RESET}")];
    for line in content {
        let fill = " ".repeat(inner.saturating_sub(len(&visible(&line))));
        lines.push(format!(
            "{indent}{frame}│{RESET}{pad}{line}{fill}{pad}{frame}│{RESET}"
        ));
    }
    lines.push(format!("{indent}{frame}╰{rule}╯{RESET}"));
    if height > lines.len() {
        lines.splice(
            0..0,
            std::iter::repeat_n(String::new(), (height - lines.len()) / 2),
        );
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
    let label = step.failed_check_hold.as_ref().map_or_else(
        || step.text.clone(),
        |hold| format!("{} — {}", step.text, hold.message),
    );
    if width < 5 {
        return cut(&label, width);
    }
    let text = cut(&label, width.saturating_sub(5));
    match step.mark {
        Mark::Done => format!(
            "{}  {}{text}{RESET}",
            tile(GREEN, INK, true, DONE),
            QUIET.fg()
        ),
        Mark::Started => format!(
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

/// One subtask: the same mark without its box, so it reads smaller.
fn sub_row(sub: &Step, width: usize) -> String {
    let lead = " ".repeat(5);
    let label = sub.failed_check_hold.as_ref().map_or_else(
        || sub.text.clone(),
        |hold| format!("{} — {}", sub.text, hold.message),
    );
    if width < 8 {
        return cut(&label, width);
    }
    let text = cut(&label, width.saturating_sub(8));
    let (mark, color, words) = match sub.mark {
        Mark::Done => (DONE, GREEN, QUIET),
        Mark::Started => (NOW, AMBER, AMBER),
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
    if width < len(&label) + 3 {
        return cut(&label, width);
    }
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
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if width == 0 {
        return String::new();
    }
    if len(&text) <= width {
        return text;
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

    fn from_plan(title: &str, plan: &Value) -> serde_json::Result<Card> {
        Card::from_view(
            title,
            &json!({"plan":plan, "work":"2 running · 1 waiting", "needs_you":"", "actions":[]}),
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
    fn outcomes_are_literal_and_unknown_replies_are_errors() {
        let outcome =
            "Compare red.md and blue.md: report differences; retain names (Rolf, 2026-09-25).";
        let plan = json!({"schema": 1, "revision": 4, "kind": "screen",
            "what_you_get": "A screen you open.", "goal": "A shorter goal",
            "does": outcome, "steps": []});
        let reply = json!({"data": {"result": {"plan":plan, "work":"2 running · 1 waiting", "needs_you":"", "actions":[]}}});
        let card = Card::from_view("Demo", &reply).unwrap();
        assert_eq!(card.about, outcome);
        assert!(screen(&card, 40).contains(&cut(outcome, 28)));
        for reply in [
            json!({}),
            json!({"data": {}}),
            json!({"steps": []}),
            json!({"schema": 1, "revision": 4, "steps": [{}]}),
            json!({"schema": 1, "revision": 4, "steps": "wrong"}),
        ] {
            assert!(Card::from_view("Demo", &reply).is_err(), "{reply}");
        }
        let old = json!({"schema": 1, "revision": 3, "kind": "screen",
            "what_you_get": "A screen you open.", "steps": []});
        assert_eq!(from_plan("Demo", &old).unwrap().about, "A screen you open.");
    }

    #[test]
    fn short_and_narrow_panes_keep_unfinished_work_counts_and_real_errors_visible() {
        let history: Vec<_> = (0..25)
            .map(|n| json!({"state":"done", "text":format!("Finished detail {n}")}))
            .collect();
        let plan = json!({"schema":1, "revision":1, "goal":"An outcome", "steps":[
            {"state":"done", "text":"Finished step", "subtasks":history},
            {"state":"running", "text":"Unfinished", "failed_check_hold":{"message":"held by failed check: permission denied"}},
        ]});
        let mut card = from_plan("Demo", &plan).unwrap();
        card.needs_you = "browser login".into();
        for width in [22, 40, 80] {
            let lines = render(&card, width, 12, "Read failed: permission denied");
            let text = lines
                .iter()
                .map(|l| visible(l))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(lines.len() <= 12);
            assert!(lines.iter().all(|l| len(&visible(l)) <= width));
            for required in [
                "Unfinished",
                "1 of 2",
                "Needs you:",
                "Read failed",
                "more steps",
            ] {
                assert!(text.contains(required), "{width}: {required}: {text}");
            }
        }
        for width in 1..22 {
            let lines = render(&card, width, 12, "Read failed: permission denied");
            assert!(lines.iter().all(|l| len(&visible(l)) <= width));
        }
        let empty = Card {
            title: "Demo".into(),
            about: String::new(),
            steps: vec![],
            work: String::new(),
            needs_you: String::new(),
            actions: vec![],
        };
        let lines = render(&empty, 22, 4, "Read failed: permission denied");
        assert!(lines.len() <= 4);
        assert!(lines.iter().all(|l| len(&visible(l)) <= 22));
        assert!(lines.iter().any(|l| visible(l).contains("Read failed")));
        let text = screen(&card, 80);
        assert!(text.contains("held by failed check: permission denied"));
        assert_eq!(card.count(Mark::Done), 1);
        assert_eq!(card.steps[0].subtasks.len(), 25);
    }

    #[test]
    fn literal_steps_and_subtasks_keep_plan_counts() {
        let plan = json!({"schema": 1, "revision": 3, "goal": "Compare red.md and blue.md", "steps": [
            {"id": "s-1", "state": "done", "text": "src/rundown/view.rs"},
            {"id": "s-2", "state": "left", "text": "t-0508", "subtasks": [
                {"id": "s-3", "state": "left", "text": "notes.md"},
                {"id": "s-4", "state": "left", "text": "job-0001"},
                {"id": "s-5", "state": "running", "text": "Compare red.md and blue.md"},
                {"id": "s-6", "state": "done", "text": "Fix src/pi/doctor.rs."},
                {"id": "s-7", "state": "left", "text": "Cut unused parts (Rolf, 2026-09-25)."},
                {"id": "s-8", "state": "left", "text": ""},
            ]},
        ]});
        let card = from_plan("Demo", &plan).unwrap();
        let steps = plan["steps"].as_array().unwrap();
        assert_eq!(card.steps.len(), steps.len());
        assert_eq!(
            card.count(Mark::Done),
            steps.iter().filter(|s| s["state"] == "done").count()
        );
        assert_eq!(card.steps[0].text, "src/rundown/view.rs");
        assert_eq!(card.steps[1].text, "t-0508");
        let subtasks = steps[1]["subtasks"].as_array().unwrap();
        assert_eq!(card.steps[1].subtasks.len(), subtasks.len());
        for (sub, stored) in card.steps[1].subtasks.iter().zip(subtasks) {
            assert_eq!(sub.text, stored["text"].as_str().unwrap());
        }
        assert_eq!(card.about, "Compare red.md and blue.md");
        let text = screen(&card, 80);
        for row in [
            "src/rundown/view.rs",
            "t-0508",
            "notes.md",
            "job-0001",
            "Compare red.md and blue.md",
            "Fix src/pi/doctor.rs.",
            "Cut unused parts (Rolf, 2026-09-25).",
            "1 of 2",
        ] {
            assert!(text.contains(row), "{row}: {text}");
        }
        let narrow = screen(&card, 40);
        assert!(narrow.contains(&cut("Compare red.md and blue.md", 20)));
        assert!(narrow.contains("1 of 2"));
    }
}
