//! What the agents already report about cost (LEAN U1).
//!
//! pi writes one `usage` object per assistant message in its session JSONL;
//! `herdr-pro` writes one line per Pro send to `usage.jsonl`. Nothing here
//! invents a number: a lane with no readable usage is shown as unknown, and a
//! Pro send has no tokens or money at all.
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

use serde_json::Value;

use crate::paths::Ctx;
use crate::project::Project;
use crate::thread;

/// Tokens, money in micro-dollars, whole minutes and send count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    pub(crate) tokens: u64,
    pub(crate) micros: u64,
    pub(crate) minutes: u64,
    pub(crate) runs: u64,
    pub(crate) unknown: bool,
}

impl Totals {
    pub(crate) fn is_empty(&self) -> bool {
        self.tokens == 0 && self.micros == 0 && self.minutes == 0 && self.runs == 0 && !self.unknown
    }

    fn merge(&mut self, other: &Totals) {
        self.tokens += other.tokens;
        self.micros += other.micros;
        self.minutes += other.minutes;
        self.runs += other.runs;
        self.unknown |= other.unknown;
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Cost {
    pub(crate) today: Totals,
    pub(crate) round: Option<(String, Totals)>,
    /// One entry per lane that could be read, for the record.
    pub(crate) lanes: Vec<(String, Totals)>,
    pub(crate) failed: bool,
}

/// The whole cost view. Filesystem only; no command runs.
pub(crate) fn load(ctx: &Ctx, project: &Project) -> Cost {
    let lanes = thread::list(project);
    let now = crate::project::now();
    let now_stamp = now.parse::<jiff::Timestamp>().ok();
    let today = local_date(&now);
    let sessions = ctx.root.join("pi/agent/sessions");
    let (pro, pro_failed) = pro_sends(&ctx.root.join("pro-bridge/usage.jsonl"), today.as_deref());
    let mut per_lane: BTreeMap<String, Totals> = BTreeMap::new();
    let mut today_totals = Totals::default();
    let mut failed = pro_failed;
    for t in &lanes {
        let (mut all, mut day, lane_failed) = if t.worktree_path.is_empty() {
            (Totals::default(), Totals::default(), false)
        } else {
            let dir = sessions.join(session_dir_name(&t.worktree_path));
            read_pi_sessions(&dir, today.as_deref())
        };
        failed |= lane_failed;

        // The old Pro packet path records a lane name but no tokens or money.
        // Attribute only names belonging to this project; never charge every
        // project for a shared usage file.
        // Only the full agent name carries the project slug. A bare `t-0001`
        // can exist in every project and is therefore not safe to attribute.
        if let Some((all_runs, day_runs)) = pro.get(&t.agent_name) {
            all.runs += all_runs;
            day.runs += day_runs;
            all.unknown = true;
            if day_runs > &0 {
                day.unknown = true;
            }
        }

        // A box session and a provider without usage still have one measured
        // fact: how long the lane has existed. Show that rather than zero.
        if all.tokens == 0 && all.micros == 0 {
            let (elapsed, elapsed_today) = lane_elapsed(t, now_stamp);
            all.minutes = all.minutes.max(elapsed);
            all.unknown = true;
            if let Some(minutes) = elapsed_today {
                day.minutes = day.minutes.max(minutes);
                day.unknown = true;
            }
        }
        today_totals.merge(&day);
        per_lane.insert(t.id.clone(), all);
    }
    let mut cost = Cost {
        today: today_totals,
        failed,
        ..Cost::default()
    };
    for (id, totals) in &per_lane {
        cost.lanes.push((id.clone(), totals.clone()));
    }
    if let Some((round, members)) = current_round(project) {
        let mut totals = Totals::default();
        for member in &members {
            match per_lane.get(member) {
                Some(t) => totals.merge(t),
                None => totals.unknown = true,
            }
        }
        if !members.is_empty() {
            cost.round = Some((round, totals));
        }
    }
    cost
}

/// pi names a session directory after the working directory: the path with
/// its leading slash removed and every separator replaced by a dash.
pub(crate) fn session_dir_name(cwd: &str) -> String {
    let trimmed = cwd.trim_start_matches('/').trim_end_matches('/');
    format!("--{}--", trimmed.replace(['/', '\\'], "-"))
}

fn local_date(at: &str) -> Option<String> {
    let ts: jiff::Timestamp = at.parse().ok()?;
    Some(
        ts.to_zoned(jiff::tz::TimeZone::system())
            .strftime("%Y-%m-%d")
            .to_string(),
    )
}

/// `(all-time, today, failed)`. Minutes are the span of the selected usage
/// lines, so a single message is zero minutes rather than an invented one.
fn read_pi_sessions(dir: &Path, today: Option<&str>) -> (Totals, Totals, bool) {
    let mut all = Totals::default();
    let mut day = Totals::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return (all, day, false),
        Err(_) => return (all, day, true),
    };
    let mut failed = false;
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(_) => {
                failed = true;
                continue;
            }
        };
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => {
                failed = true;
                continue;
            }
        };
        let mut saw_usage = false;
        let mut saw_today = false;
        let mut saw_today_usage = false;
        let mut all_span: Option<(jiff::Timestamp, jiff::Timestamp)> = None;
        let mut day_span: Option<(jiff::Timestamp, jiff::Timestamp)> = None;
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let at = value.get("timestamp").and_then(Value::as_str);
            let is_today = at.and_then(local_date).as_deref() == today;
            saw_today |= is_today;
            let Some(usage) = value
                .get("message")
                .and_then(|message| message.get("usage"))
            else {
                continue;
            };
            saw_usage = true;
            let tokens = usage
                .get("totalTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let micros = usage
                .get("cost")
                .and_then(|cost| cost.get("total"))
                .and_then(Value::as_f64)
                .map(|dollars| (dollars * 1_000_000.0).round() as u64)
                .unwrap_or(0);
            all.tokens += tokens;
            all.micros += micros;
            all.runs += 1;
            if let Some(stamp) = at.and_then(|at| at.parse::<jiff::Timestamp>().ok()) {
                all_span = Some(match all_span {
                    Some((low, high)) => (low.min(stamp), high.max(stamp)),
                    None => (stamp, stamp),
                });
            }
            if is_today {
                saw_today_usage = true;
                day.tokens += tokens;
                day.micros += micros;
                day.runs += 1;
                if let Some(stamp) = at.and_then(|at| at.parse::<jiff::Timestamp>().ok()) {
                    day_span = Some(match day_span {
                        Some((low, high)) => (low.min(stamp), high.max(stamp)),
                        None => (stamp, stamp),
                    });
                }
            }
        }
        if let Some((low, high)) = all_span {
            all.minutes += ((high.as_second() - low.as_second()).max(0) / 60) as u64;
        }
        if let Some((low, high)) = day_span {
            day.minutes += ((high.as_second() - low.as_second()).max(0) / 60) as u64;
        }
        if !saw_usage {
            all.unknown = true;
        }
        if saw_today && !saw_today_usage {
            day.unknown = true;
        }
    }
    (all, day, failed)
}

/// `usage.jsonl` records one Pro send per line: a time, a lane and a tag. No
/// tokens or money exist, so this counts all-time and today's sends by lane.
fn pro_sends(path: &Path, today: Option<&str>) -> (BTreeMap<String, (u64, u64)>, bool) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return (BTreeMap::new(), false);
        }
        Err(_) => return (BTreeMap::new(), true),
    };
    let mut runs = BTreeMap::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(lane) = value.get("lane").and_then(Value::as_str) else {
            continue;
        };
        let entry = runs.entry(lane.to_string()).or_insert((0, 0));
        entry.0 += 1;
        if value
            .get("ts")
            .and_then(Value::as_str)
            .and_then(local_date)
            .as_deref()
            == today
        {
            entry.1 += 1;
        }
    }
    (runs, false)
}

/// All-time elapsed minutes and, when the lane overlaps today, today's part.
fn lane_elapsed(lane: &thread::Thread, now: Option<jiff::Timestamp>) -> (u64, Option<u64>) {
    let (Some(start), Some(now)) = (lane.created.parse::<jiff::Timestamp>().ok(), now) else {
        return (0, None);
    };
    let end = if lane.status == thread::Status::Resolved {
        lane.updated.parse::<jiff::Timestamp>().ok()
    } else if matches!(lane.last_state.as_str(), "done" | "gone") {
        lane.last_state_change.parse::<jiff::Timestamp>().ok()
    } else {
        Some(now)
    };
    let Some(end) = end else {
        return (0, None);
    };
    let all = ((end.as_second() - start.as_second()).max(0) / 60) as u64;

    // Use real local midnight boundaries, including 23- and 25-hour daylight
    // saving days, then take this lane's overlap with that interval.
    let tz = jiff::tz::TimeZone::system();
    let date = now.to_zoned(tz.clone()).date();
    let Some(day_start) = date
        .to_zoned(tz.clone())
        .ok()
        .map(|zoned| zoned.timestamp())
    else {
        return (all, None);
    };
    let Some(day_end) = date
        .tomorrow()
        .ok()
        .and_then(|tomorrow| tomorrow.to_zoned(tz).ok())
        .map(|zoned| zoned.timestamp())
    else {
        return (all, None);
    };
    let low = start.max(day_start);
    let high = end.min(day_end);
    let today = (high >= low).then(|| ((high.as_second() - low.as_second()) / 60) as u64);
    (all, today)
}

/// The newest round that has not successfully merged, with its member lanes.
fn current_round(project: &Project) -> Option<(String, Vec<String>)> {
    let dir = crate::round::rounds_dir(project);
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut best: Option<crate::contracts::RoundRecord> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = toml::from_str::<crate::contracts::RoundRecord>(&text) else {
            continue;
        };
        if crate::threads::round_landed(project, &record.round) {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|current| record.opened >= current.opened)
        {
            best = Some(record);
        }
    }
    let record = best?;
    let members = record
        .manifest
        .members
        .iter()
        .map(|member| member.thread.clone())
        .collect();
    Some((record.round, members))
}

/// `1234567` -> `1.2m`; a small number stays whole.
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}m", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn format_money(micros: u64) -> String {
    format!("${:.2}", micros as f64 / 1_000_000.0)
}

pub(crate) fn format_totals(totals: &Totals) -> String {
    if totals.tokens == 0 && totals.micros == 0 {
        return format!("{} min, cost unknown", totals.minutes);
    }
    let mut text = format!(
        "{} tokens, {}, {} min",
        format_tokens(totals.tokens),
        format_money(totals.micros),
        totals.minutes
    );
    if totals.unknown {
        text.push_str(", some cost unknown");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_directory_mirrors_pi() {
        assert_eq!(
            session_dir_name("/home/ubuntu/projects/herdr-ade/.worktrees/t-0077"),
            "--home-ubuntu-projects-herdr-ade-.worktrees-t-0077--"
        );
    }

    #[test]
    fn pi_usage_sums_tokens_money_and_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            r#"{"timestamp":"2026-09-20T10:00:00Z","message":{"usage":{"totalTokens":100,"cost":{"total":0.5}}}}"#,
            r#"{"timestamp":"2026-09-20T10:10:00Z","message":{"usage":{"totalTokens":300,"cost":{"total":0.25}}}}"#,
            r#"{"timestamp":"2026-09-19T23:00:00Z","message":{"usage":{"totalTokens":999,"cost":{"total":9.0}}}}"#,
        ];
        std::fs::write(dir.path().join("a.jsonl"), lines.join("\n")).unwrap();
        let (all, today, failed) = read_pi_sessions(dir.path(), Some("2026-09-20"));
        assert!(!failed);
        assert_eq!(all.tokens, 1399);
        assert_eq!(all.micros, 9_750_000);
        assert_eq!(all.minutes, 670);
        assert_eq!(today.tokens, 400);
        assert_eq!(today.micros, 750_000);
        assert_eq!(today.minutes, 10);
        assert_eq!(today.runs, 2);
    }

    #[test]
    fn missing_usage_is_unknown_and_missing_directory_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jsonl"), r#"{"timestamp":"x"}"#).unwrap();
        let (all, _, failed) = read_pi_sessions(dir.path(), Some("2026-09-20"));
        assert!(all.unknown);
        assert!(!failed);
        let (empty, _, failed) = read_pi_sessions(&dir.path().join("nope"), Some("2026-09-20"));
        assert!(empty.is_empty());
        assert!(!failed);
    }

    #[test]
    fn a_lane_without_usage_reports_elapsed_time() {
        let start: jiff::Timestamp = "2026-09-20T12:00:00Z".parse().unwrap();
        let end: jiff::Timestamp = "2026-09-20T13:30:00Z".parse().unwrap();
        let lane = thread::Thread {
            created: start.to_string(),
            updated: end.to_string(),
            status: thread::Status::Resolved,
            ..thread::Thread::default()
        };
        let (all, day) = lane_elapsed(&lane, Some(end));
        assert_eq!(all, 90);
        assert_eq!(day, Some(90));
    }
}
