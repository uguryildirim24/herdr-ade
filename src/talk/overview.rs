//! Read-only projection of the existing project records. Step state comes
//! from plan::project_states, the very same projection used by plan sync.
use super::{Entry, Journal, cost::Cost, stale::Stale, tasks, view::Conversation};
use crate::{
    contracts::StepState,
    decide, glossary,
    herdr::{Agent, Pane},
    paths::Ctx,
    plan,
    project::Project,
    round,
    thread::{self, Group, Status},
    threads,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const HEADINGS: [&str; 7] = [
    "Goal",
    "What you get at the end",
    "How far along",
    "Running now",
    "Finished lately",
    "Needs you",
    "Failures",
];
pub(crate) const EMPTY: [&str; 7] = [
    "Your goal is not written down yet.",
    "The end result is not written down yet.",
    "The steps are not written down yet.",
    "Nothing is running now.",
    "Nothing has landed yet.",
    "No question is waiting here.",
    "No new or repeated failure is open.",
];
pub(crate) const GOAL_INVALID: &str = "Your goal needs a plain sentence.";
pub(crate) const TASK_INVALID: &str = "This task needs a plain description.";
pub(crate) const STEP_INVALID: &str = "This step needs a plain description.";
pub(crate) const DECISION_INVALID: &str = "This choice needs a plain description.";
pub(crate) const CATCH_UP: &str = "The plan needs to catch up.";
pub(crate) const RUNNING_ERROR: &str = "I could not read the running work.";
pub(crate) const FINISHED_ERROR: &str = "I could not read the finished work.";
pub(crate) const DECISIONS_ERROR: &str = "I could not read the choices.";
pub(crate) const NO_DECISIONS: &str = "No choices have been recorded yet.";
pub(crate) const CHANGE: &str = "Tell me what to change in the chat.";
pub(crate) const ASK_WARNING: &str = "More than three questions are waiting here.";
pub(crate) const STALE_HEADING: &str = "Stale";
pub(crate) const STALE_UNKNOWN: &str = "Some running programs could not be checked.";
pub(crate) const COST_HEADING: &str = "Cost";
pub(crate) const TASKS_HEADING: &str = "Tasks";
pub(crate) const NO_COST: &str = "No cost has been recorded yet.";
pub(crate) const COST_ERROR: &str = "I could not read the cost.";
pub(crate) const NO_TASKS: &str = "No tasks are on the list.";
pub(crate) const TASKS_ERROR: &str = "I could not read the task list.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tone {
    #[default]
    Text,
    Heading,
    Dim,
    Green,
    Yellow,
    Red,
    Peach,
    Accent,
}
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub(crate) text: String,
    pub(crate) prefix: String,
    pub(crate) marker: String,
    pub(crate) tone: Tone,
}
impl Row {
    fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            prefix: String::new(),
            marker: String::new(),
            tone: Tone::Text,
        }
    }
    pub(crate) fn full_text(&self) -> String {
        [&self.prefix, &self.text, &self.marker]
            .into_iter()
            .filter(|part| !part.is_empty())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ")
    }
}
#[derive(Default)]
pub(crate) struct Overview {
    pub(crate) sections: [Vec<Row>; 7],
    pub(crate) progress: Option<(usize, usize)>,
    pub(crate) active: usize,
    pub(crate) needs: usize,
    pub(crate) name: String,
    /// LEAN U4: only items that are behind; empty when nothing is stale.
    pub(crate) stale: Vec<Row>,
    /// LEAN U1: today's total and the current round, already rendered.
    pub(crate) cost: Vec<Row>,
    /// LEAN U5: Rolf's lists, each with its heading and one line per task.
    pub(crate) tasks: Vec<Row>,
}

/// One workflow word for a thread group, the same words Running now uses.
fn state_word(group: Group) -> &'static str {
    match group {
        Group::WaitingOnYou => "needs you",
        Group::ReadyForReview | Group::Landing => "checking",
        _ => "working",
    }
}

/// One shared poll for all local rows and the coordinator. Remote records
/// are the courier's last observation, never a claim of fresh connectivity.
#[derive(Default)]
pub(crate) struct Live {
    pub(crate) agents: Vec<Agent>,
    pub(crate) panes: Vec<Pane>,
    pub(crate) reachable: bool,
    pub(crate) state: String,
    /// LEAN U4 and U1, refreshed on a slower cadence than the live poll.
    pub(crate) stale: Stale,
    pub(crate) cost: Cost,
    groups: BTreeMap<String, Group>,
}
impl Live {
    /// The slower scan: staleness and cost read files and, for the box server,
    /// one SSH call. Separate from the three-second live poll.
    pub(crate) fn refresh_slow(&mut self, ctx: &Ctx, project: &Project) {
        self.stale = super::stale::scan(ctx, project);
        self.cost = super::cost::load(ctx, project);
    }

    pub(crate) fn poll(&mut self, ctx: &Ctx, project: &Project) {
        if let Some(view) = threads::session_view(ctx, project) {
            self.agents = view.agents;
            self.panes = view.panes;
            self.reachable = true;
            let pane = project.coordinator().map(|c| c.pane_id).unwrap_or_default();
            self.state = self
                .agents
                .iter()
                .find(|a| a.pane_id == pane)
                .map(|a| a.agent_status.clone())
                .unwrap_or_else(|| "gone".into());
            for t in thread::list(project).into_iter().filter(|t| !t.is_remote()) {
                let now = jiff::Timestamp::now();
                let live = thread::live_state(&t, &self.agents, &self.panes, now);
                self.groups
                    .insert(t.id.clone(), thread::group(&t, &live, now));
            }
        } else {
            self.reachable = false;
        }
    }
    pub(crate) fn state(&self, project: &Project) -> &str {
        if super::writer_suspended(project) {
            "other tab"
        } else if !self.reachable {
            "unreachable"
        } else {
            match self.state.as_str() {
                "idle" | "working" | "blocked" | "done" | "gone" => &self.state,
                _ => "unreachable",
            }
        }
    }
    fn group(&self, t: &thread::Thread) -> Group {
        if !t.is_remote()
            && let Some(group) = self.groups.get(&t.id)
        {
            return *group;
        }
        thread::recorded_group(t, jiff::Timestamp::now())
    }
}

fn checked(project: &Project, text: &str) -> bool {
    !text.trim().is_empty()
        && !text.chars().any(char::is_control)
        && glossary::name_in(project, text).is_none()
        && glossary::gate_row(project, text).is_ok()
}
fn safe(project: &Project, text: &str, fallback: &str) -> String {
    if checked(project, text) {
        text.to_string()
    } else {
        fallback.into()
    }
}
fn tagged(project: &Project, prefix: &str, text: &str, tone: Tone, fallback: &str) -> Row {
    tagged_with_marker(project, prefix, text, "", tone, fallback)
}

fn tagged_with_marker(
    project: &Project,
    prefix: &str,
    text: &str,
    marker: &str,
    tone: Tone,
    fallback: &str,
) -> Row {
    let composed = [prefix, text, marker]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Row {
        prefix: prefix.into(),
        text: if checked(project, &composed) {
            text.into()
        } else {
            fallback.into()
        },
        marker: marker.into(),
        tone,
    }
}

/// Unlike thread::list, do not silently equate unreadable records with an
/// empty project. Preserve every readable row and add an explicit failure.
fn records<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> (Vec<T>, bool) {
    let mut result = Vec::new();
    let mut failed = false;
    match std::fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(e) if e.path().extension().is_some_and(|ext| ext == "toml") => {
                        match std::fs::read_to_string(e.path())
                            .ok()
                            .and_then(|s| toml::from_str(&s).ok())
                        {
                            Some(record) => result.push(record),
                            None => failed = true,
                        }
                    }
                    Err(_) => failed = true,
                    _ => {}
                }
            }
        }
        Err(e) => failed = e.kind() != std::io::ErrorKind::NotFound,
    }
    (result, failed)
}

impl Overview {
    pub(crate) fn load(
        project: &Project,
        journal: &Journal,
        conversation: &Conversation,
        live: &Live,
    ) -> Self {
        let mut out = Self::default();
        match project.read_project_md() {
            Ok((s, _)) => {
                // The project name is Rolf's label, not generated overview
                // prose. Keep it exactly as stored; only an absent name gets
                // the fixed fallback.
                out.name = if s.name.is_empty() {
                    "Your project".into()
                } else {
                    s.name
                };
                out.sections[0].push(Row::text(if s.goal.is_empty() {
                    EMPTY[0].into()
                } else {
                    safe(project, &s.goal, GOAL_INVALID)
                }));
            }
            Err(_) => {
                out.name = "Your project".into();
                out.sections[0].push(Row::text(GOAL_INVALID));
            }
        }
        if let Ok(Some(mut card)) = plan::load(project) {
            if card.schema == 1
                && let Some(sentence) = crate::contracts::plan_kind_sentence(&card.kind)
                && card.what_you_get == sentence
                && checked(project, &card.does)
            {
                out.sections[1].push(Row::text(sentence));
                out.sections[1].push(Row::text(card.does.clone()));
            }
            let drift = plan::project_states(project, &mut card);
            let goal_drift = project
                .read_project_md()
                .is_ok_and(|(s, _)| s.goal != card.goal);
            if !card.steps.is_empty() {
                out.progress = Some((
                    card.steps
                        .iter()
                        .filter(|s| s.state == StepState::Done)
                        .count(),
                    card.steps.len(),
                ));
                for step in card.steps {
                    let tone = match step.state {
                        StepState::Done => Tone::Green,
                        StepState::Running => Tone::Yellow,
                        StepState::Left => Tone::Dim,
                    };
                    out.sections[2].push(tagged(
                        project,
                        step.state.word(),
                        &step.text,
                        tone,
                        STEP_INVALID,
                    ));
                }
            }
            if out.sections[2].is_empty() {
                out.sections[2].push(Row::text(EMPTY[2]));
            }
            if drift || goal_drift {
                out.sections[2].push(Row::text(CATCH_UP));
            }
        }
        let (mut tasks, failed) = records::<thread::Thread>(&project.dir().join("threads"));
        tasks.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
        let known: BTreeMap<String, Group> = tasks
            .iter()
            .map(|t| {
                let group = if t.status == Status::Resolved {
                    Group::ReadyForReview
                } else {
                    live.group(t)
                };
                (t.id.clone(), group)
            })
            .collect();
        let (rounds, rounds_failed) =
            records::<crate::contracts::RoundRecord>(&round::rounds_dir(project));
        let mut seen_work = BTreeSet::new();
        for t in tasks {
            let carrying: Vec<&crate::contracts::RoundRecord> =
                rounds.iter().filter(|round| round.carries(&t.id)).collect();
            if !carrying.is_empty()
                && carrying
                    .iter()
                    .all(|r| threads::round_landed(project, &r.round))
            {
                continue;
            }
            // A closed round cannot keep a pin pending.
            let pending_pin = carrying.iter().any(|r| {
                !threads::round_landed(project, &r.round)
                    && !r.phase.closed()
                    && r.manifest
                        .members
                        .iter()
                        .any(|m| m.thread == t.id && m.pin.is_some())
            });
            if t.status == Status::Resolved && !pending_pin {
                continue;
            }
            let group = if t.status == Status::Resolved {
                Group::ReadyForReview
            } else {
                live.group(&t)
            };
            let (state, tone) = match group {
                Group::WaitingOnYou => ("needs you", Tone::Red),
                Group::ReadyForReview | Group::Landing => ("checking", Tone::Yellow),
                _ => ("working", Tone::Yellow),
            };
            let marker = match (t.is_remote(), live.reachable) {
                (true, true) => "box",
                (true, false) => "box, last seen",
                (false, false) => "last seen",
                (false, true) => "",
            };
            let text = safe(project, &t.plain, TASK_INVALID);
            // A round and its reviewer carry the round's sentence; show the
            // work once. An unnamed row keeps its own fallback.
            if checked(project, &t.plain) && !seen_work.insert(text.clone()) {
                continue;
            }
            out.sections[3].push(tagged_with_marker(
                project,
                state,
                &text,
                marker,
                tone,
                TASK_INVALID,
            ));
            // Error strings may contain internal detail; only a checked plain
            // explanation can accompany the retained work row.
            if !t.error.is_empty() && checked(project, &t.error) {
                out.sections[3].push(Row::text(t.error));
            }
        }
        if failed || rounds_failed {
            out.sections[3].push(Row::text(RUNNING_ERROR));
        }
        let mut landed = BTreeSet::new();
        let mut evidence_failed = rounds_failed || journal.skipped > 0;
        for l in journal.lines.iter().rev() {
            if let Entry::Say {
                what,
                landed_round: Some(id),
                ..
            } = &l.entry
            {
                match round::read_merge(project, id) {
                    Ok(Some(m)) if m.phase == crate::contracts::MergePhase::Checkpointed => {
                        if landed.insert(id) {
                            out.sections[4].push(Row::text(safe(
                                project,
                                what,
                                "This finished work needs a plain description.",
                            )));
                        }
                    }
                    Ok(None) | Err(_) => evidence_failed = true,
                    _ => {}
                }
                if out.sections[4].len() == 5 {
                    break;
                }
            }
        }
        if evidence_failed
            || (out.sections[4].is_empty()
                && rounds
                    .iter()
                    .any(|r| threads::round_landed(project, &r.round)))
        {
            out.sections[4].push(Row::text(FINISHED_ERROR));
        }
        out.needs = conversation.open.len();
        for a in &conversation.open {
            out.sections[5].push(Row::text(safe(
                project,
                &a.question,
                "This question needs a plain sentence.",
            )));
        }
        if out.sections[5].is_empty() {
            out.sections[5].push(Row::text(EMPTY[5]));
        }
        if conversation.open.len() > 3 {
            out.sections[5].push(Row::text(ASK_WARNING));
        }
        out.sections[5].push(Row {
            text: "Decided for you".into(),
            prefix: String::new(),
            marker: String::new(),
            tone: Tone::Heading,
        });
        let choices = decide::current(project);
        let choices_failed = match std::fs::read_to_string(decide::decisions_path(project)) {
            Ok(text) => {
                !text.ends_with('\n') && !text.is_empty()
                    || text.lines().any(|line| {
                        !line.trim().is_empty()
                            && serde_json::from_str::<crate::contracts::Decision>(line).is_err()
                    })
            }
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        };
        if choices.is_empty() && !choices_failed {
            out.sections[5].push(Row::text(NO_DECISIONS));
        }
        for d in choices.into_iter().rev().take(5) {
            let (prefix, tone) = match d.class.as_str() {
                "what-you-get" => ("result", Tone::Accent),
                "money" => ("money", Tone::Yellow),
                "undo" => ("undo", Tone::Peach),
                _ => ("routine", Tone::Dim),
            };
            let prefix = if d.overturned.is_some() {
                "overturned"
            } else {
                prefix
            };
            let mut row = tagged(project, prefix, &d.line, tone, DECISION_INVALID);
            // The id is an action target, not user-authored prose.
            row.prefix = format!("{} {}", d.id, row.prefix);
            out.sections[5].push(row);
        }
        if choices_failed {
            out.sections[5].push(Row::text(DECISIONS_ERROR));
        }
        out.sections[5].push(Row::text(CHANGE));
        match crate::ledger::recent(project) {
            Ok(entries) => {
                for entry in entries {
                    // Keep raw error output in `ledger show`, not on Rolf's screen.
                    let what = match entry.kind.as_str() {
                        "reviewer-start-failed" => "The work check could not start",
                        "launch-not-attempted" => "A helper stopped before it could start",
                        "merge-refused" => "The checked work could not land",
                        "thread-error" | "thread-state" => "A helper needs help",
                        "courier-failed" => "The box could not send its work home",
                        "retry" => "The harness had to try again",
                        _ => "A command failed",
                    };
                    out.sections[6].push(Row {
                        text: format!("{what} ({} times).", entry.count),
                        prefix: String::new(),
                        marker: entry.id,
                        tone: Tone::Yellow,
                    });
                }
            }
            Err(_) => out.sections[6].push(Row::text("I could not read the failures.")),
        }
        for (i, rows) in out.sections.iter_mut().enumerate() {
            if rows.is_empty() {
                rows.push(Row::text(EMPTY[i]));
            }
        }
        out.stale = live
            .stale
            .items
            .iter()
            .map(|item| Row {
                text: format!("{} {}", item.what, item.remedy),
                prefix: String::new(),
                marker: String::new(),
                tone: Tone::Red,
            })
            .collect();
        if live.stale.unknown {
            out.stale.push(Row {
                text: STALE_UNKNOWN.into(),
                prefix: String::new(),
                marker: String::new(),
                tone: Tone::Peach,
            });
        }
        out.cost = cost_rows(&live.cost);
        let (tasks, open_tasks) = task_rows(project, &known);
        out.tasks = tasks;
        out.active = open_tasks;
        out
    }
}

/// One line per task, a heading per list, and the thread state for a
/// delegated task (LEAN U5).
fn task_rows(project: &Project, known: &BTreeMap<String, Group>) -> (Vec<Row>, usize) {
    let (lists, failed) = tasks::load(project);
    let mut rows = Vec::new();
    let mut count = 0;
    if failed {
        rows.push(Row::text(TASKS_ERROR));
    }
    for list in &lists {
        if list.tasks.is_empty() {
            continue;
        }
        rows.push(Row {
            text: list.heading.clone(),
            prefix: String::new(),
            marker: String::new(),
            tone: Tone::Heading,
        });
        for task in &list.tasks {
            count += 1;
            let marker = task
                .thread
                .as_ref()
                .and_then(|id| known.get(id))
                .map(|group| state_word(*group))
                .unwrap_or("");
            rows.push(tagged_with_marker(
                project,
                &task.owner,
                &task.title,
                marker,
                Tone::Text,
                TASK_INVALID,
            ));
        }
    }
    if rows.is_empty() && !failed {
        rows.push(Row::text(NO_TASKS));
    }
    (rows, count)
}

/// Today's money and tokens when the ledger has them, the current round when
/// one is open, and the elapsed time of the work running now (LEAN U1).
fn cost_rows(cost: &Cost) -> Vec<Row> {
    let mut rows = Vec::new();
    if cost.failed {
        rows.push(Row::text(COST_ERROR));
    }
    let today = super::cost::format_totals(&cost.today);
    if !today.is_empty() {
        rows.push(Row {
            prefix: "today".into(),
            text: today,
            marker: String::new(),
            tone: Tone::Text,
        });
    }
    if let Some((round, totals)) = &cost.round {
        let text = super::cost::format_totals(totals);
        if !text.is_empty() {
            rows.push(Row {
                prefix: format!("round {round}"),
                text,
                marker: String::new(),
                tone: Tone::Text,
            });
        }
    }
    if cost.running > 0 {
        rows.push(Row {
            prefix: "running".into(),
            text: format!("{} min so far", cost.running),
            marker: String::new(),
            tone: Tone::Text,
        });
    }
    if rows.is_empty() && !cost.failed {
        rows.push(Row::text(NO_COST));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::testkit::fixture;
    #[test]
    fn failures_use_the_digest_selection_and_keep_raw_errors_off_the_screen() {
        let fx = fixture();
        for n in 0..8 {
            crate::ledger::record(
                &fx.project,
                "reviewer-start-failed",
                &format!("r{n}"),
                "private raw error\nsecond line",
            )
            .unwrap();
        }
        crate::ledger::record(
            &fx.project,
            "reviewer-start-failed",
            "r0",
            "private raw error\nsecond line",
        )
        .unwrap();
        let overview = Overview::load(
            &fx.project,
            &Journal::default(),
            &Conversation::default(),
            &Live::default(),
        );
        let rows = &overview.sections[6];
        assert_eq!(rows.len(), 5);
        assert!(rows[0].text.contains("2 times"));
        assert_eq!(
            rows[0].marker,
            crate::ledger::recent(&fx.project).unwrap()[0].id
        );
        assert!(
            rows.iter()
                .all(|r| !r.text.contains('\n') && !r.text.contains("private raw"))
        );
        assert!(rows.iter().all(|r| checked(&fx.project, &r.text)));
    }

    #[test]
    fn one_fake_poll_supplies_all_local_rows_and_outage_keeps_last_state() {
        let fx = fixture();
        let a = fx.thread("First");
        let b = fx.thread("Second");
        for id in [&a, &b] {
            thread::update(&fx.project, id, |t| t.plain = "Build the screen.".into()).unwrap();
        }
        let mut live = Live::default();
        live.poll(&fx.world.ctx(), &fx.project);
        assert_eq!(fx.world.runner.count("agent list"), 1);
        assert_eq!(fx.world.runner.count("pane list"), 1);
        assert_eq!(
            live.group(&thread::load(&fx.project, &a).unwrap()),
            Group::WaitingOnYou
        );
        *fx.world.agents.borrow_mut() = "unreadable reply".into();
        live.poll(&fx.world.ctx(), &fx.project);
        assert!(!live.reachable);
        assert_eq!(live.state(&fx.project), "unreachable");
        std::fs::write(
            fx.project.dir().join("TASKS.md"),
            "## Backlog\n- [ ] First thing (me)\n- [ ] Second thing (agent)\n",
        )
        .unwrap();
        let before = fx.world.runner.calls.borrow().len();
        let o = Overview::load(
            &fx.project,
            &Journal::default(),
            &Conversation::default(),
            &live,
        );
        // The header counts the task list and the open questions, while the
        // running rows keep their own state words.
        assert_eq!(o.needs, 0);
        assert_eq!(o.active, 2);
        assert!(o.sections[3].iter().all(|r| {
            r.full_text()
                .contains("needs you Build the screen. last seen")
        }));
        assert_eq!(fx.world.runner.calls.borrow().len(), before);
    }

    #[test]
    fn the_header_keeps_the_project_name_even_when_it_is_not_plain_prose() {
        let fx = fixture();
        let path = fx.project.project_md();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replacen("name = \"Demo\"", "name = \"Adeherdr\"", 1),
        )
        .unwrap();
        let overview = Overview::load(
            &fx.project,
            &Journal::default(),
            &Conversation::default(),
            &Live::default(),
        );
        assert_eq!(overview.name, "Adeherdr");
    }

    #[test]
    fn an_existing_long_sentence_renders_cut_instead_of_the_invalid_fallback() {
        let fx = fixture();
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.goal = format!("{}.", vec!["the"; 26].join(" "));
        let front = toml::to_string(&settings).unwrap();
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{front}+++\n\n{body}"),
        )
        .unwrap();
        let o = Overview::load(
            &fx.project,
            &Journal::default(),
            &Conversation::default(),
            &Live::default(),
        );
        assert_ne!(o.sections[0][0].text, GOAL_INVALID);
        assert_eq!(o.sections[0][0].text, settings.goal);
    }

    #[test]
    fn handed_in_closed_work_stays_until_real_merge_and_only_landings_finish() {
        use crate::round::testkit::{commit_file, git};
        let fx = fixture();
        let ctx = fx.world.ctx();
        let (lane, sha) = fx.lane(1);
        thread::update(&fx.project, &lane, |t| {
            t.plain = "Build the first screen.".into();
            t.last_group = "ready-for-review".into();
        })
        .unwrap();
        plan::set(&ctx, "demo", "screen", "It shows pretend trades.", 0).unwrap();
        plan::step_add(
            &ctx,
            "demo",
            "Build the first screen.",
            vec![lane.clone()],
            vec![],
            1,
        )
        .unwrap();
        round::open(
            &ctx,
            "demo",
            round::OpenArgs {
                round: "r1".into(),
                branch: "main".into(),
                plain: Some("The first screen is ready.".into()),
                repo: Some(fx.repo.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        round::admit(&ctx, "demo", "r1", &lane).unwrap();
        fx.seal_done(&lane, 1, 1, &sha, "# report\n");
        let review = round::review(&ctx, "demo", "r1").unwrap();
        thread::update(&fx.project, &lane, |t| t.status = Status::Resolved).unwrap();
        crate::ask::say(&ctx, "demo", "The lane has finished.", None).unwrap();
        let snapshot = || {
            let journal = super::super::read(&fx.project);
            Overview::load(
                &fx.project,
                &journal,
                &Conversation::load(&fx.project, &journal),
                &Live::default(),
            )
        };
        let o = snapshot();
        assert_eq!(o.progress, Some((0, 1)));
        assert!(
            o.sections[3]
                .iter()
                .any(|r| r.prefix == "checking" && r.text.contains("Build the first screen."))
        );
        assert_eq!(o.sections[4][0].text, EMPTY[4]);
        git(&review.worktree, &["merge", "-q", "--no-edit", &sha]);
        let candidate = git(&review.worktree, &["rev-parse", "HEAD"]);
        let r = round::load(&fx.project, "r1").unwrap();
        let front = format!(
            "+++\nverdict = \"MERGE\"\nround = \"r1\"\ncandidate = \"{candidate}\"\nmanifest_hash = \"{}\"\npolicy_hash = \"{}\"\ngates = []\n+++\n\nAll gates pass.\n",
            r.manifest_hash.unwrap(),
            r.policy_hash
        );
        let verdict = commit_file(
            &review.worktree,
            &round::verdict_path("r1"),
            &front,
            "verdict",
        );
        let reviewer = fx.thread("Reviewer");
        fx.seal_done(&reviewer, 1, 1, &verdict, "# verdict report\n");
        round::bind_reviewer(&ctx, "demo", "r1", &reviewer).unwrap();
        round::merge(&ctx, "demo", "r1", None).unwrap();
        // Even stale unresolved process state cannot bring landed work back.
        thread::update(&fx.project, &lane, |t| t.status = Status::Open).unwrap();
        let o = snapshot();
        assert_eq!(o.progress, Some((1, 1)));
        assert!(
            !o.sections[3]
                .iter()
                .any(|r| r.text.contains("Build the first screen."))
        );
        assert_eq!(o.sections[4].len(), 1);
        assert_ne!(o.sections[4][0].text, EMPTY[4]);
        assert!(!o.sections[4][0].text.contains("lane has finished"));
    }

    #[test]
    fn an_abandoned_round_does_not_keep_a_handed_in_lane_running() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let (lane, sha) = fx.lane(1);
        thread::update(&fx.project, &lane, |t| {
            t.plain = "Check the abandoned path.".into();
            t.last_group = "ready-for-review".into();
        })
        .unwrap();
        round::open(
            &ctx,
            "demo",
            round::OpenArgs {
                round: "r1".into(),
                branch: "main".into(),
                plain: Some("The abandoned check is ready.".into()),
                repo: Some(fx.repo.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        round::admit(&ctx, "demo", "r1", &lane).unwrap();
        fx.seal_done(&lane, 1, 1, &sha, "# report\n");
        round::review(&ctx, "demo", "r1").unwrap();
        thread::update(&fx.project, &lane, |t| t.status = Status::Resolved).unwrap();
        round::cancel(&ctx, "demo", "r1", "superseded").unwrap();
        let o = Overview::load(
            &fx.project,
            &Journal::default(),
            &Conversation::default(),
            &Live::default(),
        );
        assert!(
            o.sections[3]
                .iter()
                .all(|r| !r.text.contains("abandoned path")),
            "{:?}",
            o.sections[3]
        );
    }

    #[test]
    fn a_lane_and_its_round_reviewer_show_the_work_once() {
        let fx = fixture();
        let (lane, _) = fx.lane(1);
        thread::update(&fx.project, &lane, |t| {
            t.plain = "Show pretend trades.".into();
            t.last_group = "working".into();
        })
        .unwrap();
        let reviewer = fx.thread("Reviewer");
        thread::update(&fx.project, &reviewer, |t| {
            t.plain = "Show pretend trades.".into();
            t.last_group = "working".into();
            t.role = "reviewer".into();
        })
        .unwrap();
        let o = Overview::load(
            &fx.project,
            &Journal::default(),
            &Conversation::default(),
            &Live::default(),
        );
        let rows = o.sections[3]
            .iter()
            .filter(|r| r.text == "Show pretend trades.")
            .count();
        assert_eq!(rows, 1, "{:?}", o.sections[3]);
    }

    #[test]
    fn the_cost_rows_show_money_or_running_time_not_an_invented_total() {
        let mut cost = Cost {
            running: 42,
            ..Cost::default()
        };
        let rows = cost_rows(&cost);
        assert!(
            rows.iter()
                .any(|r| r.prefix == "running" && r.text == "42 min so far")
        );
        assert!(rows.iter().all(|r| !r.full_text().contains("cost unknown")));
        cost.today = super::super::cost::Totals {
            tokens: 1_200,
            micros: 750_000,
            minutes: 10,
            runs: 1,
            unknown: false,
        };
        let rows = cost_rows(&cost);
        assert!(
            rows.iter()
                .any(|r| r.prefix == "today" && r.text.contains("$0.75"))
        );
    }

    #[test]
    fn fixture_projection_is_read_only_and_keeps_all_work() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        plan::set(&ctx, "demo", "screen", "It shows pretend trades.", 0).unwrap();
        for n in 0..18 {
            let id = fx.thread("work");
            thread::update(&fx.project, &id, |t| {
                t.plain = format!("Show task number {n}.");
                t.last_group = "working".into();
                if n == 0 {
                    t.machine = "oci".into();
                }
            })
            .unwrap();
        }
        plan::step_add(
            &ctx,
            "demo",
            "Show pretend trades.",
            vec!["t-0001".into()],
            vec![],
            1,
        )
        .unwrap();
        decide::decide(
            &ctx,
            "demo",
            decide::NewDecision {
                line: "I kept the words short.",
                class: "routine",
                key: None,
                basis: None,
                replaces: None,
                request: None,
            },
        )
        .unwrap();
        crate::ask::ask(
            &ctx,
            "demo",
            crate::ask::NewAsk {
                question: "May I spend five dollars on this check?".into(),
                choices: vec!["Keep it running.".into(), "Stop it now.".into()],
                what: None,
                means: None,
                round: None,
                reask: None,
            },
        )
        .unwrap();
        let before = std::fs::read(plan::plan_path(&fx.project)).unwrap();
        let j = super::super::read(&fx.project);
        let c = Conversation::load(&fx.project, &j);
        let o = Overview::load(&fx.project, &j, &c, &Live::default());
        assert_eq!(o.sections[3].len(), 18);
        assert_eq!(o.needs, 1);
        assert_eq!(o.progress, Some((0, 1)));
        assert!(o.sections[3][0].full_text().contains("box, last seen"));
        assert!(
            o.sections[5]
                .iter()
                .any(|r| r.full_text() == "d-0001 routine I kept the words short.")
        );
        for r in o.sections.iter().flatten() {
            assert!(
                glossary::gate_row(
                    &fx.project,
                    r.full_text()
                        .strip_prefix("d-0001 ")
                        .unwrap_or(&r.full_text())
                )
                .is_ok(),
                "{}",
                r.full_text()
            );
        }
        assert_eq!(before, std::fs::read(plan::plan_path(&fx.project)).unwrap());
        assert_eq!(o.sections[4][0].text, EMPTY[4]);
        decide::overturn(&ctx, "demo", "d-0001", "I want more detail.", "rolf").unwrap();
        let o = Overview::load(&fx.project, &j, &c, &Live::default());
        assert!(
            o.sections[5]
                .iter()
                .any(|r| r.full_text() == "d-0001 overturned I kept the words short.")
        );
    }

    #[test]
    fn the_task_list_shows_lists_owners_and_a_delegated_thread_state() {
        let fx = fixture();
        let lane = fx.thread("Work");
        thread::update(&fx.project, &lane, |t| {
            t.plain = "Build the screen.".into();
            t.last_group = "ready-for-review".into();
        })
        .unwrap();
        std::fs::write(
            fx.project.dir().join("TASKS.md"),
            format!(
                "# Tasks\n\n## Backlog\n- [ ] Write the screen (agent → {lane})\n- [ ] Ask Rolf (me)\n"
            ),
        )
        .unwrap();
        let o = Overview::load(
            &fx.project,
            &Journal::default(),
            &Conversation::default(),
            &Live::default(),
        );
        assert!(
            o.tasks
                .iter()
                .any(|r| r.tone == Tone::Heading && r.text == "Backlog")
        );
        assert!(
            o.tasks.iter().any(|r| {
                r.prefix == "agent" && r.text == "Write the screen" && r.marker == "checking"
            }),
            "{:?}",
            o.tasks
        );
        assert!(
            o.tasks
                .iter()
                .any(|r| r.prefix == "me" && r.text == "Ask Rolf")
        );
    }

    #[test]
    fn the_cost_section_shows_todays_pi_usage() {
        let fx = fixture();
        let (lane, _) = fx.lane(1);
        let ctx = fx.world.ctx();
        let now: jiff::Timestamp = crate::project::now().parse().unwrap();
        let record = thread::load(&fx.project, &lane).unwrap();
        let dir = ctx
            .root
            .join("pi/agent/sessions")
            .join(crate::talk::cost::session_dir_name(&record.worktree_path));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("s.jsonl"),
            format!(
                "{{\"timestamp\":\"{now}\",\"message\":{{\"usage\":{{\"totalTokens\":1200,\"cost\":{{\"total\":0.75}}}}}}}}"
            ),
        )
        .unwrap();
        let mut live = Live::default();
        live.refresh_slow(&ctx, &fx.project);
        let o = Overview::load(
            &fx.project,
            &Journal::default(),
            &Conversation::default(),
            &live,
        );
        let row = o
            .cost
            .iter()
            .find(|r| r.prefix == "today")
            .expect("a today row");
        assert!(row.text.contains("1.2k tokens"), "{:?}", row.text);
        assert!(row.text.contains("$0.75"), "{:?}", row.text);
    }
}
