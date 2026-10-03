//! One durable pile review per repository. Records are the authority for
//! landing; git is read only while starting or advancing an actual review.
//! The repository operation lock serializes CLI/ticker effects across projects.
use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::repo::Git;
use crate::thread::{self, Status, Thread};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Preparing,
    Reviewing,
    Landing,
    Cancelling,
    Complete,
    Cancelled,
    Rejected,
}
impl Phase {
    pub(crate) fn closed(&self) -> bool {
        matches!(self, Self::Complete | Self::Cancelled | Self::Rejected)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Member {
    pub thread: String,
    pub attempt: u32,
    pub event: String,
    pub sha: String,
    pub branch: String,
    pub artifact: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct GateRun {
    pub command: String,
    pub exit: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Verdict {
    pub verdict: String,
    pub review: String,
    pub candidate: String,
    #[serde(default)]
    pub without: BTreeMap<String, String>,
    /// Historical declarations remain readable, but are never mechanical proof.
    /// New checked verdicts replace these with ADE-observed results.
    #[serde(default)]
    pub gates: Vec<GateRun>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gates_note: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Review {
    pub id: String,
    pub repo: String,
    pub integration: String,
    pub base: String,
    pub candidate_branch: String,
    pub members: Vec<Member>,
    pub gates: Vec<project::Gate>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gates_note: String,
    pub selected_gates: Vec<project::Gate>,
    pub reviewer: Option<String>,
    pub phase: Phase,
    pub verdict: Option<Verdict>,
    pub verdict_event: String,
    pub reviewer_after: String,
    pub checked_event: String,
    pub retry_attempt: Option<u32>,
    pub retry_generation: u32,
    pub moved: u32,
    pub refresh_tip: Option<String>,
    pub push_remote: Option<String>,
    pub install_required: bool,
    pub fast_forward: bool,
    pub push: bool,
    pub install: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub install_result: String,
    pub close: bool,
    pub prune: bool,
    pub attention: String,
    /// First observed idle/blocked stretch with no fresh reviewer seal.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) no_verdict_since: String,
    /// Review-owned wake-ups survive resolution of the reviewer and members.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) notices: Vec<crate::steps::Notice>,
}

impl Review {
    fn command(&self, project: &Project, action: &str) -> String {
        format!(
            "ha review {action} {} --repo {}",
            crate::remote::quote(&project.slug),
            crate::remote::quote(&self.repo)
        )
    }

    pub(crate) fn landing_summary(&self) -> String {
        let merged = if self.fast_forward {
            "merged"
        } else {
            "not merged"
        };
        let publication = if self.push_remote.as_deref().is_none_or(str::is_empty) {
            "no remote configured"
        } else if self.push {
            "publication verified"
        } else {
            "publication pending"
        };
        let install = if !self.install_required {
            "install not required"
        } else if !self.install_result.is_empty() {
            &self.install_result
        } else if self.install {
            "install completed"
        } else {
            "install pending"
        };
        let continuation = if !self.install_result.starts_with("REGRESSION")
            && (matches!(self.phase, Phase::Landing | Phase::Cancelling)
                || (!self.phase.closed() && self.attention.is_empty()))
        {
            "; continues automatically"
        } else {
            ""
        };
        format!("{merged}; {publication}; {install}{continuation}")
    }

    pub(crate) fn gates_summary(&self) -> String {
        if self.gates_note.is_empty() {
            String::new()
        } else {
            format!(" — {}", self.gates_note)
        }
    }
}

pub(crate) fn dir(project: &Project) -> PathBuf {
    project.state_dir().join("reviews")
}
pub(crate) fn path(project: &Project, id: &str) -> PathBuf {
    dir(project).join(format!("{id}.toml"))
}
pub(crate) fn load(project: &Project, id: &str) -> Result<Review> {
    if !id
        .strip_prefix("review-")
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        bail!("invalid review id: {id}");
    }
    let record: Review = toml::from_str(&std::fs::read_to_string(path(project, id))?)?;
    if record.id != id {
        bail!("review identity mismatch: {id}");
    }
    Ok(record)
}
pub(crate) fn save(project: &Project, record: &Review) -> Result<()> {
    std::fs::create_dir_all(dir(project))?;
    project::write_atomic(
        &path(project, &record.id),
        toml::to_string(record)?.as_bytes(),
    )?;
    // write_atomic synced reviews/; keep this sync for its creation in .state/.
    std::fs::File::open(project.state_dir())?.sync_all()?;
    Ok(())
}
pub(crate) fn mark_notice_submitted(project: &Project, id: &str, line: &str) -> Result<()> {
    let _lock = project.lock()?;
    let mut review = load(project, id)?;
    if let Some(notice) = review.notices.iter_mut().find(|n| n.line == line) {
        notice.submitted = true;
        save(project, &review)?;
    }
    Ok(())
}

thread_local! {
    static TICKER_LISTS: std::cell::RefCell<Option<crate::record_cache::Records<Review>>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn set_cache(enabled: bool) {
    TICKER_LISTS.with(|cache| *cache.borrow_mut() = enabled.then(Default::default));
}

pub(crate) fn list(project: &Project) -> Result<Vec<Review>> {
    if let Some(cached) = TICKER_LISTS.with(|cache| {
        cache
            .borrow_mut()
            .as_mut()
            .map(|cache| cache.read(dir(project), |id| load(project, id)))
    }) {
        let (rows, errors) = cached;
        if let Some(error) = errors.into_iter().next() {
            return Err(error);
        }
        let mut records = rows.as_ref().clone();
        records.sort_by_key(|r| {
            r.id.trim_start_matches("review-")
                .parse::<u64>()
                .unwrap_or(0)
        });
        return Ok(records);
    }
    let entries = match std::fs::read_dir(dir(project)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.path().extension().is_some_and(|s| s == "toml") {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("review filename is not UTF-8"))?;
            records.push(load(project, name.trim_end_matches(".toml"))?);
        }
    }
    records.sort_by_key(|r| {
        r.id.trim_start_matches("review-")
            .parse::<u64>()
            .unwrap_or(0)
    });
    Ok(records)
}
fn reviewer_ids(project: &Project) -> Result<std::collections::BTreeSet<String>> {
    let reviews = list(project)?;
    let mut ids: std::collections::BTreeSet<String> = reviews
        .iter()
        .filter_map(|review| review.reviewer.clone())
        .collect();
    // Rounds predate pile reviews. Their reviewer lanes must not become pile
    // members merely because the old round records are no longer advanced.
    let rounds = project.state_dir().join("rounds");
    if rounds.exists() {
        for entry in std::fs::read_dir(rounds)? {
            let path = entry?.path();
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }
            #[derive(Deserialize)]
            struct RoundReviewer {
                reviewer: Option<String>,
            }
            // Historical round files can be malformed; they are never written
            // by the pile reader, and an unrelated bad file must not stop it.
            let Some(record) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| toml::from_str::<RoundReviewer>(&text).ok())
            else {
                continue;
            };
            if let Some(id) = record.reviewer.filter(|id| !id.is_empty()) {
                ids.insert(id);
            }
        }
    }
    // Allocation can crash before the review record binds its reviewer.
    // Only the review start path writes review_id on a lane.
    for lane in thread::list(project) {
        if reviews.iter().any(|review| review.id == lane.review_id) {
            ids.insert(lane.id);
        }
    }
    Ok(ids)
}
fn queue_notice(review: &mut Review, line: String) {
    if !review.notices.iter().any(|n| n.line == line) {
        review.notices.push(crate::steps::Notice {
            line,
            submitted: false,
        });
    }
}

fn needs_coordinator(project: &Project, review: &mut Review, reason: &str) -> Result<()> {
    let attention = crate::steps::short_error(reason);
    review.attention = reason.into();
    queue_notice(
        review,
        format!(
            "REVIEW {} needs attention: {attention} — next: {}",
            review.id,
            review.command(project, "retry")
        ),
    );
    save(project, review)
}

fn reset_no_verdict(project: &Project, review: &mut Review) -> Result<()> {
    if !review.no_verdict_since.is_empty() {
        review.no_verdict_since.clear();
        review
            .notices
            .retain(|n| n.submitted || !n.line.contains("has no verdict since"));
        save(project, review)?;
    }
    Ok(())
}

fn watch_no_verdict(ctx: &Ctx, project: &Project, review: &mut Review) -> Result<()> {
    if review.phase != Phase::Reviewing {
        return reset_no_verdict(project, review);
    }
    let Some(id) = review.reviewer.as_deref() else {
        return Ok(());
    };
    let lane = thread::load(project, id)?;
    let events = crate::events::checked_for_thread(project, id)?;
    let fresh = crate::events::latest_done_event(&events, id, lane.attempt.max(1))
        .is_some_and(|e| e.id != review.reviewer_after && e.id != review.checked_event);
    if fresh
        || lane.status != Status::Open
        || lane.pane_id.is_empty()
        || lane.prompt_pending
        || lane.recovery_pending
        || crate::threads::follow_up_pending_for_seal(
            &lane,
            crate::events::latest_done_event(&events, id, lane.attempt.max(1)),
        )
    {
        return reset_no_verdict(project, review);
    }
    // An absent process belongs to the normal lane failure recovery, not
    // this idle-review warning. Unknown intervals cannot count toward idle.
    let state = if lane.is_remote() {
        if lane.last_observed.is_empty()
            || !(0..=45).contains(&thread::seconds_since(
                &lane.last_observed,
                jiff::Timestamp::now(),
            ))
        {
            return reset_no_verdict(project, review);
        }
        lane.last_state.as_str()
    } else {
        let socket = project.coordinator().map(|c| c.socket).unwrap_or_default();
        let herdr = crate::herdr::Herdr::new(ctx.env.herdr_bin(), &socket, ctx.runner);
        let agents = herdr.agent_list()?;
        let Some(agent) = agents.iter().find(|a| thread::agent_matches(&lane, a)) else {
            return reset_no_verdict(project, review);
        };
        // Keep the borrowed status alive for the rest of this observation.
        return watch_no_verdict_state(ctx, project, review, &lane, &agent.agent_status);
    };
    watch_no_verdict_state(ctx, project, review, &lane, state)
}

fn watch_no_verdict_state(
    ctx: &Ctx,
    project: &Project,
    review: &mut Review,
    lane: &Thread,
    state: &str,
) -> Result<()> {
    if state != "idle" && state != "blocked" {
        if !review.no_verdict_since.is_empty() {
            review.no_verdict_since.clear();
            save(project, review)?;
        }
        return Ok(());
    }
    if review.no_verdict_since.is_empty() {
        review.no_verdict_since = project::now();
        save(project, review)?;
        return Ok(());
    }
    let secs = ctx
        .env
        .var("HERDR_ADE_REVIEW_IDLE_SECS")
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(600);
    if thread::seconds_since(&review.no_verdict_since, jiff::Timestamp::now()) >= secs
        && !review.notices.iter().any(|n| {
            n.line
                .contains(&format!("has no verdict since {}", review.no_verdict_since))
        })
        && !review
            .notices
            .iter()
            .any(|n| !n.submitted && n.line.contains("needs attention"))
    {
        let line = format!(
            "REVIEW {} has no verdict since {} (reviewer {} {}); no merge attempted — next: {}",
            review.id,
            review.no_verdict_since,
            lane.id,
            state,
            review.command(project, "retry")
        );
        queue_notice(review, line);
        save(project, review)?;
    }
    Ok(())
}

fn operation_lock(ctx: &Ctx, repo: &str) -> Result<std::fs::File> {
    let file = lock_file(ctx, repo)?;
    file.lock()?;
    Ok(file)
}
pub(crate) fn try_operation_lock(ctx: &Ctx, repo: &str) -> Result<Option<std::fs::File>> {
    let file = lock_file(ctx, repo)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}
fn lock_file(ctx: &Ctx, repo: &str) -> Result<std::fs::File> {
    let path = lock_path(ctx, repo);
    std::fs::create_dir_all(path.parent().expect("lock path has a parent"))?;
    Ok(std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?)
}
fn lock_path(ctx: &Ctx, repo: &str) -> PathBuf {
    let repo = repo_identity(repo);
    ctx.root
        .join(".review-locks")
        .join(thread::sha256_hex(repo.to_string_lossy().as_bytes()))
}
/// Read Git's worktree pointers without invoking Git on idle ticker passes.
fn repo_identity(repo: &str) -> PathBuf {
    let root = std::fs::canonicalize(repo).unwrap_or_else(|_| PathBuf::from(repo));
    let dotgit = root.join(".git");
    let gitdir = if dotgit.is_dir() {
        dotgit
    } else if let Ok(text) = std::fs::read_to_string(&dotgit) {
        let Some(path) = text.trim().strip_prefix("gitdir: ") else {
            return root;
        };
        root.join(path)
    } else {
        return root;
    };
    let common = std::fs::read_to_string(gitdir.join("commondir"))
        .map(|text| gitdir.join(text.trim()))
        .unwrap_or(gitdir);
    std::fs::canonicalize(&common).unwrap_or(common)
}
fn same_repo(a: &str, b: &str) -> bool {
    a == b || repo_identity(a) == repo_identity(b)
}
#[derive(Debug)]
struct UnconfiguredRepo;
impl std::fmt::Display for UnconfiguredRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("review repository is not configured")
    }
}
impl std::error::Error for UnconfiguredRepo {}

fn repository(ctx: &Ctx, project: &Project, requested: Option<&str>) -> Result<project::Repo> {
    let mut rows = project.read_project_md()?.0.repos;
    let primary = (rows.len() == 1).then(|| rows[0].path.clone());
    for row in crate::harness::repos(&ctx.config_dir)? {
        if !rows.iter().any(|r| same_repo(&r.path, &row.path)) {
            rows.push(row);
        }
    }
    match requested {
        Some(repo) => rows
            .into_iter()
            .find(|row| same_repo(&row.path, repo))
            .ok_or_else(|| UnconfiguredRepo.into()),
        None => {
            let events = crate::events::checked(project)?;
            let reviewers = reviewer_ids(project)?;
            let mut repos: std::collections::BTreeSet<_> = thread::list(project)
                .into_iter()
                .filter(|t| {
                    t.status != Status::Resolved
                        && !reviewers.contains(&t.id)
                        && t.merged_sha.is_empty()
                })
                .filter(|t| sealed(&events, t).is_some_and(|e| changes(t, e) != Some(false)))
                .map(|t| t.repo)
                .filter(|r| !r.is_empty())
                .collect();
            repos.extend(
                list(project)?
                    .into_iter()
                    .filter(|r| !r.phase.closed())
                    .map(|r| r.repo),
            );
            let selected = if repos.len() == 1 {
                repos.pop_first()
            } else if repos.is_empty() {
                primary.or_else(|| (rows.len() == 1).then(|| rows[0].path.clone()))
            } else {
                None
            };
            selected
                .and_then(|repo| rows.into_iter().find(|row| same_repo(&row.path, &repo)))
                .context("review repository is ambiguous; pass --repo <path>")
        }
    }
}
fn active_for_repo(ctx: &Ctx, repo: &str) -> Result<Option<(Project, Review)>> {
    for slug in project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        for review in list(&project)? {
            if !review.phase.closed() && same_repo(&review.repo, repo) {
                return Ok(Some((project, review)));
            }
        }
    }
    Ok(None)
}

/// A follow-up invalidates its previous completion until a fresh seal arrives.
pub(crate) fn sealed<'a>(
    events: &'a [crate::contracts::Event],
    lane: &Thread,
) -> Option<&'a crate::contracts::Event> {
    let event = crate::events::latest_event(events, &lane.id, lane.attempt.max(1))?;
    event.payload.done.as_ref()?;
    if lane.review_after == event.id
        || crate::threads::follow_up_pending_for_seal(lane, Some(event))
    {
        return None;
    }
    Some(event)
}
pub(crate) fn lane_review(project: &Project, lane: &Thread) -> Result<Option<Review>> {
    Ok(list(project)?.into_iter().rev().find(|r| {
        r.fast_forward
            && r.members.iter().any(|m| {
                m.thread == lane.id && m.attempt == lane.attempt.max(1) && !excluded(r, &m.thread)
            })
    }))
}
fn excluded(review: &Review, lane: &str) -> bool {
    review
        .verdict
        .as_ref()
        .is_some_and(|v| v.without.contains_key(lane))
}
pub(crate) fn lane_done(
    project: &Project,
    lane: &Thread,
    events: &[crate::contracts::Event],
) -> bool {
    if let Ok(Some(review)) = lane_review(project, lane) {
        return !review.install_required || review.install;
    }
    if !lane.merged_sha.is_empty() && lane.merged_review.is_empty() {
        return !lane.historical_install_required || !lane.installed_sha.is_empty();
    }
    sealed(events, lane).is_some_and(|e| changes(lane, e) == Some(false))
}
pub(crate) fn changes(lane: &Thread, event: &crate::contracts::Event) -> Option<bool> {
    let sealed = event.payload.done.as_ref()?;
    if lane.changes_seal == event.id {
        lane.has_changes.or(sealed.has_changes)
    } else {
        sealed.has_changes
    }
}

/// No git here: used by the ticker to decide whether a review can start.
fn pending(
    project: &Project,
    repo: &str,
    events: &[crate::contracts::Event],
    reviewers: &std::collections::BTreeSet<String>,
) -> Vec<Thread> {
    pending_from(thread::list_live(project), repo, events, reviewers)
}
fn pending_from(
    lanes: Vec<Thread>,
    repo: &str,
    events: &[crate::contracts::Event],
    reviewers: &std::collections::BTreeSet<String>,
) -> Vec<Thread> {
    lanes
        .into_iter()
        .filter(|t| {
            t.status != Status::Resolved
                && !reviewers.contains(&t.id)
                && same_repo(&t.repo, repo)
                && t.merged_sha.is_empty()
        })
        .filter(|t| sealed(events, t).is_some_and(|e| changes(t, e) != Some(false)))
        .collect()
}
fn selected(gates: &[project::Gate], files: &[String]) -> Vec<project::Gate> {
    gates
        .iter()
        .filter(|gate| {
            gate.paths.as_ref().is_none_or(|paths| {
                paths.iter().any(|pattern| {
                    files
                        .iter()
                        .any(|file| crate::gate_paths::matches(pattern, file))
                })
            })
        })
        .cloned()
        .collect()
}
fn files(git: &Git<'_>, base: &str, tip: &str) -> Result<Vec<String>> {
    Ok(git
        .run(&["diff", "--name-only", base, tip, "--"])?
        .lines()
        .map(str::to_owned)
        .collect())
}
fn member_files(git: &Git<'_>, base: &str, tip: &str) -> Result<Vec<String>> {
    Ok(git
        .run(&["diff", "--name-only", &format!("{base}...{tip}"), "--"])?
        .lines()
        .map(str::to_owned)
        .collect())
}

pub(crate) fn start(ctx: &Ctx, slug: &str, repo: Option<&str>) -> Result<Option<Review>> {
    let project = Project::load(&ctx.root, slug)?;
    let row = repository(ctx, &project, repo)?;
    let _lock = operation_lock(ctx, &row.path)?;
    // Explicit opt-in is project-wide, not inferred from any old round record.
    project::write_atomic(&project.state_dir().join("reviews-enabled"), b"enabled\n")?;
    if let Some((_, review)) = active_for_repo(ctx, &row.path)? {
        return Ok(Some(review));
    }
    start_locked(ctx, &project, row)
}
fn start_locked(ctx: &Ctx, project: &Project, row: project::Repo) -> Result<Option<Review>> {
    let events = crate::events::checked(project)?;
    let pile = pending(project, &row.path, &events, &reviewer_ids(project)?);
    if pile.is_empty() {
        return Ok(None);
    }
    let harness = crate::harness::repos(&ctx.config_dir)?;
    let harness_row = harness.iter().find(|r| same_repo(&r.path, &row.path));
    let declared_gates = harness_row
        .and_then(|r| r.gates.as_ref())
        .or(row.gates.as_ref());
    if harness_row.is_some() && declared_gates.is_none() {
        bail!(
            "harness_gates_missing: {} has no gates; add gates to its [harness] row in config.toml",
            row.path
        );
    }
    let gates_note = if declared_gates.is_none() {
        "no gates declared".to_owned()
    } else {
        String::new()
    };
    let gates = declared_gates.cloned().unwrap_or_default();
    for gate in &gates {
        if let Some(paths) = &gate.paths {
            for path in paths {
                crate::gate_paths::validate(path)?;
            }
        }
    }
    let git = Git::new(ctx.runner, &row.path);
    let integration = row
        .branch
        .clone()
        .map(Ok)
        .unwrap_or_else(|| crate::git::symbolic_head(ctx.runner, &row.path))?;
    let base = git
        .branch_head(&integration)?
        .context("integration branch is missing")?;
    let mut members = Vec::new();
    let mut changed = Vec::new();
    for lane in pile {
        let event = sealed(&events, &lane).context("pile changed while preparing review")?;
        let done = event.payload.done.as_ref().expect("sealed done");
        if lane.is_remote() {
            let (settings, _) = project.read_project_md()?;
            let profile = crate::remote::machine_profile(
                ctx.runner,
                &ctx.env.herdr_bin(),
                &ctx.config_dir,
                lane.machine_route(),
            )?;
            let (_, url) = crate::threads::box_repo_row(
                &ctx.config_dir,
                &settings,
                &profile.label,
                &lane.repo,
            )?;
            git.run(&[
                "fetch",
                &url,
                &format!(
                    "refs/heads/{}",
                    done.published_ref.as_deref().unwrap_or(&lane.branch)
                ),
            ])?;
            let fetched = git.run(&["rev-parse", "FETCH_HEAD"])?;
            if fetched != done.sha {
                bail!("published lane {} moved since its seal", lane.id);
            }
        }
        let has_changes = if let Some(value) = changes(&lane, event) {
            value
        } else {
            if lane.base.is_empty() {
                if git.is_ancestor(&done.sha, &base)? {
                    thread::update(project, &lane.id, |t| t.merged_sha = base.clone())?;
                    continue;
                }
                bail!(
                    "{} has no recorded base; cannot classify its old seal",
                    lane.id
                );
            }
            git.trees_differ(&lane.base, &done.sha)?
        };
        // Historical seals stay immutable. Cache their classification on the lane.
        thread::update(project, &lane.id, |t| {
            t.changes_seal = event.id.clone();
            t.has_changes = Some(has_changes);
        })?;
        if !has_changes {
            continue;
        }
        if git.is_ancestor(&done.sha, &base)? {
            thread::update(project, &lane.id, |t| t.merged_sha = base.clone())?;
            continue;
        }
        changed.extend(member_files(&git, &base, &done.sha)?);
        members.push(Member {
            thread: lane.id,
            attempt: event.attempt,
            event: event.id.clone(),
            sha: done.sha.clone(),
            branch: lane.branch,
            artifact: done.artifact.clone(),
        });
    }
    if members.is_empty() {
        project::refresh_page(project)?;
        return Ok(None);
    }
    // Different repositories can start concurrently in the same project.
    // Allocate the id and write its intent under the project lock.
    let allocation = project.lock()?;
    let next = list(project)?
        .iter()
        .filter_map(|r| r.id.strip_prefix("review-")?.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let id = format!("review-{next}");
    let mut review = Review {
        candidate_branch: format!("review/{}/{id}", project.slug),
        id,
        repo: row.path.clone(),
        integration,
        base,
        members,
        selected_gates: selected(&gates, &changed),
        gates,
        gates_note,
        reviewer: None,
        phase: Phase::Preparing,
        verdict: None,
        verdict_event: String::new(),
        reviewer_after: String::new(),
        checked_event: String::new(),
        retry_attempt: None,
        retry_generation: 0,
        moved: 0,
        refresh_tip: None,
        push_remote: row.push_remote.or(row.publish_url),
        install_required: harness_row.is_some(),
        fast_forward: false,
        push: false,
        install: false,
        install_result: String::new(),
        close: false,
        prune: false,
        attention: String::new(),
        no_verdict_since: String::new(),
        notices: Vec::new(),
    };
    save(project, &review)?;
    drop(allocation);
    advance(ctx, project, &mut review)?;
    project::refresh_page(project)?;
    Ok(Some(review))
}
fn task(project: &Project, review: &Review) -> String {
    let mut out = format!(
        "Run `ha skill reviewer`. Review the whole repository pile {}. Your branch starts from candidate `{}` on integration base `{}`. Merge every included SHA below (some may already be merged); resolve conflicts, fix small issues, and judge each original acceptance criterion. ADE runs the path-selected gates on your sealed candidate, on your machine, before landing. Do not push or install.\n\n",
        review.id, review.candidate_branch, review.base
    );
    for member in &review.members {
        out.push_str(&format!(
            "- {}: `{}`, report `{}`\n",
            member.thread,
            member.sha,
            crate::events::artifact_path(project, &member.artifact).display()
        ));
        out.push_str(&member_packet(project, member));
    }
    out.push_str("\nGate policy (select by all paths changed from the integration base, including your fixes):\n");
    out.push_str(
        &toml::to_string(&GatePolicy {
            gates: review.gates.clone(),
        })
        .expect("gate serialization"),
    );
    if !review.gates_note.is_empty() {
        out.push_str(&format!("\n{}\n", review.gates_note));
    }
    out.push_str("\nSelected gates for this pile:\n");
    for gate in &review.selected_gates {
        out.push_str(&format!(
            "- `{}`; environment {:?}\n",
            gate.command, gate.env
        ));
    }
    out.push_str(&format!("\nWrite a report with TOML front matter:\n+++\nreview = \"{}\"\nverdict = \"MERGE\" # or REJECT\ncandidate = \"<your exact HEAD>\"\n# Gate execution receipts are recorded by ADE, not declared here.\n# Optional: without = {{ t-0001 = \"one-line reason\" }}\n\n# Repeat for each included task's required criterion:\n[[acceptance]]\nthread = \"<member thread>\"\nevent = \"<member seal from the packet>\"\ncriterion = 1\ncondition = \"<exact original acceptance condition>\"\nestablished = true # false means not established; fix or exclude this member\nevidence = \"<durable artifact and behavior/journey references>\"\n+++\n\nIf excluding lanes, rebuild from the integration base without those lanes; their commits must not remain ancestors of your candidate. For every original required criterion, cite durable artifact/behavior evidence or say not established; gates alone do not prove semantics. Include findings and actual journey evidence. Commit repository changes if any, leave runtime deliverables untracked, then `ha done`. Output in another repository belongs in the report.\n", review.id));
    out
}
/// This text becomes part of the reviewer's own immutable launch brief. Never
/// substitute today's task wording for the intent frozen at member launch.
fn member_packet(project: &Project, member: &Member) -> String {
    let mut out = format!(
        "  Durable evidence: seal `{}` (attempt {}), report artifact `{}`.\n",
        member.event, member.attempt, member.artifact
    );
    let brief = thread::load(project, &member.thread).and_then(|lane| {
        let hash = &lane.launch.brief_hash;
        let bytes = thread::artifact(project, hash)?;
        Ok((hash.clone(), String::from_utf8(bytes)?))
    });
    match brief {
        Ok((hash, text)) => out.push_str(&format!(
            "\n### {}: original frozen brief and acceptance\nBrief artifact `{hash}`\n\n{text}\n\n### End original {} brief\n",
            member.thread, member.thread
        )),
        Err(error) => out.push_str(&format!(
            "  Original brief/acceptance: not established ({error}). Do not infer intent from the report.\n"
        )),
    }
    let (tasks, errors) = crate::task::list_with_errors(project);
    for task in tasks
        .iter()
        .filter(|task| task.attempts.contains(&member.thread))
    {
        out.push_str(&format!(
            "  Task evidence record `{}`; requests: {}\n",
            task.id,
            task.authority.join(", ")
        ));
        if let Some(judgment) = &task.acceptance_review {
            out.push_str(&format!(
                "  Coordinator judgment snapshot: {} at {}; seal {}, report artifact {}\n",
                judgment.coordinator, judgment.at, judgment.event, judgment.artifact
            ));
            for row in &judgment.criteria {
                out.push_str(&format!(
                    "  Criterion {} ({}): {} — {}\n",
                    row.criterion,
                    row.condition,
                    if row.established && !row.evidence.trim().is_empty() {
                        "established"
                    } else {
                        "not established"
                    },
                    row.evidence
                ));
            }
        }
        for evidence in &task.installed {
            out.push_str(&format!(
                "  Installation (not acceptance): {} at {}; machine {:?}, build {:?}\n",
                evidence.command, evidence.at, evidence.machine, evidence.build
            ));
        }
    }
    for error in errors {
        out.push_str(&format!("  Task evidence: not established ({error}).\n"));
    }
    out
}

#[derive(Serialize)]
struct GatePolicy {
    gates: Vec<project::Gate>,
}
fn prepare(ctx: &Ctx, project: &Project, review: &mut Review) -> Result<()> {
    let git = Git::new(ctx.runner, &review.repo);
    if git.branch_head(&review.candidate_branch)?.is_none() {
        let _lock = crate::git::lock(ctx.runner, &review.repo)?;
        git.run(&["branch", &review.candidate_branch, &review.base])?;
    }
    // Pure merges do not leave a conflicted index. Stop at the first conflict;
    // the one reviewer completes this same candidate in its checkout.
    let mut head = git
        .branch_head(&review.candidate_branch)?
        .context("candidate missing")?;
    for member in &review.members {
        if git.is_ancestor(&member.sha, &head)? {
            continue;
        }
        let tree = match git.merge_tree(&head, &member.sha) {
            Ok(tree) => tree,
            Err(e) if e.to_string().starts_with("merge_conflict:") => break,
            Err(e) => return Err(e),
        };
        let merged = git.commit_tree(
            &tree,
            &head,
            &member.sha,
            &format!("Pile {}: {}", review.id, member.thread),
        )?;
        let _lock = crate::git::lock(ctx.runner, &review.repo)?;
        git.run(&[
            "update-ref",
            &format!("refs/heads/{}", review.candidate_branch),
            &merged,
            &head,
        ])?;
        head = merged;
    }
    if review.reviewer.is_none() {
        // The identity is written into the thread before placement. Recover a
        // crash after allocation without launching a second reviewer.
        let unbound: Vec<_> = thread::list(project)
            .into_iter()
            .filter(|t| t.review_id == review.id && t.status != Status::Resolved)
            .collect();
        if unbound.len() > 1 {
            return Err(crate::refusal::error(
                format!("multiple reviewers for {}", review.id),
                "wait for the coordinator to identify the valid reviewer; no replacement is safe yet",
            ));
        }
        let reviewer = if let Some(t) = unbound.first() {
            t.clone()
        } else {
            crate::threads::start(
                ctx,
                &project.slug,
                crate::threads::StartArgs {
                    title: format!("Review pile {}", review.id),
                    repo: Some(review.repo.clone()),
                    machine: None,
                    base: Some(review.candidate_branch.clone()),
                    task: task(project, review),
                    workflow: Some("reviewer".into()),
                    recipe: None,
                    task_id: String::new(),
                    review_id: review.id.clone(),
                    attach: Vec::new(),
                    paths: Vec::new(),
                },
            )?
        };
        review.reviewer = Some(reviewer.id);
    }
    review.phase = Phase::Reviewing;
    save(project, review)
}
fn verdict(
    ctx: &Ctx,
    project: &Project,
    review: &Review,
    event: &crate::contracts::Event,
    git: &Git<'_>,
) -> Result<Verdict> {
    let done = event
        .payload
        .done
        .as_ref()
        .context("reviewer has no seal")?;
    let text = String::from_utf8(thread::artifact(project, &done.artifact)?)?;
    let front = text
        .strip_prefix("+++\n")
        .and_then(|s| s.split_once("\n+++").map(|(front, _)| front))
        .context("review report needs TOML front matter")?;
    let mut verdict: Verdict = toml::from_str(front)?;
    verdict.gates_note = review.gates_note.clone();
    if verdict.review != review.id || verdict.candidate != done.sha {
        bail!("verdict identity or candidate differs from sealed reviewer HEAD");
    }
    if !matches!(verdict.verdict.as_str(), "MERGE" | "REJECT") {
        bail!("verdict must be MERGE or REJECT");
    }
    for (lane, reason) in &verdict.without {
        if !review.members.iter().any(|m| &m.thread == lane)
            || reason.trim().is_empty()
            || reason.lines().count() != 1
        {
            bail!("invalid excluded lane or reason: {lane}");
        }
    }
    if verdict.verdict == "REJECT" {
        return Ok(verdict);
    }
    if verdict.without.len() == review.members.len() {
        bail!("use REJECT when excluding the whole pile");
    }
    if !git.is_ancestor(&review.base, &verdict.candidate)? {
        bail!("candidate does not contain integration base");
    }
    for member in &review.members {
        let contains = git.is_ancestor(&member.sha, &verdict.candidate)?;
        if contains == verdict.without.contains_key(&member.thread) {
            bail!(
                "candidate ancestry disagrees with inclusion of {}",
                member.thread
            );
        }
    }
    let criteria = crate::task::report_criteria(&text)?;
    let (tasks, errors) = crate::task::list_with_errors(project);
    if !errors.is_empty() {
        bail!("acceptance not established: unreadable task records");
    }
    for member in &review.members {
        if verdict.without.contains_key(&member.thread) {
            continue;
        }
        for task in tasks
            .iter()
            .filter(|task| task.attempts.last() == Some(&member.thread))
        {
            if !crate::task::criteria_established(task, &member.thread, &member.event, &criteria) {
                bail!(
                    "acceptance not established: {} needs artifact/behavior evidence for every required criterion; exclude it or get a fresh judgment",
                    member.thread
                );
            }
        }
    }
    // Self-reported exits are discarded. Semantics still belong to the one
    // reviewer; mechanical proof comes only from the existing execution path.
    let mut execution = review.clone();
    execution.verdict_event = event.id.clone();
    verdict.gates = observed_gates(ctx, project, &execution, &verdict.candidate, git)?;
    Ok(verdict)
}
/// ADE-owned execution evidence, separate from the reviewer-authored report.
/// A nonzero checker, lost transport, timeout or incomplete log establishes no
/// acceptance result; none is classified as a failed implementation.
#[derive(Debug, Serialize, Deserialize)]
struct GateReceipt {
    review: String,
    #[serde(default)]
    event: String,
    base: String,
    candidate: String,
    gate: project::Gate,
    machine: String,
    #[serde(default)]
    target: Option<String>,
    cwd: String,
    environment: BTreeMap<String, String>,
    started: String,
    exit: Option<i32>,
    timed_out: bool,
    complete: bool,
    error: String,
    stdout: String,
    stderr: String,
    stdout_hash: String,
    stderr_hash: String,
}

#[derive(Serialize)]
struct GateSelection<'a> {
    candidate: &'a str,
    base: &'a str,
    note: &'a str,
    changed_paths: Vec<String>,
    selected: Vec<project::Gate>,
    not_selected: Vec<project::Gate>,
}

fn log_hash(path: &std::path::Path) -> Result<String> {
    use sha2::Digest;
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut digest = sha2::Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn candidate_guard(candidate: &str) -> String {
    format!(
        "ade_candidate() {{ head=$(git rev-parse HEAD) && status=$(git status --porcelain) && [ \"$head\" = {} ] && [ -z \"$status\" ]; }}\nade_candidate || {{ echo 'candidate changed or dirty; result not established' >&2; exit 125; }}\n",
        crate::remote::quote(candidate)
    )
}

fn gate_command(
    gate: &project::Gate,
    candidate: &str,
    cwd: &str,
    environment: &BTreeMap<String, String>,
    target: Option<&str>,
) -> crate::runner::Cmd {
    use crate::runner::Cmd;
    let guard = candidate_guard(candidate);
    let script = format!(
        "cd {} || exit 125\n{guard}sh -c {}\nresult=$?\nade_candidate || {{ echo 'gate changed candidate; result not established' >&2; exit 125; }}\nexit \"$result\"",
        crate::remote::quote(cwd),
        crate::remote::quote(&gate.command)
    );
    // Capture uses the shared Runner, including process-group timeout and full
    // streamed logs. SSH transports those same streams; no second executor.
    let timeout = std::time::Duration::from_secs(1800);
    if let Some(target) = target {
        let assignments = environment
            .iter()
            .map(|(key, value)| crate::remote::quote(&format!("{key}={value}")))
            .collect::<Vec<_>>()
            .join(" ");
        let script = format!("env {assignments} sh -c {}", crate::remote::quote(&script));
        Cmd::new("ssh", timeout)
            .own_group()
            .args([
                "-o",
                "ConnectTimeout=5",
                "-o",
                "BatchMode=yes",
                "--",
                target,
            ])
            .arg(format!("sh -c {}", crate::remote::quote(&script)))
    } else {
        let mut cmd = Cmd::new("sh", timeout)
            .own_group()
            .args(["-c", &script])
            .cwd(cwd);
        for (key, value) in environment {
            cmd = cmd.env(key, value);
        }
        cmd
    }
}

struct GateContext {
    machine: String,
    target: Option<String>,
    cwd: String,
    environment: BTreeMap<String, String>,
}

fn gate_context(ctx: &Ctx, project: &Project, review: &Review) -> Result<GateContext> {
    let reviewer = thread::load(
        project,
        review.reviewer.as_deref().context("reviewer missing")?,
    )?;
    let mut environment = BTreeMap::new();
    let (machine, target) = if reviewer.is_remote() {
        let profile = crate::remote::machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            reviewer.machine_route(),
        )?;
        let declaration = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
        environment.insert("PATH".into(), declaration.path);
        (profile.id, Some(profile.target))
    } else {
        if let Some(path) = ctx.env.var("PATH") {
            environment.insert("PATH".into(), path.to_owned());
        }
        (crate::contracts::MACHINE_LOCAL.to_owned(), None)
    };
    if reviewer.worktree_path.is_empty() {
        bail!("gate result not established: reviewer checkout missing");
    }
    Ok(GateContext {
        machine,
        target,
        cwd: reviewer.worktree_path,
        environment,
    })
}

fn observed_gates(
    ctx: &Ctx,
    project: &Project,
    review: &Review,
    candidate: &str,
    git: &Git<'_>,
) -> Result<Vec<GateRun>> {
    let changed_paths = files(git, &review.base, candidate)?;
    let gates = selected(&review.gates, &changed_paths);
    let evidence_dir = dir(project).join(&review.id);
    std::fs::create_dir_all(&evidence_dir)?;
    std::fs::File::open(dir(project))?.sync_all()?;
    let selection = GateSelection {
        candidate,
        base: &review.base,
        note: &review.gates_note,
        changed_paths,
        not_selected: review
            .gates
            .iter()
            .filter(|gate| !gates.contains(gate))
            .cloned()
            .collect(),
        selected: gates.clone(),
    };
    project::write_atomic(
        &evidence_dir.join(format!("{candidate}-selection.toml")),
        toml::to_string(&selection)?.as_bytes(),
    )?;
    if gates.is_empty() {
        return Ok(Vec::new());
    }
    let GateContext {
        machine,
        target,
        cwd,
        environment,
    } = gate_context(ctx, project, review)?;
    let mut runs = Vec::new();
    for (index, gate) in gates.into_iter().enumerate() {
        let mut environment = environment.clone();
        environment.extend(gate.env.clone());
        let started = jiff::Timestamp::now().to_string();
        let run_dir = evidence_dir.join(format!("{candidate}-{index}-{started}"));
        std::fs::create_dir(&run_dir)?;
        std::fs::File::open(&evidence_dir)?.sync_all()?;
        let logs = crate::runner::OutputLogs {
            stdout: run_dir.join("stdout.log"),
            stderr: run_dir.join("stderr.log"),
        };
        let cmd = gate_command(&gate, candidate, &cwd, &environment, target.as_deref());
        let capture = ctx
            .runner
            .capture(&cmd, ctx.runner.is_real().then_some(&logs));
        let mut receipt = GateReceipt {
            review: review.id.clone(),
            event: review.verdict_event.clone(),
            base: review.base.clone(),
            candidate: candidate.into(),
            gate,
            machine: machine.clone(),
            target: target.clone(),
            cwd: cwd.clone(),
            environment,
            started,
            exit: None,
            timed_out: false,
            complete: false,
            error: String::new(),
            stdout: logs
                .stdout
                .strip_prefix(project.state_dir())?
                .to_string_lossy()
                .into_owned(),
            stderr: logs
                .stderr
                .strip_prefix(project.state_dir())?
                .to_string_lossy()
                .into_owned(),
            stdout_hash: String::new(),
            stderr_hash: String::new(),
        };
        match capture {
            Ok(capture) => {
                receipt.exit = capture.output.code;
                receipt.timed_out = capture.output.timed_out;
                receipt.complete = capture.complete();
                receipt.error = [capture.stdout.error, capture.stderr.error]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; ");
                if !ctx.runner.is_real() {
                    project::write_atomic(&logs.stdout, capture.output.stdout.as_bytes())?;
                    project::write_atomic(&logs.stderr, capture.output.stderr.as_bytes())?;
                }
            }
            Err(error) => receipt.error = format!("{error:#}"),
        }
        receipt.stdout_hash = log_hash(&logs.stdout).unwrap_or_default();
        receipt.stderr_hash = log_hash(&logs.stderr).unwrap_or_default();
        receipt.complete &= !receipt.stdout_hash.is_empty() && !receipt.stderr_hash.is_empty();
        let path = run_dir.join("receipt.toml");
        project::write_atomic(&path, toml::to_string(&receipt)?.as_bytes())?;
        if !receipt.matches(project, review, candidate, &cmd, &machine) {
            bail!(
                "gate result not established (checker/transport/output error): {}; exit {:?}, timed_out {}, complete {}, error {}; receipt {}",
                receipt.gate.command,
                receipt.exit,
                receipt.timed_out,
                receipt.complete,
                receipt.error,
                path.display()
            );
        }
        runs.push(GateRun {
            command: receipt.gate.command,
            exit: 0,
        });
    }
    Ok(runs)
}

impl GateReceipt {
    fn matches(
        &self,
        project: &Project,
        review: &Review,
        candidate: &str,
        command: &crate::runner::Cmd,
        machine: &str,
    ) -> bool {
        self.review == review.id
            && self.event == review.verdict_event
            && self.base == review.base
            && self.candidate == candidate
            && self.machine == machine
            && review.gates.contains(&self.gate)
            && self.exit == Some(0)
            && !self.timed_out
            && self.complete
            && self.error.is_empty()
            && command
                == &gate_command(
                    &self.gate,
                    candidate,
                    &self.cwd,
                    &self.environment,
                    self.target.as_deref(),
                )
            && log_hash(&project.state_dir().join(&self.stdout))
                .is_ok_and(|hash| hash == self.stdout_hash)
            && log_hash(&project.state_dir().join(&self.stderr))
                .is_ok_and(|hash| hash == self.stderr_hash)
    }
}

/// Recheck durable execution proof at the merge boundary, including after a
/// crash. A cached exit=0 verdict is not a receipt, and missing/corrupt logs
/// cannot be promoted to success by a landing retry.
fn verify_gate_receipts(
    ctx: &Ctx,
    project: &Project,
    review: &Review,
    candidate: &str,
    git: &Git<'_>,
) -> Result<()> {
    let gates = selected(&review.gates, &files(git, &review.base, candidate)?);
    if gates.is_empty() {
        return Ok(());
    }
    let GateContext {
        machine,
        target,
        cwd,
        environment,
    } = gate_context(ctx, project, review)?;
    let entries = std::fs::read_dir(dir(project).join(&review.id))
        .context("gate result not established: matching ADE execution receipts missing")?;
    let mut receipts = Vec::<GateReceipt>::new();
    for entry in entries {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("{candidate}-"))
        {
            continue;
        }
        let path = entry.path().join("receipt.toml");
        if path.is_file() {
            receipts.push(toml::from_str(&std::fs::read_to_string(path)?)?);
        }
    }
    for gate in gates {
        let mut environment = environment.clone();
        environment.extend(gate.env.clone());
        let cmd = gate_command(&gate, candidate, &cwd, &environment, target.as_deref());
        let receipt = receipts
            .iter()
            .filter(|receipt| {
                receipt.event == review.verdict_event
                    && receipt.candidate == candidate
                    && receipt.gate == gate
                    && receipt.machine == machine
            })
            .max_by_key(|receipt| receipt.started.parse::<jiff::Timestamp>().ok());
        if !receipt
            .is_some_and(|receipt| receipt.matches(project, review, candidate, &cmd, &machine))
        {
            bail!(
                "gate result not established: {} has no matching complete passing ADE receipt for this seal/candidate/environment",
                gate.command
            );
        }
    }
    Ok(())
}

fn defer_members(project: &Project, review: &Review) -> Result<()> {
    for member in &review.members {
        if review.phase == Phase::Rejected || excluded(review, &member.thread) {
            let reason = review
                .verdict
                .as_ref()
                .and_then(|v| v.without.get(&member.thread))
                .map(String::as_str)
                .unwrap_or("pile rejected; follow up before the next review");
            thread::update(project, &member.thread, |t| {
                t.review_after = member.event.clone();
                t.review_reason = reason.into();
            })?;
        }
    }
    Ok(())
}
fn advance(ctx: &Ctx, project: &Project, review: &mut Review) -> Result<()> {
    if review.install_result.starts_with("REGRESSION") && !review.attention.is_empty() {
        return Ok(());
    }
    if review.phase == Phase::Cancelling {
        let reason = review.attention.clone();
        return cancel_record(ctx, project, review, &reason);
    }
    if let Some(attempt) = review.retry_attempt {
        let id = review
            .reviewer
            .as_deref()
            .context("retry reviewer missing")?;
        let lane = thread::load(project, id)?;
        let message = format!(
            "Retry {} #{}: finish the same pile from integration base {} and seal a fresh verdict. Preserve the checkout's fixes and resolve any unfinished merge.",
            review.id, review.retry_generation, review.base
        );
        if lane.attempt.max(1) == attempt && !lane.follow_ups.iter().any(|f| f.text == message) {
            crate::threads::retry_during_advance(ctx, &project.slug, id, &message)?;
        }
        review.retry_attempt = None;
        save(project, review)?;
    }
    if review.phase == Phase::Preparing {
        prepare(ctx, project, review)?;
    }
    if review.phase == Phase::Reviewing {
        if let Some(tip) = review.refresh_tip.clone() {
            let reviewer = review.reviewer.as_deref().context("reviewer missing")?;
            let message = format!(
                "Integration moved to {tip}. Merge that tip into your candidate and rerun the path-selected gates. Keep review = \"{}\" and seal a new verdict with ha done. This is the one allowed refresh.",
                review.id
            );
            // Queue once by matching the durable follow-up if a crash occurred
            // after delivery but before the review record was saved.
            let lane = thread::load(project, reviewer)?;
            if lane.is_remote() {
                let profile = crate::remote::machine_profile(
                    ctx.runner,
                    &ctx.env.herdr_bin(),
                    &ctx.config_dir,
                    lane.machine_route(),
                )?;
                let settings = project.read_project_md()?.0;
                let (box_repo, url) = crate::threads::box_repo_row(
                    &ctx.config_dir,
                    &settings,
                    &profile.label,
                    &review.repo,
                )?;
                let reference = format!("refs/heads/{}", review.integration);
                // This publishes the already-current integration tip, never
                // the unaccepted review candidate.
                let git = Git::new(ctx.runner, &review.repo);
                if !remote_contains(&git, &url, &reference, &tip)? {
                    git.run(&["push", &url, &format!("{tip}:{reference}")])?;
                }
                let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
                let script = format!(
                    "git -C {} fetch {} {}",
                    crate::remote::quote(&box_repo),
                    crate::remote::quote(&url),
                    crate::remote::quote(&reference)
                );
                let out = crate::remote::ssh(
                    ctx.runner,
                    &profile.target,
                    &crate::remote::with_path(&machine.path, &script),
                    None,
                    std::time::Duration::from_secs(120),
                )?;
                if !out.success() {
                    bail!(
                        "could not fetch the new integration tip on the box: {}",
                        out.error_text()
                    );
                }
            }
            if !lane
                .follow_ups
                .iter()
                .any(|f| f.text == message && f.attempt == lane.attempt.max(1))
            {
                crate::threads::prompt(ctx, &project.slug, reviewer, &message)?;
            }
            review.refresh_tip = None;
            save(project, review)?;
        }
        let reviewer = thread::load(
            project,
            review.reviewer.as_deref().context("reviewer missing")?,
        )?;
        let events = crate::events::checked_for_thread(project, &reviewer.id)?;
        if reviewer.status == Status::Resolved && !reviewer.cancellation_reason.is_empty() {
            return Err(crate::refusal::error(
                "reviewer was cancelled",
                review.command(project, "retry"),
            ));
        }
        if reviewer.status == Status::Failed && !reviewer.recovery_pending {
            needs_coordinator(
                project,
                review,
                &format!("reviewer {} failed: {}", reviewer.id, reviewer.error),
            )?;
            return Ok(());
        }
        // A follow-up sent after a seal can defer landing, but it must not
        // defer checking the report. Preserve the checked verdict while the
        // correction is in flight; a later seal supersedes it, and an empty
        // correction can release the same seal for landing.
        let latest =
            crate::events::latest_done_event(&events, &reviewer.id, reviewer.attempt.max(1));
        let pending = crate::threads::follow_up_pending_for_seal(&reviewer, latest);
        if pending {
            if let Some(event) =
                latest.filter(|e| e.id != review.reviewer_after && e.id != review.checked_event)
            {
                // A local checkout already advanced for the correction has
                // superseded this report. Wait for its next seal instead.
                if !reviewer.is_remote()
                    && !reviewer.worktree_path.is_empty()
                    && Git::new(ctx.runner, &reviewer.worktree_path)
                        .run(&["rev-parse", "HEAD"])
                        .is_ok_and(|head| head != event.payload.done.as_ref().expect("seal").sha)
                {
                    return Ok(());
                }
                let git = Git::new(ctx.runner, &review.repo);
                fetch_reviewer(ctx, project, review, &reviewer, event, &git)?;
                match verdict(ctx, project, review, event, &git) {
                    Ok(checked) => {
                        review.verdict = Some(checked);
                        review.checked_event = event.id.clone();
                        review.attention.clear();
                        save(project, review)?;
                    }
                    Err(error) => {
                        review.verdict = None;
                        review.checked_event = event.id.clone();
                        review.attention = format!("{error:#}");
                        needs_coordinator(project, review, &review.attention.clone())?;
                        return Err(error);
                    }
                }
            }
            return Ok(());
        }
        let Some(event) = sealed(&events, &reviewer).filter(|e| {
            e.id != review.reviewer_after
                && (e.id != review.checked_event
                    || review.verdict.is_some()
                        && review.verdict_event.is_empty()
                        && review.attention.is_empty())
        }) else {
            return Ok(());
        };
        let git = Git::new(ctx.runner, &review.repo);
        fetch_reviewer(ctx, project, review, &reviewer, event, &git)?;
        let verdict = match verdict(ctx, project, review, event, &git) {
            Ok(verdict) => verdict,
            Err(error) => {
                review.verdict = None;
                review.checked_event = event.id.clone();
                review.attention = format!("{error:#}");
                needs_coordinator(project, review, &review.attention.clone())?;
                return Err(error);
            }
        };
        if verdict.verdict == "MERGE" {
            let _git_lock = crate::git::lock(ctx.runner, &review.repo)?;
            let old = git
                .branch_head(&review.candidate_branch)?
                .context("candidate branch missing")?;
            git.run(&[
                "update-ref",
                &format!("refs/heads/{}", review.candidate_branch),
                &verdict.candidate,
                &old,
            ])?;
        }
        review.attention.clear();
        review.verdict_event = event.id.clone();
        review.verdict = Some(verdict);
        if review
            .verdict
            .as_ref()
            .is_some_and(|v| v.verdict == "REJECT")
        {
            review.phase = Phase::Rejected;
            let reason = review
                .verdict
                .as_ref()
                .map(|v| v.without.values().cloned().collect::<Vec<_>>().join("; "))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "reviewer rejected the pile".into());
            queue_notice(review, format!("REVIEW {} rejected: {reason}", review.id));
            // Write barriers before closing the review so a crash cannot return
            // unchanged rejected lanes to automatic review.
            defer_members(project, review)?;
            save(project, review)?;
            crate::threads::resolve_automatically(ctx, project, &reviewer.id, "review rejected");
            return Ok(());
        }
        // Persist the exact landing intent before touching the integration ref.
        review.phase = Phase::Landing;
        save(project, review)?;
    }
    if review.phase != Phase::Landing {
        return Ok(());
    }
    land(ctx, project, review)
}
fn fetch_reviewer(
    ctx: &Ctx,
    project: &Project,
    review: &Review,
    reviewer: &Thread,
    event: &crate::contracts::Event,
    git: &Git<'_>,
) -> Result<()> {
    if !reviewer.is_remote() {
        return Ok(());
    }
    let settings = project.read_project_md()?.0;
    let profile = crate::remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        reviewer.machine_route(),
    )?;
    let (_, url) =
        crate::threads::box_repo_row(&ctx.config_dir, &settings, &profile.label, &review.repo)?;
    let done = event
        .payload
        .done
        .as_ref()
        .context("reviewer has no seal")?;
    git.run(&[
        "fetch",
        &url,
        &format!(
            "refs/heads/{}",
            done.published_ref.as_deref().unwrap_or(&reviewer.branch)
        ),
    ])?;
    if git.run(&["rev-parse", "FETCH_HEAD"])? != done.sha {
        bail!("reviewer publication differs from seal");
    }
    Ok(())
}

/// `ls-remote <name>` uses its fetch URL, whereas `push <name>` uses its
/// push URLs. Verify each actual destination, never the reviewer clone that
/// might already have fast-forwarded its own integration branch.
fn push_destinations(git: &Git<'_>, remote: &str) -> Result<Vec<String>> {
    let names = git.run(&["remote"])?;
    let urls = if names.lines().any(|name| name == remote) {
        git.run(&["remote", "get-url", "--push", "--all", remote])?
    } else {
        // Let Git resolve pushInsteadOf for a literal URL too. This remote
        // exists only in this command's config, not in the repository.
        let mut name = "ade-publication".to_owned();
        while names.lines().any(|existing| existing == name) {
            name.push('-');
        }
        let rows = git.run(&[
            "-c",
            &format!("remote.{name}.url={remote}"),
            "remote",
            "--verbose",
        ])?;
        let prefix = format!("{name}\t");
        rows.lines()
            .filter_map(|row| row.strip_prefix(&prefix)?.strip_suffix(" (push)"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let urls = urls
        .lines()
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if urls.is_empty() {
        bail!("integration not published: {remote} has no push destination");
    }
    Ok(urls)
}

fn remote_contains(git: &Git<'_>, remote: &str, reference: &str, candidate: &str) -> Result<bool> {
    let published = git.run(&["ls-remote", remote, reference])?;
    match published.split_whitespace().next() {
        Some(head) if head == candidate => Ok(true),
        Some(head) => {
            git.run(&["fetch", remote, reference])?;
            git.is_ancestor(candidate, head)
        }
        None => Ok(false),
    }
}

fn land(ctx: &Ctx, project: &Project, review: &mut Review) -> Result<()> {
    let review_id = review.id.clone();
    land_with_install(ctx, project, review, || {
        let installed = crate::harness::install_for_review(ctx, &project.slug, &review_id)?;
        if installed.box_failed() {
            anyhow::bail!(
                "box pending: local installation finished; box installation still needs to complete: {}",
                installed.warnings.join("; ")
            );
        }
        Ok(installed.summary().to_string())
    })
}

fn land_with_install(
    ctx: &Ctx,
    project: &Project,
    review: &mut Review,
    install: impl FnOnce() -> Result<String>,
) -> Result<()> {
    let git = Git::new(ctx.runner, &review.repo);
    let candidate = review
        .verdict
        .as_ref()
        .context("landing verdict missing")?
        .candidate
        .clone();
    if !review.fast_forward {
        verify_gate_receipts(ctx, project, review, &candidate, &git)?;
        let _git_lock = crate::git::lock(ctx.runner, &review.repo)?;
        let head = git
            .branch_head(&review.integration)?
            .context("integration branch missing")?;
        if head != candidate && !git.is_ancestor(&candidate, &head)? {
            if head != review.base {
                drop(_git_lock);
                if review.moved > 0 {
                    cancel_record(
                        ctx,
                        project,
                        review,
                        "integration moved twice; starting a fresh review",
                    )?;
                    return Ok(());
                }
                review.moved += 1;
                review.base = head.clone();
                review.refresh_tip = Some(head);
                review.reviewer_after = review.verdict_event.clone();
                review.verdict = None;
                review.phase = Phase::Reviewing;
                save(project, review)?;
                return Ok(());
            }
            let events = crate::events::checked(project)?;
            for member in &review.members {
                let lane = thread::load(project, &member.thread)?;
                if sealed(&events, &lane).map(|e| &e.id) != Some(&member.event) {
                    return Err(crate::refusal::error(
                        format!("{} changed during review", member.thread),
                        review.command(project, "cancel"),
                    ));
                }
            }
            if let Some(checkout) = git.checkout_of(&review.integration)? {
                if !git.dirty_paths(&checkout)?.is_empty() {
                    bail!("integration checkout is dirty");
                }
                git.run_in(&checkout, &["merge", "--ff-only", &candidate])?;
            } else {
                git.run(&[
                    "update-ref",
                    &format!("refs/heads/{}", review.integration),
                    &candidate,
                    &head,
                ])?;
            }
        }
        review.fast_forward = true;
        save(project, review)?;
    }
    // Idempotent lane markers are written only after the durable FF boundary.
    for member in &review.members {
        if !excluded(review, &member.thread) {
            let lane = thread::load(project, &member.thread)?;
            if lane.merged_sha != candidate || lane.merged_review != review.id {
                thread::update(project, &member.thread, |t| {
                    t.merged_sha = candidate.clone();
                    t.merged_review = review.id.clone();
                })?;
            }
        }
    }
    defer_members(project, review)?;
    if review.push_remote.is_none() {
        let row = repository(ctx, project, Some(&review.repo))?;
        review.push_remote = row.push_remote.or(row.publish_url);
    }
    if let Some(remote) = review
        .push_remote
        .clone()
        .filter(|remote| !remote.is_empty())
    {
        let target = format!("refs/heads/{}", review.integration);
        let destinations = push_destinations(&git, &remote)?;
        let mut published = true;
        for url in &destinations {
            published &= remote_contains(&git, url, &target, &candidate)?;
        }
        // A pending install must not trust a historical push cursor. Repair a
        // falsely recorded publication before any local or box install runs.
        if !published {
            review.push = false;
            save(project, review)?;
            git.run(&["push", &remote, &format!("{candidate}:{target}")])?;
            for url in &destinations {
                if !remote_contains(&git, url, &target, &candidate)? {
                    bail!(
                        "integration not published: {url} {target} does not contain {candidate} after push"
                    );
                }
            }
        }
    }
    if !review.push {
        review.push = true;
        save(project, review)?;
    }
    if !review.install {
        if review.install_required {
            match install() {
                Ok(summary) => review.install_result = summary,
                Err(error) => {
                    if error.to_string().starts_with("REGRESSION") {
                        review.install_result = error.to_string();
                        review.attention = error.to_string();
                        queue_notice(
                            review,
                            format!("REVIEW {}: {}", review.id, review.landing_summary()),
                        );
                        save(project, review)?;
                    }
                    return Err(error);
                }
            }
        }
        review.install = true;
        save(project, review)?;
    }
    if !review.close {
        let ids = review
            .members
            .iter()
            .filter(|m| !excluded(review, &m.thread))
            .map(|m| m.thread.clone())
            .chain(review.reviewer.clone())
            .collect::<Vec<_>>();
        // The terminal thread marker is the durable per-member cursor. A
        // crash during external cleanup leaves cleanup_pending for the bounded
        // retry queue, without replaying earlier members of this landing.
        let pending = ids
            .iter()
            .map(|id| thread::load(project, id))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter(|lane| lane.status != Status::Resolved)
            .collect::<Vec<_>>();
        let mut views = crate::threads::CleanupViews::default();
        for lane in pending.iter().take(crate::threads::CLEANUP_BATCH_SIZE) {
            crate::threads::resolve_automatically_with_views(
                ctx, project, &lane.id, "merged", &mut views,
            );
        }
        if pending.len() > crate::threads::CLEANUP_BATCH_SIZE {
            return Ok(());
        }
        for id in &ids {
            if thread::load(project, id)?.status != Status::Resolved {
                return Ok(());
            }
        }
        // Cleanup belongs to each merged thread, not to the landing. Its
        // durable cleanup_pending marker lets the ticker retry independently.
        review.close = true;
        save(project, review)?;
    }
    if !review.prune {
        let _git_lock = crate::git::lock(ctx.runner, &review.repo)?;
        if let Some(head) = git.branch_head(&review.candidate_branch)? {
            git.run(&[
                "update-ref",
                "-d",
                &format!("refs/heads/{}", review.candidate_branch),
                &head,
            ])?;
        }
        review.prune = true;
        save(project, review)?;
    }
    review.phase = Phase::Complete;
    let members = review
        .members
        .iter()
        .filter(|m| !excluded(review, &m.thread))
        .map(|m| m.thread.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    queue_notice(
        review,
        format!(
            "REVIEW {} merged {members} (commit {}; {}){}",
            review.id,
            &candidate[..candidate.len().min(7)],
            review.landing_summary(),
            review.gates_summary()
        ),
    );
    review.attention.clear();
    save(project, review)?;
    project::refresh_page(project)
}
fn cancel_record(ctx: &Ctx, project: &Project, review: &mut Review, reason: &str) -> Result<()> {
    if !review.fast_forward && review.phase == Phase::Landing {
        let git = Git::new(ctx.runner, &review.repo);
        let head = git
            .branch_head(&review.integration)?
            .context("integration branch missing")?;
        if let Some(verdict) = &review.verdict
            && git.is_ancestor(&verdict.candidate, &head)?
        {
            review.fast_forward = true;
            save(project, review)?;
        }
    }
    if review.fast_forward {
        return Err(crate::refusal::error(
            "review has landed; resume publication, installation and cleanup",
            review.command(project, "retry"),
        ));
    }
    // Seal the cancellation before touching the reviewer. A crash after
    // stopping it must never accept its earlier MERGE seal on the next pass.
    review.phase = Phase::Cancelling;
    review.attention = reason.into();
    save(project, review)?;
    if let Some(id) = &review.reviewer {
        let outcome = crate::threads::cancel(ctx, &project.slug, id, reason)?;
        if outcome.state == "cleanup_pending" {
            bail!("reviewer cancellation cleanup pending: {id}");
        }
    }
    review.close = true;
    save(project, review)?;
    let git = Git::new(ctx.runner, &review.repo);
    let _git_lock = crate::git::lock(ctx.runner, &review.repo)?;
    if let Some(head) = git.branch_head(&review.candidate_branch)? {
        git.run(&[
            "update-ref",
            "-d",
            &format!("refs/heads/{}", review.candidate_branch),
            &head,
        ])?;
    }
    review.prune = true;
    review.phase = Phase::Cancelled;
    save(project, review)
}
pub(crate) fn cancel(ctx: &Ctx, slug: &str, repo: Option<&str>) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let row = repository(ctx, &project, repo)?;
    let _lock = operation_lock(ctx, &row.path)?;
    if let Some((home, mut record)) = active_for_repo(ctx, &row.path)? {
        cancel_record(ctx, &home, &mut record, "cancelled by coordinator")?;
    }
    project::refresh_page(&project)
}
pub(crate) fn retry(ctx: &Ctx, slug: &str, repo: Option<&str>) -> Result<Option<Review>> {
    let project = Project::load(&ctx.root, slug)?;
    let row = repository(ctx, &project, repo)?;
    let _lock = operation_lock(ctx, &row.path)?;
    let Some((home, mut record)) = active_for_repo(ctx, &row.path)? else {
        return Err(crate::refusal::error(
            "no active pile review",
            format!(
                "ha review {} --repo {}",
                crate::remote::quote(slug),
                crate::remote::quote(&row.path)
            ),
        ));
    };
    if record.fast_forward || matches!(record.phase, Phase::Landing | Phase::Cancelling) {
        if record.install_result.starts_with("REGRESSION") {
            record.install_result.clear();
            record.attention.clear();
            save(&home, &record)?;
        }
        if !record.fast_forward && record.phase == Phase::Landing {
            let git = Git::new(ctx.runner, &record.repo);
            let candidate = record
                .verdict
                .as_ref()
                .context("landing verdict missing")?
                .candidate
                .clone();
            if verify_gate_receipts(ctx, &home, &record, &candidate, &git).is_err() {
                // An explicit retry may replace lost/old mechanical evidence,
                // but it still validates the original semantic seal and runs
                // on the same machine/candidate. Never fabricate a receipt.
                let event = crate::events::load(&home, &record.verdict_event)?;
                let checked = verdict(ctx, &home, &record, &event, &git)?;
                if checked.verdict != "MERGE" {
                    bail!("landing seal is not MERGE");
                }
                record.verdict = Some(checked);
                save(&home, &record)?;
            }
        }
        advance(ctx, &home, &mut record)?;
        return Ok(Some(record));
    }
    // Preserve the reviewer's checkout, conflicts and fixes when replacing its
    // process. Persist the new review phase before a parked lane is reopened.
    if let Some(id) = &record.reviewer {
        let lane = thread::load(&home, id)?;
        record.retry_attempt = Some(lane.attempt.max(1));
        record.retry_generation += 1;
        record.reviewer_after = crate::events::latest_done_event(
            &crate::events::checked(&home)?,
            id,
            lane.attempt.max(1),
        )
        .map(|e| e.id.clone())
        .unwrap_or_default();
    }
    record.verdict = None;
    record.verdict_event.clear();
    record.checked_event.clear();
    record.no_verdict_since.clear();
    record.refresh_tip = None;
    record.phase = if record.reviewer.is_some() {
        Phase::Reviewing
    } else {
        Phase::Preparing
    };
    record.attention.clear();
    save(&home, &record)?;
    advance(ctx, &home, &mut record)?;
    Ok(Some(record))
}
/// Refuse member follow-ups during review rather than silently changing its
/// sealed input set. A moved-tip follow-up belongs to the same reviewer.
pub(crate) fn require_follow_up(project: &Project, id: &str) -> Result<()> {
    for record in list(project)? {
        if !record.phase.closed()
            && (record
                .members
                .iter()
                .any(|m| m.thread == id && !excluded(&record, id))
                || record.phase == Phase::Landing && record.reviewer.as_deref() == Some(id))
        {
            return Err(crate::refusal::error(
                format!(
                    "{id} is in {}; finish landing before a follow-up, or cancel an unlanded review",
                    record.id
                ),
                record.command(
                    project,
                    if record.fast_forward || record.phase == Phase::Landing {
                        "retry"
                    } else {
                        "cancel"
                    },
                ),
            ));
        }
    }
    let lane = thread::load(project, id)?;
    if let Some(event) =
        crate::events::latest_done_event(&crate::events::checked(project)?, id, lane.attempt.max(1))
    {
        thread::update(project, id, |t| t.review_after = event.id.clone())?;
    }
    Ok(())
}
pub(crate) fn require_resolvable(project: &Project, id: &str) -> Result<()> {
    for record in list(project)? {
        if !record.phase.closed()
            && record.phase != Phase::Cancelling
            && !record.fast_forward
            && (record.members.iter().any(|m| m.thread == id)
                || record.reviewer.as_deref() == Some(id))
        {
            return Err(crate::refusal::error(
                format!("{id} is in {}; cancel the review first", record.id),
                record.command(project, "cancel"),
            ));
        }
    }
    Ok(())
}
// The lane remains eligible if its repository is configured again. Cache only
// the warning, not the classification, so repeated reads stay quiet.
fn lane_repository(
    ctx: &Ctx,
    project: &Project,
    lane: &Thread,
    seal: &str,
) -> Result<Option<project::Repo>> {
    match repository(ctx, project, Some(&lane.repo)) {
        Ok(row) => Ok(Some(row)),
        Err(error) if error.is::<UnconfiguredRepo>() => {
            if lane.unconfigured_repo_seal != seal {
                eprintln!(
                    "note: {}/{} seal {} skipped: repository {} is not configured",
                    project.slug, lane.id, seal, lane.repo
                );
                thread::update(project, &lane.id, |t| {
                    t.unconfigured_repo_seal = seal.to_owned()
                })?;
            }
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Classify old seals outside the open pile. The seal id is the cache key: a
/// later attempt/follow-up can be classified afresh, but idle passes do no git.
pub(crate) fn classify_old_seals(ctx: &Ctx, project: &Project, include_open: bool) -> Result<()> {
    let mut events_by_thread: BTreeMap<String, Vec<crate::contracts::Event>> = BTreeMap::new();
    for event in crate::events::checked(project)? {
        events_by_thread
            .entry(event.thread.clone())
            .or_default()
            .push(event);
    }
    let harness = crate::harness::repos(&ctx.config_dir)?;
    let mut tasks = None;
    let mut heads: BTreeMap<String, String> = BTreeMap::new();
    let reviewers = reviewer_ids(project)?;
    for lane in thread::list(project) {
        if reviewers.contains(&lane.id) || lane.repo.is_empty() || !lane.merged_sha.is_empty() {
            continue;
        }
        let events = events_by_thread
            .get(&lane.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let Some(event) = sealed(events, &lane) else {
            continue;
        };
        if lane.historical_seal == event.id {
            continue;
        }
        // The ticker leaves open piles to review start. Explicit reads can
        // classify unknown seals even when the lane remains open.
        if lane.status != Status::Resolved && (!include_open || changes(&lane, event).is_some()) {
            continue;
        }
        let done = event.payload.done.as_ref().expect("sealed done");
        let Some(row) = lane_repository(ctx, project, &lane, &event.id)? else {
            continue;
        };
        let Some(_lock) = try_operation_lock(ctx, &row.path)? else {
            continue;
        };
        let git = Git::new(ctx.runner, &row.path);
        let tip = if let Some(tip) = heads.get(&row.path) {
            tip.clone()
        } else {
            let branch = row
                .branch
                .clone()
                .map(Ok)
                .unwrap_or_else(|| crate::git::symbolic_head(ctx.runner, &row.path))?;
            let tip = git
                .branch_head(&branch)?
                .context("integration branch is missing")?;
            heads.insert(row.path.clone(), tip.clone());
            tip
        };
        let has_changes = if let Some(known) = changes(&lane, event) {
            known
        } else if lane.base.is_empty() {
            // No base survives: ancestry still proves a landed seal, but
            // cannot distinguish a no-op from a change.
            if !git.is_ancestor(&done.sha, &tip)? {
                continue;
            }
            true
        } else {
            git.trees_differ(&lane.base, &done.sha)?
        };
        let merged = has_changes && git.is_ancestor(&done.sha, &tip)?;
        let harness_repo = harness.iter().any(|r| same_repo(&r.path, &row.path));
        let installed = if merged && harness_repo {
            let builds = tasks
                .get_or_insert_with(|| crate::task::list_with_errors(project).0)
                .iter()
                .filter(|t| t.attempts.contains(&lane.id))
                .flat_map(|t| t.installed.iter().filter_map(|e| e.build.as_deref()));
            // Task install evidence is enough; otherwise inspect the installed
            // build record for this harness repository.
            let mut covered = false;
            for build in builds {
                if !build.is_empty() && git.is_ancestor(&done.sha, build.trim()).unwrap_or(false) {
                    covered = true;
                    break;
                }
            }
            if !covered {
                let bin = if row.path.ends_with("/herdr-ade") {
                    "herdr-ade"
                } else {
                    "herdr"
                };
                if let Ok(build) = std::fs::read_to_string(
                    ctx.env
                        .home
                        .join(".local/bin")
                        .join(format!(".{bin}.installed-commit")),
                ) {
                    covered = git.is_ancestor(&done.sha, build.trim()).unwrap_or(false);
                }
            }
            covered
        } else {
            false
        };
        thread::update(project, &lane.id, |t| {
            t.changes_seal = event.id.clone();
            t.historical_seal = event.id.clone();
            t.has_changes = Some(has_changes);
            if merged {
                t.merged_sha = tip.clone();
                t.historical_install_required = harness_repo;
                if installed {
                    t.installed_sha = tip.clone();
                }
            }
        })?;
    }
    Ok(())
}

/// Holds have their own journal because there may be no review record yet.
/// A reason is announced once until it changes or the hold clears. Pending
/// notices survive that clearing and are handed to the shared durable outbox.
#[derive(Default, Serialize, Deserialize)]
struct PileHolds {
    current: BTreeMap<String, String>,
    notices: Vec<crate::steps::Notice>,
}
fn holds_path(project: &Project) -> PathBuf {
    project.state_dir().join("pile-holds.json")
}
pub(crate) fn current_holds(project: &Project) -> Result<BTreeMap<String, String>> {
    Ok(load_holds(project)?.current)
}

pub(crate) fn hold_notices(project: &Project) -> Result<Vec<crate::steps::Notice>> {
    Ok(load_holds(project)?.notices)
}
fn load_holds(project: &Project) -> Result<PileHolds> {
    match std::fs::read(holds_path(project)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PileHolds::default()),
        Err(error) => Err(error.into()),
    }
}
pub(crate) fn mark_hold_submitted(project: &Project, index: usize) -> Result<()> {
    let _lock = project.lock()?;
    let mut holds = load_holds(project)?;
    if let Some(notice) = holds.notices.get_mut(index) {
        notice.submitted = true;
        project::write_json(&holds_path(project), &holds)?;
    }
    Ok(())
}
fn record_holds(project: &Project, current: BTreeMap<String, (String, String)>) -> Result<()> {
    let _lock = project.lock()?;
    let mut holds = load_holds(project)?;
    for (repo, (reason, line)) in &current {
        if holds.current.get(repo) != Some(reason) {
            holds.notices.push(crate::steps::Notice {
                line: line.clone(),
                submitted: false,
            });
        }
    }
    let next = current
        .into_iter()
        .map(|(repo, (reason, _))| (repo, reason))
        .collect();
    if holds.current != next {
        holds.current = next;
        project::write_json(&holds_path(project), &holds)?;
    }
    Ok(())
}
fn working_hold(lane: &Thread, events: &[crate::contracts::Event]) -> Option<&'static str> {
    if !matches!(lane.status, Status::Starting | Status::Open) {
        return None;
    }
    let latest = crate::events::latest_event(events, &lane.id, lane.attempt.max(1));
    if crate::threads::follow_up_pending_for_seal(lane, latest)
        || lane.follow_ups.iter().any(|f| {
            f.attempt == lane.attempt.max(1)
                && f.state == thread::FollowUpState::Delivered
                && latest.is_some_and(|event| f.waiting_event == event.id)
        })
    {
        return Some("follow-up pending");
    }
    if latest.is_some_and(|event| event.payload.done.is_some() || event.payload.waiting.is_some()) {
        None
    } else {
        Some("working")
    }
}

#[cfg(test)]
fn tick(ctx: &Ctx, project: &Project) -> Result<()> {
    tick_observed(ctx, project, |_| true)
}

pub(crate) fn tick_observed(
    ctx: &Ctx,
    project: &Project,
    can_advance: impl Fn(&Review) -> bool,
) -> Result<()> {
    let enabled = project.state_dir().join("reviews-enabled").exists();
    let mut first = None;
    if enabled && let Err(error) = classify_old_seals(ctx, project, false) {
        // An unrelated historical seal must not prevent a fresh pile starting.
        first = Some(error);
    }
    for old in list(project)?
        .into_iter()
        .filter(|r| !r.phase.closed() && can_advance(r))
    {
        let Some(_lock) = try_operation_lock(ctx, &old.repo)? else {
            continue;
        };
        let mut record = load(project, &old.id)?;
        if let Err(error) = advance(ctx, project, &mut record)
            .and_then(|()| watch_no_verdict(ctx, project, &mut record))
        {
            if !crate::remote::is_unreachable(&format!("{error:#}")) {
                record.attention = format!("{error:#}");
                save(project, &record)?;
            }
            first.get_or_insert(error);
        }
    }
    crate::events::checked(project)?;
    let events = crate::events::for_unresolved_threads(project);
    let threads = thread::list_live(project);
    let reviewers = reviewer_ids(project)?;
    let repos: BTreeMap<_, _> = threads
        .iter()
        .filter(|t| !reviewers.contains(&t.id) && t.status != Status::Resolved)
        .map(|t| (repo_identity(&t.repo), t.repo.clone()))
        .collect();
    let mut holds = BTreeMap::new();
    for repo in repos.into_values() {
        let mut pile = pending(project, &repo, &events, &reviewers);
        if repo.is_empty() || pile.is_empty() {
            continue;
        }
        let outcome = (|| -> Result<Option<String>> {
            let active = active_for_repo(ctx, &repo)?;
            // Members already under review are not a new ready pile.
            if let Some((home, review)) = &active {
                if home.slug == project.slug {
                    pile.retain(|lane| !review.members.iter().any(|m| m.thread == lane.id));
                }
                if pile.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(format!(
                    "{} / {} (active review {})",
                    home.slug,
                    review.reviewer.as_deref().unwrap_or("reviewer allocation"),
                    review.id
                )));
            }
            if !enabled {
                return Ok(Some(format!(
                    "automatic review not enabled (next: ha review {} --repo {})",
                    crate::remote::quote(&project.slug),
                    crate::remote::quote(&repo)
                )));
            }
            let mut blockers: Vec<String> = threads
                .iter()
                .filter(|t| same_repo(&t.repo, &repo) && !reviewers.contains(&t.id))
                .filter_map(|t| working_hold(t, &events).map(|why| format!("{} ({why})", t.id)))
                .collect();
            let failed: Vec<_> = threads
                .iter()
                .filter(|t| {
                    same_repo(&t.repo, &repo)
                        && !reviewers.contains(&t.id)
                        && t.status == Status::Failed
                        && !t.is_remote()
                })
                .collect();
            if !failed.is_empty() {
                let socket = project.coordinator().map(|c| c.socket).unwrap_or_default();
                let herdr = crate::herdr::Herdr::new(ctx.env.herdr_bin(), &socket, ctx.runner);
                let agents = herdr.agent_list()?;
                let panes = herdr.pane_list()?;
                blockers.extend(
                    failed
                        .into_iter()
                        .filter(|t| {
                            thread::recoverable_agent(t, &threads, &agents, &panes).is_some()
                        })
                        .map(|t| format!("{} (failed lane has a live agent)", t.id)),
                );
            }
            if !blockers.is_empty() {
                blockers.sort();
                return Ok(Some(blockers.join(", ")));
            }
            let Some(_lock) = try_operation_lock(ctx, &repo)? else {
                return Ok(Some("repository operation lock (busy)".into()));
            };
            // Recheck under the operation lock: another project may have started a review.
            if let Some((home, review)) = active_for_repo(ctx, &repo)? {
                return Ok(Some(format!(
                    "{} / {} (active review {})",
                    home.slug,
                    review.reviewer.as_deref().unwrap_or("reviewer allocation"),
                    review.id
                )));
            }
            let mut configured = None;
            let mut unconfigured = Vec::new();
            for lane in &pile {
                let seal = sealed(&events, lane).expect("pending seal");
                if let Some(row) = lane_repository(ctx, project, lane, &seal.id)? {
                    configured = Some(row);
                } else {
                    unconfigured.push(format!("{} (repository not configured)", lane.id));
                }
            }
            if let Some(row) = configured {
                start_locked(ctx, project, row)?;
                Ok(None)
            } else {
                Ok(Some(unconfigured.join(", ")))
            }
        })();
        let reason = match outcome {
            Ok(reason) => reason,
            Err(error) => {
                let reason = format!(
                    "{} (review start: {})",
                    pile.iter()
                        .map(|t| t.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    crate::steps::short_error(&format!("{error:#}"))
                );
                first.get_or_insert(error);
                Some(reason)
            }
        };
        if let Some(reason) = reason {
            let ready = pile
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let name = std::path::Path::new(&repo)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            holds.insert(
                repo_identity(&repo).to_string_lossy().into_owned(),
                (
                    reason.clone(),
                    format!("PILE {name}: {ready} ready; held by {reason}"),
                ),
            );
        }
    }
    record_holds(project, holds)?;
    first.map_or(Ok(()), Err)
}
