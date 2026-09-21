//! Keyboard-first alternate screen. Drawing is pure over local snapshots;
//! only explicit input uses the existing request and answer operations.
use super::{
    overview::{self, Live, Overview, Row, Tone},
    theme::Theme,
    view::{self, Body, CardState, Conversation, JournalReader, Selection, Target},
};
use crate::{contracts::Ask, paths::Ctx, project::Project};
use anyhow::Result;
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Area {
    Overview,
    Chat,
    Combined,
    Questions,
    Full,
}
#[derive(Clone, Copy)]
enum Action {
    Overview,
    Focus,
    Question,
    Send,
    Clear,
    PageUp,
    PageDown,
    Home,
    End,
    Backspace,
    Left,
    Right,
    Exit,
}
const KEYS: &[(KeyCode, Action, &str, &str)] = &[
    (KeyCode::F(2), Action::Overview, "F2", "overview"),
    (KeyCode::F(6), Action::Focus, "F6", "scroll area"),
    (KeyCode::Tab, Action::Question, "Tab", "question"),
    (KeyCode::Enter, Action::Send, "Enter", "send"),
    (KeyCode::Esc, Action::Clear, "Esc", "clear"),
    (KeyCode::PageUp, Action::PageUp, "Page up", ""),
    (KeyCode::PageDown, Action::PageDown, "Page down", "scroll"),
    (KeyCode::Home, Action::Home, "Home", ""),
    (KeyCode::End, Action::End, "End", ""),
    (KeyCode::Backspace, Action::Backspace, "", ""),
    (KeyCode::Left, Action::Left, "", ""),
    (KeyCode::Right, Action::Right, "", ""),
];
fn hints(narrow: bool) -> String {
    KEYS.iter()
        .take(if narrow { 4 } else { 7 })
        .map(|(_, _, key, label)| {
            let label = if narrow && *key == "F6" {
                "area"
            } else {
                label
            };
            format!("{key} {label}").trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join(if narrow { " · " } else { " " })
}

#[derive(Default)]
struct Scroll {
    top: usize,
    height: usize,
    len: usize,
    follow: bool,
    new: bool,
    anchor: Option<(String, usize)>,
}
impl Scroll {
    fn position(&mut self, doc: &Document, height: usize) {
        self.height = height;
        self.len = doc.lines.len();
        if self.follow {
            self.top = self.max();
        } else if let Some((key, offset)) = &self.anchor
            && let Some(start) = doc.keys.iter().position(|k| k == key)
        {
            self.top = start + offset;
        }
        self.top = self.top.min(self.max());
        self.remember(doc);
    }
    fn max(&self) -> usize {
        self.len.saturating_sub(self.height)
    }
    fn remember(&mut self, doc: &Document) {
        self.anchor = doc.keys.get(self.top).map(|key| {
            let start = doc.keys.iter().position(|k| k == key).unwrap_or(self.top);
            (key.clone(), self.top - start)
        });
    }
    fn scroll(&mut self, action: Action) {
        self.top = match action {
            Action::Home => 0,
            Action::End => self.max(),
            Action::PageUp => self.top.saturating_sub(self.height.max(1)),
            Action::PageDown => self.top.saturating_add(self.height.max(1)).min(self.max()),
            _ => self.top,
        };
        self.anchor = None;
        self.follow = self.top == self.max();
        if self.follow {
            self.new = false;
        }
    }
}

#[derive(Default)]
struct Composer {
    text: String,
    cursor: usize,
}
impl Composer {
    fn insert(&mut self, text: &str) {
        // Paste is text, never a command key. Keep newlines as spaces in this
        // one-message composer and prevent terminal control injection.
        let text: String = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }
    fn left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .char_indices()
            .last()
            .map_or(0, |(i, _)| i);
    }
    fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }
    fn backspace(&mut self) {
        let old = self.cursor;
        self.left();
        self.text.drain(self.cursor..old);
    }
    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }
}

struct App {
    selection: Selection,
    composer: Composer,
    full: bool,
    narrow: bool,
    focus: Area,
    overview: Scroll,
    chat: Scroll,
    combined: Scroll,
    questions: Scroll,
    full_scroll: Scroll,
    regions: Vec<(Area, Rect)>,
    hits: Vec<(Rect, super::AnswerRef)>,
    pending: Option<Target>,
    reveal: bool,
    hint: Option<(String, Instant)>,
    chat_items: Option<usize>,
    /// After an Esc, the bytes of a split mouse report can arrive as plain
    /// characters; collect and drop them instead of typing them.
    escape_tail: Option<String>,
}
impl Default for App {
    fn default() -> Self {
        Self {
            selection: Selection::default(),
            composer: Composer::default(),
            full: false,
            narrow: false,
            focus: Area::Chat,
            overview: Scroll::default(),
            chat: Scroll {
                follow: true,
                ..Scroll::default()
            },
            combined: Scroll::default(),
            questions: Scroll::default(),
            full_scroll: Scroll::default(),
            regions: Vec::new(),
            hits: Vec::new(),
            pending: None,
            reveal: false,
            hint: None,
            chat_items: None,
            escape_tail: None,
        }
    }
}
impl App {
    fn scroll(&mut self, area: Area) -> &mut Scroll {
        match area {
            Area::Overview => &mut self.overview,
            Area::Chat => &mut self.chat,
            Area::Combined => &mut self.combined,
            Area::Questions => &mut self.questions,
            Area::Full => &mut self.full_scroll,
        }
    }
    fn refresh(&mut self, c: &Conversation) {
        if self.chat_items.is_some_and(|n| c.items.len() > n) {
            if !self.chat.follow {
                self.chat.new = true;
            }
            if !self.combined.follow {
                self.combined.new = true;
            }
        }
        self.chat_items = Some(c.items.len());
        let previous = self.selection.selected.clone();
        self.selection.refresh(&c.open);
        if previous != self.selection.selected {
            self.reveal = true;
        }
    }
    fn action(&mut self, action: Action, c: &Conversation) {
        match action {
            Action::Overview => {
                self.full = !self.full;
                self.focus = if self.full {
                    Area::Full
                } else if self.narrow {
                    Area::Combined
                } else {
                    Area::Chat
                };
            }
            Action::Focus => {
                let i = self
                    .regions
                    .iter()
                    .position(|(a, _)| *a == self.focus)
                    .unwrap_or(0);
                if !self.regions.is_empty() {
                    self.focus = self.regions[(i + 1) % self.regions.len()].0;
                }
            }
            Action::Question if !c.open.is_empty() => {
                self.selection.next(&c.open);
                self.full = false;
                self.reveal = true;
                self.focus = Area::Questions;
            }
            Action::Clear => self.composer.clear(),
            Action::Backspace => self.composer.backspace(),
            Action::Left => self.composer.left(),
            Action::Right => self.composer.right(),
            Action::Home | Action::End | Action::PageUp | Action::PageDown => {
                let area = self.focus;
                self.scroll(area).scroll(action);
            }
            _ => {}
        }
    }
    fn mouse(&mut self, event: MouseEvent) {
        let contains = |r: &Rect| {
            event.column >= r.x
                && event.column < r.right()
                && event.row >= r.y
                && event.row < r.bottom()
        };
        if let Some((area, _)) = self.regions.iter().find(|(_, r)| contains(r)) {
            self.focus = *area;
            if matches!(
                event.kind,
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            ) {
                let scroll = self.scroll(*area);
                scroll.top = if event.kind == MouseEventKind::ScrollUp {
                    scroll.top.saturating_sub(3)
                } else {
                    (scroll.top + 3).min(scroll.max())
                };
                scroll.anchor = None;
                scroll.follow = scroll.top == scroll.max();
                if scroll.follow {
                    scroll.new = false;
                }
            }
        }
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(reference) = self
                .hits
                .iter()
                .find(|(r, _)| contains(r))
                .map(|(_, reference)| reference.clone())
        {
            self.selection.selected = Some(reference);
            // A click can select a timeline copy while the pinned panel is
            // scrolled elsewhere. Reveal the same card before a digit can
            // become an answer target.
            self.reveal = true;
        }
    }
    fn key(&mut self, key: KeyEvent, c: &Conversation) -> Input {
        if key.kind == KeyEventKind::Release {
            return Input::None;
        }
        if let Some(mut tail) = self.escape_tail.take() {
            if let KeyCode::Char(ch) = key.code
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                tail.push(ch);
                if mouse_report(&tail) {
                    // A split SGR mouse report; swallow it whole.
                    return Input::None;
                }
                if mouse_report_prefix(&tail) {
                    self.escape_tail = Some(tail);
                    return Input::None;
                }
                // The Esc was a real clear; type what followed it.
                self.composer.insert(&tail);
                return Input::None;
            }
            // Any other key ends the collection and keeps the text.
            self.composer.insert(&tail);
        }
        let action =
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                Some(Action::Exit)
            } else {
                KEYS.iter()
                    .find(|(code, _, _, _)| *code == key.code)
                    .map(|(_, a, _, _)| *a)
            };
        if let Some(action) = action {
            match action {
                Action::Exit => return Input::Exit,
                Action::Send if !self.composer.text.is_empty() => {
                    return Input::Send(self.composer.text.clone());
                }
                _ => self.action(action, c),
            }
            if matches!(action, Action::Clear) {
                self.escape_tail = Some(String::new());
            }
        } else if let KeyCode::Char(ch) = key.code
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            if self.composer.text.is_empty()
                && let Some(target) = self.selection.drawn.clone()
                && let Some(n) = ch.to_digit(10)
            {
                if n as usize <= target.choices {
                    return Input::Answer(target, n);
                }
                self.hint = Some((
                    format!("this question has choices 0 to {}", target.choices),
                    Instant::now(),
                ));
            } else {
                self.composer.insert(&ch.to_string());
            }
        }
        Input::None
    }
}
enum Input {
    None,
    Send(String),
    Answer(Target, u32),
    Exit,
}

/// A complete SGR mouse report, e.g. `[<64;15;5M` or `[<64;15;5m`.
fn mouse_report(text: &str) -> bool {
    let Some(last) = text.chars().last() else {
        return false;
    };
    if last != 'M' && last != 'm' {
        return false;
    }
    let body = &text[..text.len() - last.len_utf8()];
    let Some(body) = body.strip_prefix("[<") else {
        return false;
    };
    !body.is_empty()
        && body
            .split(';')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// A prefix of the SGR mouse report grammar, `[`, `[<`, `[<64;` and so on.
fn mouse_report_prefix(text: &str) -> bool {
    match text.strip_prefix("[<") {
        Some(body) => body.chars().all(|c| c.is_ascii_digit() || c == ';'),
        None => text == "[",
    }
}

#[derive(Default)]
struct Document {
    lines: Vec<Line<'static>>,
    keys: Vec<String>,
}
impl Document {
    fn push(&mut self, key: &str, line: Line<'static>) {
        self.lines.push(line);
        self.keys.push(key.into());
    }
    fn prose(&mut self, key: &str, spans: Vec<Span<'static>>, width: usize, indent: usize) {
        for line in wrap(spans, width.saturating_sub(indent).max(1)) {
            let mut spans = vec![Span::raw(" ".repeat(indent))];
            spans.extend(line.spans);
            self.push(key, Line::from(spans));
        }
    }
    fn extend(&mut self, other: Document) {
        self.lines.extend(other.lines);
        self.keys.extend(other.keys);
    }
}
fn style(color: Color) -> Style {
    Style::default().fg(color)
}
fn span(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(text.into(), style(color))
}
fn bold(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(text.into(), style(color).add_modifier(Modifier::BOLD))
}
fn tone(t: Theme, tone: Tone) -> Color {
    match tone {
        Tone::Text => t.text,
        Tone::Heading | Tone::Accent => t.accent,
        Tone::Dim => t.overlay0,
        Tone::Green => t.green,
        Tone::Yellow => t.yellow,
        Tone::Red => t.red,
        Tone::Peach => t.peach,
    }
}

/// Word-boundary wrapping, with style preserved across spans. Only a word
/// longer than an entire available row is broken. Widths are terminal cells.
fn wrap(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut words: Vec<Vec<(char, Style)>> = Vec::new();
    let mut word = Vec::new();
    for s in spans {
        for ch in s.content.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            } else if !ch.is_control() {
                word.push((ch, s.style));
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut used = 0;
    for word in words {
        let size: usize = word.iter().map(|(c, _)| c.width().unwrap_or(0)).sum();
        if used > 0 && used + 1 + size > width {
            rows.push(Line::from(std::mem::take(&mut row)));
            used = 0;
        }
        if used > 0 {
            row.push(Span::raw(" "));
            used += 1;
        }
        for (ch, st) in word {
            let w = ch.width().unwrap_or(0);
            if used + w > width && used > 0 {
                rows.push(Line::from(std::mem::take(&mut row)));
                used = 0;
            }
            row.push(Span::styled(ch.to_string(), st));
            used += w;
        }
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(Line::from(row));
    }
    rows
}
/// Editing preserves spaces and maps the byte cursor to its wrapped cell.
fn composer_layout(input: &Composer, width: usize) -> (Vec<Line<'static>>, usize, usize) {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut col = 0;
    let mut caret = (0, 0);
    let chars: Vec<_> = input.text.char_indices().collect();
    for (index, &(byte, ch)) in chars.iter().enumerate() {
        if !ch.is_whitespace() && (index == 0 || chars[index - 1].1.is_whitespace()) {
            let word_width: usize = chars[index..]
                .iter()
                .take_while(|(_, c)| !c.is_whitespace())
                .map(|(_, c)| c.width().unwrap_or(0))
                .sum();
            if col > 0 && word_width <= width && col + word_width > width {
                rows.push(String::new());
                col = 0;
            }
        }
        let size = ch.width().unwrap_or(0);
        if col + size > width {
            rows.push(String::new());
            col = 0;
        }
        if byte == input.cursor {
            caret = (rows.len() - 1, col);
        }
        rows.last_mut().expect("one row").push(ch);
        col += size;
    }
    if input.cursor == input.text.len() {
        if col == width {
            rows.push(String::new());
            col = 0;
        }
        caret = (rows.len() - 1, col);
    }
    (rows.into_iter().map(Line::from).collect(), caret.0, caret.1)
}

fn overview_doc(o: &Overview, width: usize, narrow: bool, t: Theme, full: bool) -> Document {
    let mut d = Document::default();
    let mut section = |key: &str, heading: &str, rows: &[Row], progress| {
        overview_section(&mut d, key, heading, rows, progress, width, narrow, t, full)
    };
    if !o.stale.is_empty() {
        section("stale", overview::STALE_HEADING, &o.stale, None);
    }
    section("overview-0", overview::HEADINGS[0], &o.sections[0], None);
    section("overview-1", overview::HEADINGS[1], &o.sections[1], None);
    section(
        "overview-2",
        overview::HEADINGS[2],
        &o.sections[2],
        o.progress,
    );
    section("cost", overview::COST_HEADING, &o.cost, None);
    section("overview-3", overview::HEADINGS[3], &o.sections[3], None);
    section("overview-4", overview::HEADINGS[4], &o.sections[4], None);
    section("tasks", overview::TASKS_HEADING, &o.tasks, None);
    section("overview-5", overview::HEADINGS[5], &o.sections[5], None);
    section("overview-6", overview::HEADINGS[6], &o.sections[6], None);
    d
}

#[allow(clippy::too_many_arguments)]
fn overview_section(
    d: &mut Document,
    key: &str,
    heading: &str,
    rows: &[Row],
    progress: Option<(usize, usize)>,
    width: usize,
    narrow: bool,
    t: Theme,
    full: bool,
) {
    if full {
        d.prose(
            key,
            vec![bold(heading, t.accent)],
            width.saturating_sub(1),
            1,
        );
    } else {
        d.push(key, one_line("", heading, "", Tone::Heading, width, t));
    }
    if let Some((done, total)) = progress {
        let cells = if narrow { 10 } else { 20 };
        if full {
            let mut spans = vec![
                span(format!("{done} of {total} done"), t.green),
                Span::raw("  "),
            ];
            spans.extend(progress_spans(done, total, cells, t));
            d.prose("progress", spans, width.saturating_sub(1), 1);
        } else {
            let label = format!("{done} of {total} done");
            let available_cells = width.saturating_sub(1 + label.width() + 2).min(cells);
            let mut spans = vec![Span::raw(" "), span(label, t.green)];
            if available_cells > 0 {
                spans.push(Span::raw("  "));
                spans.extend(progress_spans(done, total, available_cells, t));
            }
            d.push("progress", Line::from(spans));
        }
    }
    for (i, row) in rows.iter().enumerate() {
        if full {
            let spans = if row.tone == Tone::Heading {
                vec![bold(row.full_text(), t.accent)]
            } else {
                let mut spans = Vec::new();
                if !row.prefix.is_empty() {
                    spans.push(span(format!("{} ", row.prefix), tone(t, row.tone)));
                }
                spans.push(span(row.text.clone(), t.text));
                if !row.marker.is_empty() {
                    spans.push(span(format!(" {}", row.marker), t.overlay0));
                }
                spans
            };
            d.prose(&format!("{key}-{i}"), spans, width.saturating_sub(1), 1);
        } else {
            d.push(
                &format!("{key}-{i}"),
                one_line(&row.prefix, &row.text, &row.marker, row.tone, width, t),
            );
        }
    }
}

fn progress_spans(done: usize, total: usize, cells: usize, t: Theme) -> Vec<Span<'static>> {
    let filled = cells * done / total;
    vec![
        span("█".repeat(filled), t.green),
        span("░".repeat(cells - filled), t.surface1),
    ]
}

/// One compact overview row as exactly one terminal line: the one-cell
/// indent, the row's prefix and right marker, and the text cut to the cells
/// left between them. The full overview keeps wrapping the whole text.
fn one_line(
    prefix: &str,
    text: &str,
    marker: &str,
    row_tone: Tone,
    width: usize,
    t: Theme,
) -> Line<'static> {
    let avail = width.saturating_sub(1).max(1);
    if row_tone == Tone::Heading {
        return Line::from(vec![Span::raw(" "), bold(clip(text, avail), t.accent)]);
    }
    let lead = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix} ")
    };
    let marker_room = avail
        .saturating_sub(lead.width())
        .saturating_sub(usize::from(!text.is_empty()));
    let marker = if marker.is_empty() || marker_room <= 1 {
        String::new()
    } else {
        format!(" {}", clip(marker, marker_room - 1))
    };
    let text_budget = avail.saturating_sub(lead.width() + marker.width());
    let mut spans = vec![Span::raw(" ")];
    if !lead.is_empty() {
        spans.push(span(lead, tone(t, row_tone)));
    }
    spans.push(span(clip(text, text_budget), t.text));
    if !marker.is_empty() {
        spans.push(span(marker, t.overlay0));
    }
    Line::from(spans)
}

/// Cut `text` to at most `max` terminal cells. When it does not fit, the
/// last whole word shown is followed by one ellipsis; a word wider than the
/// whole row is cut at the cell boundary. Whitespace collapses.
fn clip(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.into();
    }
    let Some(budget) = max.checked_sub(1) else {
        return String::new();
    };
    let mut out = String::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        let gap = usize::from(used > 0);
        if used + gap + word.width() > budget {
            if used == 0 {
                for ch in word.chars() {
                    let cw = ch.width().unwrap_or(0);
                    if used + cw > budget {
                        break;
                    }
                    out.push(ch);
                    used += cw;
                }
            }
            break;
        }
        if gap == 1 {
            out.push(' ');
        }
        out.push_str(word);
        used += gap + word.width();
    }
    out.push('…');
    out
}
fn delivery_color(state: crate::contracts::TalkRequestState, t: Theme) -> Color {
    use crate::contracts::TalkRequestState::*;
    match state {
        Queued => t.yellow,
        Submitted => t.subtext0,
        Uncertain => t.peach,
        Accepted => t.green,
    }
}
fn label(
    d: &mut Document,
    key: &str,
    at: &str,
    who: &str,
    state: Option<crate::contracts::TalkRequestState>,
    width: usize,
    appearance: (Color, Theme),
) {
    let (color, t) = appearance;
    let text = format!("{} {who}", view::clock(at));
    let mut spans = vec![span(text.clone(), color)];
    if let Some(state) = state {
        let status = view::delivery(state);
        let pad = width.saturating_sub(text.width() + status.width()).max(1);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(span(status, delivery_color(state, t)));
    }
    // Keep right alignment if it fits; on the tiniest rows wrap rather than overflow.
    if spans.iter().map(|s| s.width()).sum::<usize>() <= width {
        d.push(key, Line::from(spans));
    } else {
        d.prose(key, spans, width, 0);
    }
}
fn card(a: &Ask, selected: bool, compact: bool, width: usize, t: Theme) -> Document {
    let mut d = Document::default();
    let key = format!("ask-{}-{}", a.id, a.revision);
    let width = width.clamp(4, 80);
    let inner = width - 4;
    let border = if selected { t.accent } else { t.surface1 };
    d.push(
        &key,
        Line::from(span(format!("╭{}╮", "─".repeat(width - 2)), border)),
    );
    let mut content = Document::default();
    if !compact {
        for text in [&a.what, &a.means].into_iter().flatten() {
            content.prose(&key, vec![span(text.clone(), t.subtext0)], inner, 0);
        }
    }
    content.prose(
        &key,
        vec![bold(
            format!(
                "{}{}",
                if selected && compact { "▸ " } else { "" },
                a.question
            ),
            t.text,
        )],
        inner,
        0,
    );
    for (i, text) in a.choices.iter().enumerate() {
        content.prose(
            &key,
            vec![
                span(format!("{} ", i + 1), t.accent),
                span(text.clone(), t.text),
            ],
            inner,
            0,
        );
    }
    content.prose(
        &key,
        vec![
            span("0 ", t.peach),
            span(crate::ask::NOT_UNDERSTOOD, t.text),
        ],
        inner,
        0,
    );
    if compact {
        content.prose(
            &key,
            vec![span(format!("asked {}", view::clock(&a.asked)), t.overlay0)],
            inner,
            0,
        );
    }
    for line in content.lines {
        let used = line.width();
        let mut spans = vec![span("│ ", border)];
        spans.extend(line.spans);
        spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
        spans.push(span(" │", border));
        d.push(&key, Line::from(spans));
    }
    d.push(
        &key,
        Line::from(span(format!("╰{}╯", "─".repeat(width - 2)), border)),
    );
    d
}
fn timeline(
    c: &Conversation,
    selected: &Selection,
    width: usize,
    narrow: bool,
    t: Theme,
) -> Document {
    let mut d = Document::default();
    let mut day = String::new();
    let indent = if narrow { 0 } else { 7 };
    for (i, item) in c.items.iter().enumerate() {
        let key = format!("chat-{i}");
        let next = view::date(&item.at);
        if !next.is_empty() && day != next {
            d.prose(
                &key,
                vec![span(format!("── {next} ──"), t.overlay0)],
                width,
                0,
            );
            day = next;
        }
        match &item.body {
            Body::Say { what, means } => {
                label(
                    &mut d,
                    &key,
                    &item.at,
                    "coordinator",
                    None,
                    width,
                    (t.subtext0, t),
                );
                d.prose(&key, vec![bold(what.clone(), t.text)], width, indent);
                if let Some(means) = means {
                    d.prose(
                        &key,
                        vec![span(format!("for you: {means}"), t.subtext0)],
                        width,
                        indent,
                    );
                }
            }
            Body::Rolf(text) => {
                label(
                    &mut d,
                    &key,
                    &item.at,
                    "you",
                    item.delivery,
                    width,
                    (t.accent, t),
                );
                d.prose(&key, vec![span(text.clone(), t.accent)], width, indent);
            }
            Body::Notice(text) => {
                d.prose(
                    &key,
                    vec![span("! ", t.peach), span(text.clone(), t.subtext0)],
                    width,
                    indent,
                );
            }
            Body::Card {
                ask,
                state: CardState::Open,
            } => {
                label(
                    &mut d,
                    &key,
                    &item.at,
                    "question",
                    None,
                    width,
                    (t.subtext0, t),
                );
                let inner = card(
                    ask,
                    selected.matches(ask),
                    false,
                    width.saturating_sub(indent),
                    t,
                );
                for (line, card_key) in inner.lines.into_iter().zip(inner.keys) {
                    let mut spans = vec![Span::raw(" ".repeat(indent))];
                    spans.extend(line.spans);
                    d.push(&card_key, Line::from(spans));
                }
            }
            Body::Card {
                state: CardState::Answered { choice, text },
                ..
            } => {
                let color = if *choice == 0 { t.peach } else { t.accent };
                label(
                    &mut d,
                    &key,
                    &item.at,
                    &format!("you chose {choice}"),
                    item.delivery,
                    width,
                    (color, t),
                );
                d.prose(&key, vec![span(text.clone(), color)], width, indent);
            }
            Body::Card {
                state: CardState::AskedAgain,
                ..
            } => d.prose(
                &key,
                vec![span(
                    format!("{} question asked again below", view::clock(&item.at)),
                    t.overlay0,
                )],
                width,
                0,
            ),
        }
        d.push(&key, Line::default());
    }
    d
}

fn card_hits(app: &mut App, doc: &Document, rect: Rect, area: Area, c: &Conversation) {
    let top = app.scroll(area).top;
    for (row, key) in doc
        .keys
        .iter()
        .skip(top)
        .take(rect.height as usize)
        .enumerate()
    {
        for a in &c.open {
            if key == &format!("ask-{}-{}", a.id, a.revision) {
                app.hits.push((
                    Rect::new(rect.x, rect.y + row as u16, rect.width, 1),
                    super::AnswerRef {
                        id: a.id.clone(),
                        revision: a.revision,
                    },
                ));
            }
        }
    }
}

fn viewport(
    f: &mut Frame,
    rect: Rect,
    doc: &Document,
    scroll: &mut Scroll,
    t: Theme,
    markers: bool,
) {
    scroll.position(doc, rect.height as usize);
    let lines = doc
        .lines
        .iter()
        .skip(scroll.top)
        .take(rect.height as usize)
        .cloned()
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(lines), rect);
    if markers && rect.height > 1 {
        if scroll.top > 0 {
            edge(f, rect, "more above", false, t.overlay0);
        }
        if scroll.top < scroll.max() {
            edge(f, rect, "more below", true, t.overlay0);
        }
    }
}
fn edge(f: &mut Frame, rect: Rect, text: &str, bottom: bool, color: Color) {
    let w = text.width().min(rect.width as usize) as u16;
    if rect.height > 0 {
        f.render_widget(
            Paragraph::new(span(text, color)),
            Rect::new(
                rect.right() - w,
                if bottom { rect.bottom() - 1 } else { rect.y },
                w,
                1,
            ),
        );
    }
}
fn separator(f: &mut Frame, rect: Rect, t: Theme) {
    f.render_widget(
        Paragraph::new(span("─".repeat(rect.width as usize), t.surface1)),
        rect,
    );
}
fn shorten(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.into();
    }
    let mut out = String::new();
    for word in text.split_whitespace() {
        if out.width() + word.width() + 2 > max {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.push('…');
    out
}
fn header(
    f: &mut Frame,
    rect: Rect,
    o: &Overview,
    state: &str,
    c: &Conversation,
    capability: &str,
    t: Theme,
) {
    let narrow = rect.height == 2;
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(t.panel_bg)),
        rect,
    );
    let state_color = match state {
        "working" => t.yellow,
        "idle" | "done" => t.green,
        "other tab" => t.peach,
        _ => t.red,
    };
    let mut counts = Vec::new();
    if o.active > 0 {
        counts.push(format!("{} open tasks", o.active));
    }
    if o.needs > 0 {
        counts.push(format!("{} need you", o.needs));
    }
    let time = format!("chat {}", view::clock(&c.latest_at));
    let mut suffix = counts.join(" · ");
    if narrow {
        let name = shorten(
            &o.name,
            (rect.width as usize).saturating_sub(state.width() + 5),
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![
                bold(name, t.accent),
                span(" · ● ", state_color),
                span(state, t.subtext0),
            ])),
            Rect { height: 1, ..rect },
        );
        if !suffix.is_empty() {
            suffix.push_str(" · ");
        }
        suffix.push_str(&time);
        f.render_widget(
            Paragraph::new(span(suffix, t.subtext0)),
            Rect::new(rect.x, rect.y + 1, rect.width, 1),
        );
    } else {
        let base = format!(" · ● {state}");
        let need = if o.needs > 0 {
            format!(" · {} need you", o.needs)
        } else {
            String::new()
        };
        let mut tasks = if o.active > 0 {
            format!(" · {} open tasks", o.active)
        } else {
            String::new()
        };
        let mut capability = format!(" · {capability}");
        let mut clock = format!("  {time}");
        // Optional fields disappear in a fixed order; counts are work, not processes.
        for drop in 0..3 {
            if o.name.width()
                + base.width()
                + tasks.width()
                + need.width()
                + capability.width()
                + clock.width()
                <= rect.width as usize
            {
                break;
            }
            match drop {
                0 => capability.clear(),
                1 => clock.clear(),
                _ => tasks.clear(),
            }
        }
        let reserved =
            base.width() + tasks.width() + need.width() + capability.width() + clock.width();
        let name = shorten(&o.name, (rect.width as usize).saturating_sub(reserved));
        let pad = if clock.is_empty() {
            0
        } else {
            (rect.width as usize).saturating_sub(name.width() + reserved)
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                bold(name, t.accent),
                span(" · ● ", state_color),
                span(state, t.subtext0),
                span(tasks, t.text),
                span(need, t.accent),
                span(capability, t.overlay0),
                Span::raw(" ".repeat(pad)),
                span(clock, t.overlay0),
            ])),
            rect,
        );
    }
}

fn draw(
    f: &mut Frame,
    app: &mut App,
    o: &Overview,
    c: &Conversation,
    state: &str,
    capability: &str,
    t: Theme,
) {
    let area = f.area();
    let w = area.width;
    let h = area.height;
    f.render_widget(
        Paragraph::new("").style(Style::default().fg(t.text).bg(Color::Reset)),
        area,
    );
    app.regions.clear();
    app.hits.clear();
    app.pending = None;
    let cw = w.saturating_sub(2).max(1) as usize;
    let (composer_rows, cursor_row, cursor_col) = composer_layout(&app.composer, cw);
    let ch = composer_rows.len().clamp(1, 3) as u16;
    let kh = u16::from(w >= 40);
    let initial_body = h.saturating_sub(1 + ch + kh + 1);
    let narrow = w < 80 || initial_body < 16;
    if narrow != app.narrow && !app.full {
        if narrow {
            let source = if app.focus == Area::Overview {
                &app.overview
            } else {
                &app.chat
            };
            if source.anchor.is_some() {
                app.combined.anchor = source.anchor.clone();
                app.combined.follow = app.focus == Area::Chat && source.follow;
            }
            if app.focus != Area::Questions {
                app.focus = Area::Combined;
            }
        } else if let Some((key, _)) = &app.combined.anchor {
            let is_overview = key.starts_with("overview") || key == "progress";
            let target = if is_overview {
                &mut app.overview
            } else {
                &mut app.chat
            };
            target.anchor = app.combined.anchor.clone();
            target.follow = !is_overview && app.combined.follow;
            if app.focus != Area::Questions {
                app.focus = if is_overview {
                    Area::Overview
                } else {
                    Area::Chat
                };
            }
        }
    }
    app.narrow = narrow;
    let hh = if narrow { 2 } else { 1 };
    let body_height = h.saturating_sub(hh + ch + kh + 1);
    let body = Rect::new(area.x, area.y + hh.min(h), w, body_height);
    let composer = Rect::new(area.x, area.y + h.saturating_sub(ch + kh), w, ch.min(h));
    if w < 20 || h < 10 {
        f.render_widget(
            Paragraph::new(span("Make this view larger.", t.text)),
            Rect {
                height: h.saturating_sub(ch + kh),
                ..area
            },
        );
    } else {
        header(f, Rect { height: hh, ..area }, o, state, c, capability, t);
        let odoc = overview_doc(o, w as usize, narrow, t, app.full);
        if app.full {
            app.regions.push((Area::Full, body));
            viewport(f, body, &odoc, &mut app.full_scroll, t, true);
        } else if narrow {
            let selected = c.open.iter().find(|a| app.selection.matches(a));
            let qdoc = selected.map(|a| card(a, true, true, w as usize, t));
            let qh = qdoc
                .as_ref()
                .map_or(0, |d| (d.lines.len() as u16 + 1).min(body.height / 2));
            let combined = Rect {
                height: body.height.saturating_sub(qh),
                ..body
            };
            let mut doc = odoc;
            doc.push("chat-start", Line::from(span("── chat ──", t.surface1)));
            doc.extend(timeline(c, &app.selection, w as usize, true, t));
            app.regions.push((Area::Combined, combined));
            viewport(f, combined, &doc, &mut app.combined, t, false);
            card_hits(app, &doc, combined, Area::Combined, c);
            if app.combined.new && app.combined.top < app.combined.max() {
                edge(f, combined, "↓ new", true, t.accent);
            } else if app.combined.top == app.combined.max() {
                app.combined.new = false;
            }
            if let (Some(a), Some(qdoc)) = (selected, qdoc) {
                let qr = Rect::new(body.x, combined.bottom(), w, qh);
                let n = c
                    .open
                    .iter()
                    .position(|ask| ask.id == a.id && ask.revision == a.revision)
                    .unwrap_or(0)
                    + 1;
                f.render_widget(
                    Paragraph::new(bold(
                        format!("needs you · {n} of {}", c.open.len()),
                        t.accent,
                    )),
                    Rect { height: 1, ..qr },
                );
                let content = Rect::new(qr.x, qr.y + 1, qr.width, qr.height.saturating_sub(1));
                if app.reveal {
                    app.questions.top = 0;
                    app.questions.anchor = None;
                    app.questions.follow = false;
                }
                app.regions.push((Area::Questions, content));
                viewport(f, content, &qdoc, &mut app.questions, t, true);
                card_hits(app, &qdoc, content, Area::Questions, c);
                if content.height >= 3 {
                    app.pending = Some(Target {
                        id: a.id.clone(),
                        revision: a.revision,
                        choices: a.choices.len(),
                    });
                }
            }
        } else {
            // Give the selected question card enough room to read in full; the
            // overview keeps its own scroll position. Never starve the chat.
            let selected = c.open.iter().find(|a| app.selection.matches(a));
            let oh = match selected {
                Some(a) => {
                    let card_h = card(a, false, true, 36, t).lines.len() as u16;
                    body.height.saturating_sub(card_h + 2).clamp(6, 26)
                }
                None => body.height.saturating_sub(9).min(26),
            };
            let or = Rect { height: oh, ..body };
            app.regions.push((Area::Overview, or));
            viewport(f, or, &odoc, &mut app.overview, t, true);
            separator(f, Rect::new(body.x, or.bottom(), w, 1), t);
            let cr = Rect::new(
                body.x,
                or.bottom() + 1,
                if c.open.is_empty() { w } else { w - 37 },
                body.height - oh - 1,
            );
            let doc = timeline(c, &app.selection, cr.width as usize, false, t);
            app.regions.push((Area::Chat, cr));
            viewport(f, cr, &doc, &mut app.chat, t, false);
            card_hits(app, &doc, cr, Area::Chat, c);
            if app.chat.new && app.chat.top < app.chat.max() {
                edge(f, cr, "↓ new", true, t.accent);
            } else if app.chat.top == app.chat.max() {
                app.chat.new = false;
            }
            if !c.open.is_empty() {
                let divider = Rect::new(cr.right(), cr.y, 1, cr.height);
                f.render_widget(
                    Paragraph::new(vec![
                        Line::from(span("│", t.surface1));
                        cr.height as usize
                    ]),
                    divider,
                );
                let qr = Rect::new(cr.right() + 1, cr.y, 36, cr.height);
                f.render_widget(
                    Paragraph::new(bold("needs you", t.accent)),
                    Rect { height: 1, ..qr },
                );
                let content = Rect::new(qr.x, qr.y + 1, qr.width, qr.height.saturating_sub(1));
                let mut doc = Document::default();
                let mut selected_range = None;
                let mut first = true;
                for a in &c.open {
                    if !first {
                        doc.push("question-gap", Line::default());
                    }
                    first = false;
                    let start = doc.lines.len();
                    doc.extend(card(a, app.selection.matches(a), true, 36, t));
                    if app.selection.matches(a) {
                        selected_range = Some((start, doc.lines.len(), a));
                    }
                }
                if app.reveal
                    && let Some((start, _, _)) = selected_range
                {
                    app.questions.top = start;
                    app.questions.anchor = None;
                    app.questions.follow = false;
                }
                app.regions.push((Area::Questions, content));
                viewport(f, content, &doc, &mut app.questions, t, true);
                card_hits(app, &doc, content, Area::Questions, c);
                if let Some((start, end, a)) = selected_range
                    && end
                        .min(app.questions.top + content.height as usize)
                        .saturating_sub(start.max(app.questions.top))
                        >= 3
                {
                    app.pending = Some(Target {
                        id: a.id.clone(),
                        revision: a.revision,
                        choices: a.choices.len(),
                    });
                }
            }
        }
        app.reveal = false;
        if !app.regions.iter().any(|(a, _)| *a == app.focus) {
            app.focus = app.regions.first().map_or(Area::Combined, |(a, _)| *a);
        }
        if composer.y > area.y {
            separator(f, Rect::new(area.x, composer.y - 1, w, 1), t);
        }
    }
    let placeholder = match &app.pending {
        Some(target) if narrow => format!("type, or 0 to {} answers", target.choices),
        Some(target) => format!(
            "type a message · 0 to {} answers the selected question",
            target.choices
        ),
        None => "type a message".into(),
    };
    if composer.height > 0 {
        f.render_widget(
            Paragraph::new(span(">", t.accent)),
            Rect {
                width: 1.min(w),
                ..composer
            },
        );
        let input = Rect::new(
            composer.x + 2.min(w),
            composer.y,
            w.saturating_sub(2),
            composer.height,
        );
        let start = cursor_row.saturating_sub(ch as usize - 1);
        if app.composer.text.is_empty() {
            f.render_widget(Paragraph::new(span(placeholder, t.overlay0)), input);
        } else {
            f.render_widget(
                Paragraph::new(
                    composer_rows
                        .into_iter()
                        .skip(start)
                        .take(ch as usize)
                        .collect::<Vec<_>>(),
                ),
                input,
            );
        }
        if input.width > 0 {
            let col = cursor_col.min(input.width as usize - 1);
            f.set_cursor_position((
                input.x + col as u16,
                input.y + (cursor_row - start).min(input.height.saturating_sub(1) as usize) as u16,
            ));
        }
    }
    if kh > 0 && h > 0 {
        let text = app
            .hint
            .as_ref()
            .filter(|(_, when)| when.elapsed() < Duration::from_secs(2))
            .map(|(s, _)| s.clone())
            .unwrap_or_else(|| hints(narrow));
        f.render_widget(
            Paragraph::new(span(text, t.overlay0)),
            Rect::new(area.x, area.bottom() - 1, w, 1),
        );
    }
}

/// Constructed before the first terminal mutation, so partial setup, IO
/// errors, explicit exit and unwinding all run exactly the same cleanup.
struct Cleanup<F: FnMut()>(F);
impl<F: FnMut()> Drop for Cleanup<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}
fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        event::DisableMouseCapture,
        DisableBracketedPaste,
        SetCursorStyle::DefaultUserShape,
        Show,
        LeaveAlternateScreen
    );
    let _ = io::stdout().flush();
}
fn guarded<T>(cleanup: impl FnMut(), run: impl FnOnce() -> Result<T>) -> Result<T> {
    let _guard = Cleanup(cleanup);
    run()
}

/// True when the installed build differs from the running one.
fn screen_version_behind(installed: Option<&str>) -> bool {
    installed.is_some_and(|version| version != crate::VERSION)
}

/// Hand over to the installed program when it is a different build. The
/// terminal, screen and the composer draft are inherited by the new process.
fn reexec_if_stale(ctx: &Ctx, draft: &str) {
    if std::env::var_os("HERDR_TALK_REEXEC").is_some() {
        return;
    }
    let installed = ctx.env.home.join(".local/bin/herdr-ade");
    let same = std::fs::canonicalize(&installed)
        .ok()
        .zip(std::env::current_exe().ok())
        .is_some_and(|(a, b)| a == b);
    if same {
        return;
    }
    let version = super::stale::installed_version(ctx);
    if !screen_version_behind(version.as_deref()) {
        return;
    }
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new(&installed)
        .args(std::env::args_os().skip(1))
        .env("HERDR_TALK_DRAFT", draft)
        .env("HERDR_TALK_REEXEC", "1")
        .exec();
    eprintln!("herdr-ade: could not restart the project screen: {error}");
}

pub(crate) fn run(ctx: &Ctx, slug: &str) -> Result<()> {
    // A stale screen replaces itself before it paints, so an install never
    // leaves an old copy running in the tab.
    reexec_if_stale(ctx, "");
    let project = Project::load(&ctx.root, slug)?;
    guarded(restore, || {
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            event::EnableMouseCapture,
            SetCursorStyle::SteadyBlock
        )?;
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        let theme = Theme::load(ctx.env);
        let mut app = App::default();
        if let Ok(draft) = std::env::var("HERDR_TALK_DRAFT") {
            app.composer.insert(&draft);
        }
        let mut journal = JournalReader::default();
        let mut live = Live::default();
        let mut poll = Instant::now() - Duration::from_secs(3);
        let mut slow = Instant::now() - Duration::from_secs(30);
        let capability =
            crate::adapters::capability_label(&project, &super::coordinator_kind(&project));
        let capability = if capability.starts_with("capability: unqualified") {
            "say and ask only"
        } else {
            "checked after reply"
        };
        loop {
            if poll.elapsed() >= Duration::from_secs(3) {
                live.poll(ctx, &project);
                poll = Instant::now();
            }
            if slow.elapsed() >= Duration::from_secs(30) {
                live.refresh_slow(ctx, &project);
                slow = Instant::now();
                reexec_if_stale(ctx, &app.composer.text);
            }
            journal.refresh(&project)?;
            let mut conversation = Conversation::load(&project, &journal.journal);
            if !live.reachable
                && let Some(text) = crate::ask::notice_text("request_waiting")
            {
                conversation.items.push(view::Item {
                    at: String::new(),
                    body: Body::Notice(text.into()),
                    delivery: None,
                });
            }
            app.refresh(&conversation);
            let overview = Overview::load(&project, &journal.journal, &conversation, &live);
            terminal.draw(|f| {
                draw(
                    f,
                    &mut app,
                    &overview,
                    &conversation,
                    live.state(&project),
                    capability,
                    theme,
                )
            })?;
            app.selection.drawn = app.pending.clone();
            if event::poll(Duration::from_millis(250))? {
                let input = match event::read()? {
                    Event::Key(key) => app.key(key, &conversation),
                    Event::Mouse(event) => {
                        app.mouse(event);
                        Input::None
                    }
                    Event::Paste(text) => {
                        app.composer.insert(&text);
                        Input::None
                    }
                    _ => Input::None,
                };
                match input {
                    Input::Exit => break,
                    Input::Send(text) => match super::handle(ctx, &project, &text) {
                        Ok(()) => app.composer.clear(),
                        Err(_) => {
                            app.hint =
                                Some(("I could not send this message.".into(), Instant::now()))
                        }
                    },
                    Input::Answer(target, choice) => {
                        if super::answer(ctx, &project, &target, choice).is_err() {
                            app.hint =
                                Some(("I could not send this answer.".into(), Instant::now()));
                        }
                        // Until a new frame commits there is no answer target.
                        app.selection.drawn = None;
                    }
                    Input::None => {}
                }
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::testkit::fixture;
    use ratatui::backend::TestBackend;
    fn capture(width: u16, height: u16, app: &mut App, o: &Overview, c: &Conversation) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| {
                draw(
                    f,
                    app,
                    o,
                    c,
                    "working",
                    "say and ask only",
                    Theme::default(),
                )
            })
            .unwrap();
        app.selection.drawn = app.pending.clone();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn cold_layouts_and_full_overview_keep_goal_first_and_all_work_reachable() {
        let fx = fixture();
        for n in 0..30 {
            crate::thread::allocate(&fx.project, |t| {
                t.plain = format!("Show task number {n}.");
                t.last_group = "working".into();
            })
            .unwrap();
        }
        let c = Conversation::default();
        let j = super::super::read(&fx.project);
        let o = Overview::load(&fx.project, &j, &c, &Live::default());
        for (w, h) in [(120, 40), (80, 40), (60, 54), (58, 54)] {
            let mut app = App::default();
            let text = capture(w, h, &mut app, &o, &c);
            assert!(
                text.contains("Goal") && text.contains(overview::EMPTY[0]),
                "{w}: {text}"
            );
            assert_eq!(
                if app.narrow {
                    app.combined.top
                } else {
                    app.overview.top
                },
                0
            );
            app.focus = if app.narrow {
                Area::Combined
            } else {
                Area::Overview
            };
            app.action(Action::End, &c);
            let normal = capture(w, h, &mut app, &o, &c);
            assert!(normal.contains("Show task number 29."), "{normal}");
            app.full = true;
            app.focus = Area::Full;
            capture(w, h, &mut app, &o, &c);
            app.action(Action::End, &c);
            let text = capture(w, h, &mut app, &o, &c);
            assert!(text.contains("Show task number 29."), "{text}");
            assert!(text.contains("Decided for you"));
        }
    }
    #[test]
    fn a_long_overview_row_is_one_cut_line_but_complete_in_the_full_overview() {
        let long = "The goal sentence runs on well past the width of one overview line.";
        let mut o = Overview::default();
        o.sections[0].push(overview::Row {
            text: long.into(),
            prefix: String::new(),
            marker: String::new(),
            tone: Tone::Text,
        });
        o.sections[3].push(overview::Row {
            text: long.into(),
            prefix: "checking".into(),
            marker: "box last seen".into(),
            tone: Tone::Yellow,
        });
        o.sections[6].push(overview::Row {
            text: "The work check could not start (2 times).".into(),
            prefix: String::new(),
            marker: "f-0001".into(),
            tone: Tone::Yellow,
        });
        o.progress = Some((2, 5));
        let lines = |d: &Document| -> Vec<String> {
            d.lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect()
        };
        let compact = lines(&overview_doc(&o, 40, true, Theme::default(), false));
        assert_eq!(
            compact
                .iter()
                .filter(|line| line.starts_with(" The goal"))
                .count(),
            1,
            "{compact:?}"
        );
        let row = compact
            .iter()
            .find(|line| line.starts_with(" The goal"))
            .unwrap();
        assert!(row.ends_with('…'), "{row:?}");
        assert!(row.width() <= 40, "{row:?}");
        assert!(compact.iter().any(|line| line.contains("Failures")));
        assert!(compact.iter().any(|line| line.ends_with("f-0001")));
        let marked = compact
            .iter()
            .find(|line| line.contains("checking"))
            .unwrap();
        assert!(
            marked.contains('…') && marked.ends_with("box last seen"),
            "{marked:?}"
        );
        assert!(
            compact.iter().all(|line| line.width() <= 40)
                && !compact.iter().any(|line| line.contains("overview")),
            "{compact:?}"
        );
        let tiny = lines(&overview_doc(&o, 20, true, Theme::default(), false));
        assert!(tiny.iter().all(|line| line.width() <= 20), "{tiny:?}");
        assert_eq!(
            tiny.iter()
                .filter(|line| line.contains("2 of 5 done"))
                .count(),
            1
        );
        let full = lines(&overview_doc(&o, 40, true, Theme::default(), true));
        let full = full.join(" ");
        assert!(full.contains("overview line."), "{full:?}");
        assert!(full.contains("box last seen"), "{full:?}");
    }
    #[test]
    fn numeric_binding_hidden_cards_typing_paste_and_invalid_choice() {
        let fx = fixture();
        let a = crate::ask::ask(
            &fx.world.ctx(),
            "demo",
            crate::ask::NewAsk {
                question: "May I spend five dollars on this check?".into(),
                choices: vec![
                    "Keep it running.".into(),
                    "Stop it now.".into(),
                    "Wait for me.".into(),
                ],
                what: None,
                means: None,
                round: None,
                reask: None,
            },
        )
        .unwrap();
        let j = super::super::read(&fx.project);
        let c = Conversation::load(&fx.project, &j);
        let o = Overview::load(&fx.project, &j, &c, &Live::default());
        let mut app = App::default();
        app.refresh(&c);
        let text = capture(60, 54, &mut app, &o, &c);
        assert!(text.contains("0 to 3 answers"));
        let key = |ch| KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
        assert!(matches!(app.key(key('4'), &c), Input::None));
        assert!(app.hint.as_ref().unwrap().0.ends_with('3'));
        assert!(matches!(app.key(key('3'),&c),Input::Answer(t,3) if t.id == a.id));
        app.composer.insert(" ");
        assert!(matches!(app.key(key('1'), &c), Input::None));
        assert_eq!(app.composer.text, " 1");
        app.composer.clear();
        app.composer.insert("2");
        assert_eq!(app.composer.text, "2");
        app.composer.clear();
        app.action(Action::Overview, &c);
        let text = capture(60, 54, &mut app, &o, &c);
        assert!(!text.contains("0 to 3 answers"));
        app.key(key('2'), &c);
        assert_eq!(app.composer.text, "2");
        app.composer.clear();
        app.action(Action::Question, &c);
        capture(60, 54, &mut app, &o, &c);
        assert!(app.selection.drawn.is_some());
        capture(19, 8, &mut app, &o, &c);
        assert!(app.selection.drawn.is_none());
    }
    #[test]
    fn wraps_words_and_preserves_scroll_and_input_across_resize_and_new_chat() {
        let lines = wrap(vec![Span::raw("one wonderful word extraordinary")], 12);
        let text: Vec<_> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(text, ["one", "wonderful", "word", "extraordinar", "y"]);
        let fx = fixture();
        let mut app = App::default();
        app.composer.insert("Keep this.");
        let mut c = Conversation::default();
        let o = Overview::load(
            &fx.project,
            &super::super::read(&fx.project),
            &c,
            &Live::default(),
        );
        app.refresh(&c);
        capture(60, 54, &mut app, &o, &c);
        let top = app.combined.top;
        c.items.push(view::Item {
            at: String::new(),
            body: Body::Rolf("new words ".repeat(200)),
            delivery: None,
        });
        app.refresh(&c);
        let text = capture(60, 54, &mut app, &o, &c);
        assert_eq!(app.combined.top, top);
        assert!(text.contains("↓ new"));
        app.action(Action::Overview, &c);
        capture(80, 40, &mut app, &o, &c);
        app.action(Action::Overview, &c);
        capture(60, 54, &mut app, &o, &c);
        assert_eq!(app.composer.text, "Keep this.");
        assert_eq!(app.combined.top, top);
        app.focus = Area::Combined;
        app.action(Action::End, &c);
        capture(60, 54, &mut app, &o, &c);
        assert!(!app.combined.new);
        assert!(app.combined.follow);
    }
    #[test]
    fn mouse_focus_and_composer_cells_do_not_change_the_message() {
        let mut app = App::default();
        app.composer.insert("one  wonderful word ");
        let (rows, row, col) = composer_layout(&app.composer, 12);
        let text: String = rows
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert_eq!(text, app.composer.text);
        assert_eq!((row, col), (2, 5));
        app.composer.left();
        app.composer.backspace();
        app.composer.insert("界");
        assert!(app.composer.text.ends_with("wor界 "));
        app.regions = vec![
            (Area::Overview, Rect::new(0, 1, 80, 20)),
            (Area::Chat, Rect::new(0, 22, 43, 10)),
        ];
        app.overview.len = 100;
        app.overview.height = 20;
        app.mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 2,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.focus, Area::Overview);
        assert_eq!(app.overview.top, 3);
        assert_eq!(app.chat.top, 0);
        let reference = super::super::AnswerRef {
            id: "a-1".into(),
            revision: 1,
        };
        app.hits.push((Rect::new(43, 22, 36, 8), reference.clone()));
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 44,
            row: 23,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selection.selected, Some(reference));
        assert!(app.reveal);
        assert!(app.selection.drawn.is_none());
    }
    #[test]
    fn cleanup_runs_on_partial_setup_errors_normal_exit_and_unwind() {
        use std::cell::Cell;
        for fail_at in 0..6 {
            let cleaned = Cell::new(0);
            let result = guarded(
                || cleaned.set(cleaned.get() + 1),
                || {
                    for stage in 0..5 {
                        if stage == fail_at {
                            anyhow::bail!("terminal operation failed");
                        }
                    }
                    Ok(())
                },
            );
            assert_eq!(result.is_err(), fail_at < 5);
            assert_eq!(cleaned.get(), 1);
        }
        let cleaned = Cell::new(false);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Result<()> = guarded(|| cleaned.set(true), || panic!("draw failed"));
        }));
        assert!(cleaned.get());
    }
    #[test]
    fn every_fixed_label_passes_the_empty_registry() {
        let g = crate::plain::Glossary::default();
        let mut labels = vec![
            "Your project",
            "Mon Tue Wed Thu Fri Sat Sun",
            "Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec",
            "idle",
            "working",
            "blocked",
            "done",
            "gone",
            "unreachable",
            "other tab",
            "2 open tasks",
            "1 need you",
            "checked after reply",
            "say and ask only",
            "chat 13:35",
            "coordinator",
            "you",
            "for you",
            "question",
            "you chose 0",
            "question asked again below",
            "queued",
            "sent",
            "unsure",
            "accepted",
            "needs you",
            "asked 13:33",
            "type a message",
            "type a message · 0 to 3 answers the selected question",
            "type, or 0 to 3 answers",
            "this question has choices 0 to 3",
            "more below",
            "more above",
            "new",
            "Make this view larger.",
            "result",
            "money",
            "undo",
            "routine",
            "Decided for you",
            "working box last seen",
            "This finished work needs a plain description.",
            "This question needs a plain sentence.",
            "This question could not be shown.",
            "I could not send this message.",
            "I could not send this answer.",
        ];
        labels.extend(overview::HEADINGS);
        labels.extend(overview::EMPTY);
        labels.extend([
            overview::GOAL_INVALID,
            overview::TASK_INVALID,
            overview::STEP_INVALID,
            overview::DECISION_INVALID,
            overview::CATCH_UP,
            overview::RUNNING_ERROR,
            overview::FINISHED_ERROR,
            overview::DECISIONS_ERROR,
            overview::NO_DECISIONS,
            overview::CHANGE,
            overview::ASK_WARNING,
            overview::STALE_HEADING,
            overview::STALE_UNKNOWN,
            overview::COST_HEADING,
            overview::TASKS_HEADING,
            overview::NO_COST,
            overview::COST_ERROR,
            overview::NO_TASKS,
            overview::TASKS_ERROR,
        ]);
        let wide = hints(false);
        let narrow = hints(true);
        labels.extend([wide.as_str(), narrow.as_str(), crate::ask::NOT_UNDERSTOOD]);
        let failures: Vec<_> = labels
            .iter()
            .filter_map(|s| {
                let r = crate::plain::check(s, &g);
                (!r.passed()).then(|| format!("{s}: {:?}", r.violations))
            })
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn a_split_mouse_report_is_not_typed_into_the_composer() {
        let mut app = App::default();
        let c = Conversation::default();
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let ch = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        // The Esc arrives alone, then the rest of the SGR report is read as
        // plain characters, as a slow terminal can split it.
        app.key(esc, &c);
        for letter in "[<64;15;5M".chars() {
            app.key(ch(letter), &c);
        }
        assert_eq!(app.composer.text, "");
        // A real clear still lets ordinary typing through.
        app.key(esc, &c);
        for letter in "hello".chars() {
            app.key(ch(letter), &c);
        }
        assert_eq!(app.composer.text, "hello");
    }

    #[test]
    fn the_selected_question_fits_at_the_default_sizes() {
        let fx = fixture();
        let a = crate::ask::ask(
            &fx.world.ctx(),
            "demo",
            crate::ask::NewAsk {
                question: "May I spend five dollars on this check?".into(),
                choices: vec![
                    "Keep it running.".into(),
                    "Stop it now.".into(),
                    "Wait for me.".into(),
                ],
                what: None,
                means: None,
                round: None,
                reask: None,
            },
        )
        .unwrap();
        assert_eq!(a.choices.len(), 3);
        let j = super::super::read(&fx.project);
        let c = Conversation::load(&fx.project, &j);
        let o = Overview::load(&fx.project, &j, &c, &Live::default());
        for (w, h) in [(120u16, 40u16), (134u16, 40u16), (60u16, 40u16)] {
            let mut app = App::default();
            app.refresh(&c);
            let text = capture(w, h, &mut app, &o, &c);
            assert_eq!(app.questions.max(), 0, "{w}x{h} needs scrolling:\n{text}");
            assert!(
                text.contains("I did not understand the question"),
                "{w}x{h}:\n{text}"
            );
        }
    }

    #[test]
    fn an_untimed_notice_does_not_draw_an_empty_date_rule() {
        let mut c = Conversation::default();
        c.items.push(view::Item {
            at: String::new(),
            body: Body::Notice("Your message waits until the coordinator is ready.".into()),
            delivery: None,
        });
        let d = timeline(&c, &Selection::default(), 60, false, Theme::default());
        let text: String = d
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("── ──"), "{text}");
    }

    #[test]
    fn only_a_different_installed_build_is_stale() {
        assert!(!screen_version_behind(None));
        assert!(!screen_version_behind(Some(crate::VERSION)));
        assert!(screen_version_behind(Some("0.1.0+older")));
    }
}
