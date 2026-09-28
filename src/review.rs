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
    pub gates: Vec<GateRun>,
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
    std::fs::File::open(dir(project))?.sync_all()?;
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

pub(crate) fn list(project: &Project) -> Result<Vec<Review>> {
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
            "REVIEW {} needs you: {attention} — next: ha review retry {}",
            review.id, project.slug
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
    let events = crate::events::checked(project)?;
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
            .any(|n| !n.submitted && n.line.contains("needs you"))
    {
        let line = format!(
            "REVIEW {} has no verdict since {} (reviewer {} {}); inspect the pane or run ha review retry {}. No merge was attempted.",
            review.id, review.no_verdict_since, lane.id, state, project.slug
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
            .context("review repository is not configured"),
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
fn changes(lane: &Thread, event: &crate::contracts::Event) -> Option<bool> {
    let sealed = event.payload.done.as_ref()?;
    if lane.changes_seal == event.id {
        lane.has_changes.or(sealed.has_changes)
    } else {
        sealed.has_changes
    }
}

/// Carry pre-tree-rule cached classifications forward once per machine. Old
/// seal events are immutable; the matching thread cache is their correction.
pub(crate) fn reclassify_old_changes(ctx: &Ctx, mut log: impl FnMut(&str)) -> Result<()> {
    let marker = ctx.root.join(".change-reclass-v1.json");
    if marker.exists() {
        return Ok(());
    }
    for slug in project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        let events = crate::events::checked(&project)?;
        let (lanes, errors) = thread::list_with_errors(&project);
        if let Some(error) = errors.into_iter().next() {
            return Err(error);
        }
        for lane in lanes
            .into_iter()
            .filter(|lane| lane.has_changes == Some(true))
        {
            let Some(event) = events.iter().find(|event| {
                event.id == lane.changes_seal
                    && event.thread == lane.id
                    && event.payload.done.is_some()
            }) else {
                continue;
            };
            if lane.base.is_empty() || lane.repo.is_empty() {
                continue;
            }
            let sha = &event.payload.done.as_ref().expect("sealed done").sha;
            let git = Git::new(ctx.runner, &lane.repo);
            match git.trees_differ(&lane.base, sha) {
                Ok(false) => {
                    thread::update(&project, &lane.id, |record| {
                        if record.changes_seal == event.id && record.has_changes == Some(true) {
                            record.has_changes = Some(false);
                            // A pre-tree-rule classifier may also have inferred
                            // a historical landing and install from ancestry.
                            // Those are not evidence of a changed tree.
                            if record.merged_review.is_empty() {
                                record.merged_sha.clear();
                                record.installed_sha.clear();
                                record.historical_install_required = false;
                            }
                        }
                    })?;
                }
                Ok(true) => {}
                Err(error) => log(&format!(
                    "one-time change reclassification: {slug}/{} skipped (objects unavailable locally): {error:#}",
                    lane.id
                )),
            }
        }
    }
    project::write_json(&marker, &true)
}

/// No git here: used by the ticker to decide whether a review can start.
fn pending(
    project: &Project,
    repo: &str,
    events: &[crate::contracts::Event],
    reviewers: &std::collections::BTreeSet<String>,
) -> Vec<Thread> {
    pending_from(thread::list(project), repo, events, reviewers)
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
    let gates = row.gates.clone().unwrap_or_default();
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
        push_remote: row.push_remote,
        install_required: crate::harness::repos(&ctx.config_dir)?
            .iter()
            .any(|r| same_repo(&r.path, &row.path)),
        fast_forward: false,
        push: false,
        install: false,
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
        "Run `ha skill reviewer`. Review the whole repository pile {}. Your branch starts from candidate `{}` on integration base `{}`. Merge every included SHA below (some may already be merged); resolve conflicts, fix small issues, then run the selected gates once. Do not push or install.\n\n",
        review.id, review.candidate_branch, review.base
    );
    for member in &review.members {
        out.push_str(&format!(
            "- {}: `{}`, report `{}`\n",
            member.thread,
            member.sha,
            crate::events::artifact_path(project, &member.artifact).display()
        ));
    }
    out.push_str("\nGate policy (select by all paths changed from the integration base, including your fixes):\n");
    out.push_str(
        &toml::to_string(&GatePolicy {
            gates: review.gates.clone(),
        })
        .expect("gate serialization"),
    );
    out.push_str("\nSelected gates for this pile:\n");
    for gate in &review.selected_gates {
        out.push_str(&format!(
            "- `{}`; environment {:?}\n",
            gate.command, gate.env
        ));
    }
    out.push_str(&format!("\nWrite a report with TOML front matter:\n+++\nreview = \"{}\"\nverdict = \"MERGE\" # or REJECT\ncandidate = \"<your exact HEAD>\"\ngates = [{{ command = \"<selected command>\", exit = 0 }}]\n# Optional: without = {{ t-0001 = \"one-line reason\" }}\n+++\n\nUse gates = [] if none are selected. If excluding lanes, rebuild from the integration base without those lanes before running gates; their commits must not remain ancestors of your candidate. Include gate output and findings. Commit your fixes, then `ha done --report <report in your git folder> --sha <your HEAD>`. Output in another repository belongs in the report.\n", review.id));
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
            bail!("multiple reviewers for {}; cancel or retry", review.id);
        }
        let reviewer = if let Some(t) = unbound.first() {
            t.clone()
        } else {
            crate::threads::start_during_advance(
                ctx,
                &project.slug,
                crate::threads::StartArgs {
                    title: format!("Review pile {}", review.id),
                    repo: Some(review.repo.clone()),
                    machine: None,
                    base: Some(review.candidate_branch.clone()),
                    task: task(project, review),
                    plain: "Check the finished work together and land what is ready".into(),
                    workflow: Some("reviewer".into()),
                    recipe: None,
                    recipe_basis: None,
                    task_id: String::new(),
                    review_id: review.id.clone(),
                },
            )?
        };
        review.reviewer = Some(reviewer.id);
    }
    review.phase = Phase::Reviewing;
    save(project, review)
}
fn verdict(
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
    let verdict: Verdict = toml::from_str(front)?;
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
    let gates = selected(
        &review.gates,
        &files(git, &review.base, &verdict.candidate)?,
    );
    if verdict.gates.iter().any(|run| run.exit != 0)
        || gates.iter().any(|gate| {
            !verdict
                .gates
                .iter()
                .any(|run| run.command == gate.command && run.exit == 0)
        })
    {
        bail!(
            "verdict must report each candidate-selected gate with exit 0; all extra gates must pass"
        );
    }
    Ok(verdict)
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
        let events = crate::events::checked(project)?;
        let reviewer = thread::load(
            project,
            review.reviewer.as_deref().context("reviewer missing")?,
        )?;
        if reviewer.status == Status::Resolved && !reviewer.cancellation_reason.is_empty() {
            bail!("reviewer was cancelled; use review retry or review cancel");
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
                match verdict(project, review, event, &git) {
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
        // Recheck a historical refusal caused by the old exact gate-list check.
        let Some(event) = sealed(&events, &reviewer).filter(|e| {
            e.id != review.reviewer_after
                && (e.id != review.checked_event
                    || review.verdict.is_some()
                        && review.verdict_event.is_empty()
                        && review.attention.is_empty()
                    || review.attention
                        == "verdict must report each path-selected gate, in order, with exit 0")
        }) else {
            return Ok(());
        };
        let git = Git::new(ctx.runner, &review.repo);
        fetch_reviewer(ctx, project, review, &reviewer, event, &git)?;
        let verdict = match verdict(project, review, event, &git) {
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
        if installed
            .warnings
            .iter()
            .any(|warning| warning.contains("box pending"))
        {
            anyhow::bail!(
                "box pending: local installation finished; box installation still needs to complete: {}",
                installed.warnings.join("; ")
            );
        }
        Ok(())
    })
}

fn land_with_install(
    ctx: &Ctx,
    project: &Project,
    review: &mut Review,
    install: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let git = Git::new(ctx.runner, &review.repo);
    let candidate = review
        .verdict
        .as_ref()
        .context("landing verdict missing")?
        .candidate
        .clone();
    if !review.fast_forward {
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
                    bail!(
                        "{} changed during review; cancel and start a fresh pile",
                        member.thread
                    );
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
            thread::update(project, &member.thread, |t| {
                t.merged_sha = candidate.clone();
                t.merged_review = review.id.clone();
            })?;
        }
    }
    defer_members(project, review)?;
    if !review.push {
        if let Some(remote) = review
            .push_remote
            .as_deref()
            .filter(|remote| !remote.is_empty())
        {
            let target = format!("refs/heads/{}", review.integration);
            if !remote_contains(&git, remote, &target, &candidate)? {
                git.run(&["push", remote, &format!("{candidate}:{target}")])?;
                if !remote_contains(&git, remote, &target, &candidate)? {
                    bail!("integration publication not verified");
                }
            }
        }
        review.push = true;
        save(project, review)?;
    }
    if !review.install {
        if review.install_required {
            install()?;
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
        for id in &ids {
            crate::threads::resolve_automatically(ctx, project, id, "merged");
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
    let published = if review
        .push_remote
        .as_deref()
        .is_some_and(|remote| !remote.is_empty())
    {
        ", pushed"
    } else {
        ""
    };
    queue_notice(
        review,
        format!(
            "REVIEW {} merged {members} ({}{published})",
            review.id,
            &candidate[..candidate.len().min(7)]
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
        bail!("review has landed; resume publication and cleanup instead");
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
        bail!("no active pile review; start one with ha review {slug}");
    };
    if record.fast_forward || matches!(record.phase, Phase::Landing | Phase::Cancelling) {
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
            bail!(
                "{} is in {}; cancel its review before requesting a follow-up",
                id,
                record.id
            );
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
            bail!("{id} is in {}; cancel the review first", record.id);
        }
    }
    Ok(())
}
/// Classify old seals outside the open pile. The seal id is the cache key: a
/// later attempt/follow-up can be classified afresh, but idle passes do no git.
pub(crate) fn classify_old_seals(ctx: &Ctx, project: &Project, include_open: bool) -> Result<()> {
    let events = crate::events::checked(project)?;
    let harness = crate::harness::repos(&ctx.config_dir)?;
    let tasks = crate::task::list_with_errors(project).0;
    let mut heads: BTreeMap<String, String> = BTreeMap::new();
    let reviewers = reviewer_ids(project)?;
    for lane in thread::list(project) {
        if reviewers.contains(&lane.id) || lane.repo.is_empty() || !lane.merged_sha.is_empty() {
            continue;
        }
        let Some(event) = sealed(&events, &lane) else {
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
        let row = repository(ctx, project, Some(&lane.repo))?;
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

pub(crate) fn tick(ctx: &Ctx, project: &Project) -> Result<()> {
    if project.state_dir().join("reviews-enabled").exists() {
        classify_old_seals(ctx, project, false)?;
    }
    let mut first = None;
    for old in list(project)?.into_iter().filter(|r| !r.phase.closed()) {
        let Some(_lock) = try_operation_lock(ctx, &old.repo)? else {
            continue;
        };
        let mut record = load(project, &old.id)?;
        if let Err(error) = advance(ctx, project, &mut record)
            .and_then(|()| watch_no_verdict(ctx, project, &mut record))
        {
            record.attention = format!("{error:#}");
            save(project, &record)?;
            first.get_or_insert(error);
        }
    }
    if project.state_dir().join("reviews-enabled").exists() {
        let events = crate::events::checked(project)?;
        let threads = thread::list(project);
        let reviewers = reviewer_ids(project)?;
        let repos: std::collections::BTreeSet<_> = threads
            .iter()
            .filter(|t| !reviewers.contains(&t.id) && t.status != Status::Resolved)
            .map(|t| t.repo.clone())
            .collect();
        for repo in repos {
            if repo.is_empty() || pending(project, &repo, &events, &reviewers).is_empty() {
                continue;
            }
            let failed_local = threads.iter().any(|t| {
                same_repo(&t.repo, &repo)
                    && !reviewers.contains(&t.id)
                    && t.status == Status::Failed
                    && !t.is_remote()
            });
            let live_failed = if failed_local {
                let socket = project.coordinator().map(|c| c.socket).unwrap_or_default();
                let herdr = crate::herdr::Herdr::new(ctx.env.herdr_bin(), &socket, ctx.runner);
                let agents = herdr.agent_list()?;
                let panes = herdr.pane_list()?;
                threads.iter().any(|t| {
                    same_repo(&t.repo, &repo)
                        && thread::recoverable_agent(t, &threads, &agents, &panes).is_some()
                })
            } else {
                false
            };
            if live_failed
                || threads.iter().any(|t| {
                    same_repo(&t.repo, &repo)
                        && !reviewers.contains(&t.id)
                        && matches!(t.status, Status::Starting | Status::Open)
                        && (crate::events::latest_event(&events, &t.id, t.attempt.max(1))
                            .is_none_or(|e| e.payload.done.is_none())
                            || crate::threads::follow_up_pending_for_seal(
                                t,
                                crate::events::latest_done_event(&events, &t.id, t.attempt.max(1)),
                            ))
                })
            {
                continue;
            }
            let Some(_lock) = try_operation_lock(ctx, &repo)? else {
                continue;
            };
            if active_for_repo(ctx, &repo)?.is_none()
                && let Err(error) =
                    start_locked(ctx, project, repository(ctx, project, Some(&repo))?)
            {
                first.get_or_insert(error);
            }
        }
    }
    first.map_or(Ok(()), Err)
}
