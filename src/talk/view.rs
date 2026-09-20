//! Conversation-only folding. No delivery, live calls or overview reads.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};

use super::{AnswerRef, Entry, Journal};
use crate::{ask, contracts::Ask, contracts::TalkRequestState, project::Project};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub revision: u32,
    pub choices: usize,
}

#[derive(Debug, Clone)]
pub enum Body {
    Say { what: String, means: Option<String> },
    Rolf(String),
    Card { ask: Box<Ask>, state: CardState },
    Notice(String),
}

#[derive(Debug, Clone)]
pub enum CardState {
    Open,
    Answered { choice: u32, text: String },
    AskedAgain,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub at: String,
    pub body: Body,
    pub delivery: Option<TalkRequestState>,
}

#[derive(Debug, Default)]
pub struct Conversation {
    pub items: Vec<Item>,
    pub open: Vec<Ask>,
    pub latest_at: String,
}

pub fn clock(at: &str) -> String {
    local(at)
        .map(|z| z.strftime("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".into())
}
fn local(at: &str) -> Option<jiff::Zoned> {
    Some(
        at.parse::<jiff::Timestamp>()
            .ok()?
            .to_zoned(jiff::tz::TimeZone::system()),
    )
}
pub fn date(at: &str) -> String {
    local(at)
        .map(|z| z.strftime("%a %d %b").to_string())
        .unwrap_or_default()
}
pub fn delivery(state: TalkRequestState) -> &'static str {
    match state {
        TalkRequestState::Queued => "queued",
        TalkRequestState::Submitted => "sent",
        TalkRequestState::Uncertain => "unsure",
        TalkRequestState::Accepted => "accepted",
    }
}

impl Conversation {
    pub fn load(project: &Project, journal: &Journal) -> Self {
        let mut lines: Vec<_> = journal.lines.iter().collect();
        lines.sort_by_key(|l| l.seq);
        let mut states = BTreeMap::new();
        let mut latest = BTreeMap::<String, u32>::new();
        let mut carriers = BTreeMap::new();
        let mut answers = BTreeMap::new();
        for l in &lines {
            match &l.entry {
                Entry::Inbound(i) => {
                    states.insert(i.request.clone(), i.state);
                }
                Entry::Ask { id, revision } => {
                    latest
                        .entry(id.clone())
                        .and_modify(|r| *r = (*r).max(*revision))
                        .or_insert(*revision);
                }
                Entry::Answer {
                    id,
                    revision,
                    choice,
                } => {
                    answers.insert(
                        AnswerRef {
                            id: id.clone(),
                            revision: *revision,
                        },
                        (*choice, l.at.clone()),
                    );
                }
                Entry::Rolf {
                    request,
                    answer: Some(reference),
                    ..
                } => {
                    carriers.insert(reference.clone(), request.clone());
                }
                _ => {}
            }
        }
        // Historical carrier text is hidden only on an exact match with a
        // stored answer. Never infer an answer from prose.
        let mut old_carriers = BTreeSet::new();
        let references: BTreeSet<_> = lines
            .iter()
            .filter_map(|l| match &l.entry {
                Entry::Ask { id, revision } => Some(AnswerRef {
                    id: id.clone(),
                    revision: *revision,
                }),
                _ => None,
            })
            .collect();
        for l in &lines {
            if let Entry::Rolf {
                request,
                text,
                answer: None,
            } = &l.entry
            {
                for reference in &references {
                    if let Some(a) = ask::answer_of(project, &reference.id, reference.revision) {
                        let exact = format!(
                            "ANSWER {}@{} {}: Rolf chose \"{}\"",
                            a.id, a.revision, a.choice, a.text
                        );
                        if text == &exact {
                            carriers.insert(reference.clone(), request.clone());
                            old_carriers.insert(request.clone());
                        }
                    }
                }
            }
        }
        let authoritative = ask::open_asks(project);
        let mut seen = BTreeSet::new();
        let mut result = Self {
            latest_at: lines.last().map(|l| l.at.clone()).unwrap_or_default(),
            ..Self::default()
        };
        for l in lines {
            let mut at = l.at.clone();
            let mut status = None;
            let body = match &l.entry {
                Entry::Inbound(_) | Entry::Answer { .. } => continue,
                Entry::Rolf {
                    request,
                    text,
                    answer,
                } => {
                    if answer.is_some() || old_carriers.contains(request) {
                        continue;
                    }
                    status = states.get(request).copied();
                    Body::Rolf(text.clone())
                }
                Entry::Say { what, means, .. } => Body::Say {
                    what: what.clone(),
                    means: means.clone(),
                },
                Entry::Notice { id } => match ask::notice_text(id) {
                    Some(text) => Body::Notice(text.into()),
                    None => continue,
                },
                Entry::Ask { id, revision } => {
                    let reference = AnswerRef {
                        id: id.clone(),
                        revision: *revision,
                    };
                    if !seen.insert(reference.clone()) {
                        continue;
                    }
                    let Ok(Some(a)) = ask::load_revision(project, id, *revision) else {
                        result.items.push(Item {
                            at,
                            body: Body::Notice("This question could not be shown.".into()),
                            delivery: None,
                        });
                        continue;
                    };
                    if let Some(withdrawal) = ask::withdrawal_of(project, id, *revision) {
                        result.items.push(Item {
                            at: withdrawal.at,
                            body: Body::Notice(format!(
                                "{id} withdrawn: {} — {}",
                                a.question, withdrawal.reason
                            )),
                            delivery: None,
                        });
                        continue;
                    }
                    let state = if latest.get(id).is_some_and(|r| r > revision) {
                        CardState::AskedAgain
                    } else if let Some(answer) = ask::answer_of(project, id, *revision) {
                        at = answers
                            .get(&reference)
                            .map(|(_, at)| at.clone())
                            .unwrap_or(answer.answered);
                        status = carriers
                            .get(&reference)
                            .and_then(|r| states.get(r))
                            .copied();
                        CardState::Answered {
                            choice: answer.choice,
                            text: answer.text,
                        }
                    } else if let Some((choice, when)) = answers.get(&reference) {
                        at = when.clone();
                        let text = if *choice == 0 {
                            ask::NOT_UNDERSTOOD.into()
                        } else {
                            a.choices
                                .get(*choice as usize - 1)
                                .cloned()
                                .unwrap_or_default()
                        };
                        status = carriers
                            .get(&reference)
                            .and_then(|r| states.get(r))
                            .copied();
                        CardState::Answered {
                            choice: *choice,
                            text,
                        }
                    } else {
                        if authoritative
                            .iter()
                            .any(|open| open.id == *id && open.revision == *revision)
                        {
                            result.open.push(a.clone());
                        }
                        CardState::Open
                    };
                    Body::Card {
                        ask: Box::new(a),
                        state,
                    }
                }
            };
            result.items.push(Item {
                at,
                body,
                delivery: status,
            });
        }
        result
    }

    pub fn replay(&self) -> String {
        let mut out = String::new();
        let mut day = String::new();
        for item in &self.items {
            let next = date(&item.at);
            if next != day {
                out.push_str(&format!("── {next} ──\n"));
                day = next;
            }
            let time = clock(&item.at);
            let status = item
                .delivery
                .map(|s| format!("  {}", delivery(s)))
                .unwrap_or_default();
            match &item.body {
                Body::Say { what, means } => {
                    out.push_str(&format!("{time} coordinator\n{what}\n"));
                    if let Some(means) = means {
                        out.push_str(&format!("for you: {means}\n"));
                    }
                }
                Body::Rolf(text) => out.push_str(&format!("{time} you{status}\n{text}\n")),
                Body::Notice(text) => out.push_str(&format!("{time} ! {text}\n")),
                Body::Card {
                    ask,
                    state: CardState::Open,
                } => {
                    out.push_str(&format!("{time} question\n"));
                    for text in [&ask.what, &ask.means].into_iter().flatten() {
                        out.push_str(&format!("{text}\n"));
                    }
                    out.push_str(&ask.question);
                    out.push('\n');
                    for (i, text) in ask.choices.iter().enumerate() {
                        out.push_str(&format!("  {}. {text}\n", i + 1));
                    }
                    out.push_str(&format!("  0. {}\n", ask::NOT_UNDERSTOOD));
                }
                Body::Card {
                    state: CardState::Answered { choice, text },
                    ..
                } => out.push_str(&format!("{time} you chose {choice}{status}\n{text}\n")),
                Body::Card {
                    state: CardState::AskedAgain,
                    ..
                } => out.push_str(&format!("{time} question asked again below\n")),
            }
            out.push('\n');
        }
        out
    }
}

#[derive(Default)]
pub struct Selection {
    pub selected: Option<AnswerRef>,
    /// Set only after a frame was actually drawn. Never changed by refresh.
    pub drawn: Option<Target>,
}
impl Selection {
    pub fn refresh(&mut self, open: &[Ask]) {
        if !open.iter().any(|a| self.matches(a)) {
            self.selected = open.last().map(reference);
        }
    }
    pub fn matches(&self, a: &Ask) -> bool {
        self.selected.as_ref() == Some(&reference(a))
    }
    pub fn next(&mut self, open: &[Ask]) {
        if open.is_empty() {
            return;
        }
        let i = open
            .iter()
            .position(|a| self.matches(a))
            .map_or(0, |i| (i + 1) % open.len());
        self.selected = Some(reference(&open[i]));
    }
}
fn reference(a: &Ask) -> AnswerRef {
    AnswerRef {
        id: a.id.clone(),
        revision: a.revision,
    }
}

/// Only complete bytes advance the cursor. A cut tail is retried on the next
/// poll; truncation resets the view instead of retaining phantom entries.
#[derive(Default)]
pub struct JournalReader {
    pub journal: Journal,
    offset: u64,
}
impl JournalReader {
    pub fn refresh(&mut self, project: &Project) -> std::io::Result<()> {
        let mut file = match std::fs::File::open(super::journal_path(project)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                *self = Self::default();
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        if file.metadata()?.len() < self.offset {
            *self = Self::default();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let count = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let fresh = super::parse(&bytes[..count]);
        self.journal.lines.extend(fresh.lines);
        self.journal.skipped += fresh.skipped;
        self.journal.tail_incomplete = count < bytes.len();
        self.offset += count as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::testkit::fixture;
    pub fn question(fx: &crate::round::testkit::Fx, reask: Option<String>) -> Ask {
        ask::ask(
            &fx.world.ctx(),
            "demo",
            ask::NewAsk {
                question: format!(
                    "May I spend {} dollars on this check?",
                    ask::open_asks(&fx.project).len() + 5
                ),
                choices: vec![
                    "Keep it running.".into(),
                    "Stop it now.".into(),
                    "Wait for me.".into(),
                ],
                what: None,
                means: None,
                round: None,
                reask,
            },
        )
        .unwrap()
    }
    #[test]
    fn folds_duplicates_answers_delivery_reasks_and_sticky_selection() {
        let fx = fixture();
        let a = question(&fx, None);
        super::super::append(
            &fx.project,
            None,
            Entry::Ask {
                id: a.id.clone(),
                revision: a.revision,
            },
        )
        .unwrap();
        let mut select = Selection::default();
        let v = Conversation::load(&fx.project, &super::super::read(&fx.project));
        assert_eq!(v.items.len(), 1);
        assert_eq!(v.open.len(), 1);
        select.refresh(&v.open);
        let b = question(&fx, None);
        let v = Conversation::load(&fx.project, &super::super::read(&fx.project));
        select.refresh(&v.open);
        assert!(select.matches(&a));
        select.next(&v.open);
        assert!(select.matches(&b));
        let target = Target {
            id: b.id.clone(),
            revision: b.revision,
            choices: 3,
        };
        select.drawn = Some(target.clone());
        question(&fx, Some(b.id.clone()));
        select.refresh(&Conversation::load(&fx.project, &super::super::read(&fx.project)).open);
        assert_eq!(select.drawn, Some(target.clone()));
        super::super::answer(&fx.world.ctx(), &fx.project, &target, 1).unwrap();
        assert!(ask::answer_of(&fx.project, &b.id, 2).is_none());
        super::super::answer(
            &fx.world.ctx(),
            &fx.project,
            &Target {
                id: a.id.clone(),
                revision: 1,
                choices: 3,
            },
            2,
        )
        .unwrap();
        let v = Conversation::load(&fx.project, &super::super::read(&fx.project));
        assert!(v.items.iter().any(|i| matches!(
            &i.body,
            Body::Card {
                state: CardState::AskedAgain,
                ..
            }
        )));
        assert!(v.replay().contains("you chose 2"));
        assert!(!v.replay().contains("ANSWER"));
        assert!(!v.items.iter().any(|i| matches!(i.body, Body::Rolf(_))));
    }
    #[test]
    fn incremental_reader_retries_tail_and_resets_after_truncation() {
        let fx = fixture();
        std::fs::create_dir_all(super::super::talk_dir(&fx.project)).unwrap();
        let path = super::super::journal_path(&fx.project);
        std::fs::write(&path, "bad\n{\"seq\":1,\"notice\":{\"id\":\"native_on\"}").unwrap();
        let mut r = JournalReader::default();
        r.refresh(&fx.project).unwrap();
        assert_eq!(r.journal.skipped, 1);
        assert!(r.journal.lines.is_empty());
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"}\n")
            .unwrap();
        r.refresh(&fx.project).unwrap();
        assert_eq!(r.journal.lines.len(), 1);
        r.refresh(&fx.project).unwrap();
        assert_eq!(r.journal.lines.len(), 1);
        std::fs::write(&path, "").unwrap();
        r.refresh(&fx.project).unwrap();
        assert!(r.journal.lines.is_empty());
    }
}
