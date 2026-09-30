//! Prune harness refs only after their lane is resolved or their commit landed.
//! Remote deletion checks the advertised tip and atomically leases the deletion.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::runner::{Cmd, Runner};
use crate::thread::{self, Status, Thread};

const TIMEOUT: Duration = Duration::from_secs(40);

fn git(runner: &dyn Runner, repo: &str, args: &[&str]) -> Result<String> {
    let out = runner.run(
        &Cmd::new("git", TIMEOUT)
            .args(["-C", repo])
            .args(args.iter().copied()),
    )?;
    if !out.success() {
        bail!("git {} in {repo}: {}", args.join(" "), out.error_text());
    }
    Ok(out.stdout)
}

fn refs(runner: &dyn Runner, repo: &str, remote: Option<&str>) -> Result<BTreeMap<String, String>> {
    let output = if let Some(remote) = remote {
        git(runner, repo, &["ls-remote", "--heads", remote])?
    } else {
        git(
            runner,
            repo,
            &[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                "refs/heads",
            ],
        )?
    };
    Ok(parse_refs(&output))
}

fn parse_refs(output: &str) -> BTreeMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let sha = fields.next()?;
            // ls-remote can advertise a zero OID while another push is deleting
            // the ref. That is a deletion marker, not a moved branch tip.
            if sha.bytes().all(|byte| byte == b'0') {
                return None;
            }
            let name = fields.next()?.strip_prefix("refs/heads/")?;
            Some((name.to_string(), sha.to_string()))
        })
        .collect()
}

fn checked_out(runner: &dyn Runner, repo: &str) -> Result<BTreeSet<String>> {
    let output = git(runner, repo, &["worktree", "list", "--porcelain"])?;
    Ok(output
        .lines()
        .filter_map(|line| line.strip_prefix("branch refs/heads/").map(str::to_owned))
        .collect())
}

fn checked_threads(project: &Project) -> Result<Vec<Thread>> {
    let (threads, errors) = thread::list_with_errors(project);
    if let Some(error) = errors.first() {
        bail!("thread records unreadable: {error:#}");
    }
    Ok(threads)
}

fn active_refs(root: &Path) -> Result<BTreeSet<(String, String)>> {
    let mut active = BTreeSet::new();
    for slug in project::list_slugs(root) {
        let project = Project::load(root, &slug)?;
        for record in checked_threads(&project)? {
            if record.status != Status::Resolved {
                active.insert((record.repo, record.branch));
            }
        }
        for review in crate::review::list(&project)? {
            if !review.phase.closed() {
                active.insert((review.repo, review.candidate_branch));
            }
        }
    }
    Ok(active)
}

fn harness_ref(name: &str) -> bool {
    (name.starts_with("hp/") || name.starts_with("review/"))
        && !name.contains("..")
        && !name.contains('@')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
}

// A failed compare-and-delete can mean another cleanup already removed the ref.
// Interpret its exit only after the postcondition query.
fn deletion_command(
    runner: &dyn Runner,
    repo: &str,
    args: &[&str],
) -> Result<crate::runner::Output> {
    let cmd = Cmd::new("git", TIMEOUT)
        .args(["-C", repo])
        .args(args.iter().copied());
    runner.run(&cmd)
}

fn delete_local(runner: &dyn Runner, repo: &str, branch: &str, expected: &str) -> Result<()> {
    if checked_out(runner, repo)?.contains(branch) {
        bail!("branch {branch} is still checked out; not removing it");
    }
    let name = format!("refs/heads/{branch}");
    // update-ref compares the old value atomically. Unlike branch -D it cannot
    // lose a check/delete race to another cleanup process.
    let out = deletion_command(runner, repo, &["update-ref", "-d", &name, expected])?;
    if out.success() {
        return Ok(());
    }
    match refs(runner, repo, None)?.get(branch) {
        None => Ok(()),
        Some(actual) if actual != expected => Err(crate::refusal::error(
            format!("branch {branch} moved; not removing it"),
            "ha thread show <project> <thread> (verify the branch head before cleanup)",
        )),
        _ => {
            bail!(
                "git update-ref -d {name} {expected} in {repo}: {}",
                out.error_text()
            );
        }
    }
}

fn delete_remote(
    runner: &dyn Runner,
    repo: &str,
    url: &str,
    branch: &str,
    expected: &str,
) -> Result<()> {
    let current = refs(runner, repo, Some(url))?;
    if !current.contains_key(branch) {
        return Ok(());
    }
    if current.get(branch).map(String::as_str) != Some(expected) {
        return Err(crate::refusal::error(
            format!(
                "published branch {branch} moved from {expected} to {}; not removing it",
                current[branch]
            ),
            "ha thread show <project> <thread> (verify the branch head before cleanup)",
        ));
    }
    let deletion = format!(":refs/heads/{branch}");
    let lease = format!("--force-with-lease=refs/heads/{branch}:{expected}");
    let out = deletion_command(runner, repo, &["push", &lease, url, &deletion])?;
    if out.success() {
        return Ok(());
    }
    match refs(runner, repo, Some(url))?.get(branch) {
        None => Ok(()),
        Some(actual) if actual != expected => Err(crate::refusal::error(
            format!("published branch {branch} moved from {expected} to {actual}; not removing it"),
            "ha thread show <project> <thread> (verify the branch head before cleanup)",
        )),
        _ => {
            bail!(
                "git push {lease} {url} {deletion} in {repo}: {}",
                out.error_text()
            );
        }
    }
}

/// Pin the checked-out branch tip for explicit retained-worktree removal.
/// Box lanes must match their published tip or immutable seal; local lanes
/// need no publication.
pub(crate) fn require_published_tip(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
) -> Result<String> {
    if record.branch.is_empty() || !harness_ref(&record.branch) {
        bail!("{} has no owned branch", record.id);
    }
    if record.is_remote() {
        let (settings, _) = project.read_project_md()?;
        let profile = crate::remote::machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            record.machine_route(),
        )?;
        let (_, url) =
            crate::threads::box_repo_row(&ctx.config_dir, &settings, &profile.label, &record.repo)?;
        let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
        let script = crate::remote::with_path(
            &machine.path,
            &format!(
                "cd {} && git symbolic-ref --quiet HEAD && git rev-parse HEAD && git rev-parse --verify {}",
                crate::remote::quote(&record.worktree_path),
                crate::remote::quote(&format!("refs/heads/{}", record.branch))
            ),
        );
        let out = crate::remote::ssh(ctx.runner, &profile.target, &script, None, TIMEOUT)?;
        if !out.success() {
            bail!(
                "box checkout {}: {}",
                record.worktree_path,
                out.error_text()
            );
        }
        let mut lines = out.stdout.lines();
        let checked_out = lines.next().unwrap_or_default();
        let head = lines.next().unwrap_or_default();
        let tip = lines.next().unwrap_or_default();
        if checked_out != format!("refs/heads/{}", record.branch) || head != tip || tip.is_empty() {
            bail!(
                "{} is no longer checked out at the published branch tip; not removing it",
                record.worktree_path
            );
        }
        let tip = tip.to_string();
        let remote = refs(ctx.runner, &record.repo, Some(&url))?;
        let local = refs(ctx.runner, &record.repo, None)?;
        let events = crate::events::list(project);
        let latest_seal = events
            .iter()
            .filter(|event| event.thread == record.id)
            .filter_map(|event| event.payload.done.as_ref())
            .rfind(|done| done.published_ref.is_some());
        let sealed = latest_seal.is_some_and(|done| {
            let reference = crate::ops::seal_ref(&record.branch, &done.sha);
            done.sha == tip
                && done.published_ref.as_deref() == Some(reference.as_str())
                && remote.get(&reference) == Some(&tip)
        });
        let matches_tip = |actual: &String| {
            actual == &tip || (sealed && !record.base.is_empty() && actual == &record.base)
        };
        if (latest_seal.is_some() && !sealed)
            || (!sealed && remote.get(&record.branch) != Some(&tip))
            || remote
                .get(&record.branch)
                .is_some_and(|sha| !matches_tip(sha))
            || local
                .get(&record.branch)
                .is_some_and(|sha| !matches_tip(sha))
        {
            bail!(
                "branch {} has unpushed commits or has moved since publication",
                record.branch
            );
        }
        Ok(tip)
    } else {
        let checked_out = git(
            ctx.runner,
            &record.worktree_path,
            &["symbolic-ref", "--quiet", "HEAD"],
        )?;
        if checked_out.trim() != format!("refs/heads/{}", record.branch) {
            bail!(
                "{} no longer checks out {}; not removing it",
                record.worktree_path,
                record.branch
            );
        }
        let head = git(ctx.runner, &record.worktree_path, &["rev-parse", "HEAD"])?;
        let tip = git(
            ctx.runner,
            &record.repo,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", record.branch),
            ],
        )?;
        if head.trim() != tip.trim() {
            bail!(
                "{} has moved since the branch check; not removing it",
                record.worktree_path
            );
        }
        Ok(tip.trim().to_string())
    }
}

/// Called after a worktree has gone. A failed deletion remains retryable via
/// the thread's durable cleanup_pending marker.
pub(crate) fn resolved_thread(ctx: &Ctx, project: &Project, record: &Thread) -> Result<()> {
    if record.repo.is_empty() || !harness_ref(&record.branch) {
        return Ok(());
    }
    if checked_threads(project)?.iter().any(|t| {
        t.id != record.id
            && t.repo == record.repo
            && t.branch == record.branch
            && t.status != Status::Resolved
    }) {
        bail!("branch {} still belongs to an active thread", record.branch);
    }
    if checked_out(ctx.runner, &record.repo)?.contains(&record.branch) {
        bail!(
            "branch {} is still checked out; not removing it anywhere",
            record.branch
        );
    }
    let review_pin = if !record.merged_review.is_empty() {
        crate::review::load(project, &record.merged_review)?
            .members
            .into_iter()
            .find(|m| m.thread == record.id)
            .map(|m| m.sha)
    } else if !record.review_id.is_empty() {
        let review = crate::review::load(project, &record.review_id)?;
        review
            .fast_forward
            .then_some(review.verdict)
            .flatten()
            .map(|v| v.candidate)
    } else {
        None
    };
    let events = crate::events::list(project);
    let last_box_seal = events
        .iter()
        .filter(|e| e.thread == record.id)
        .filter_map(|e| e.payload.done.as_ref())
        .rfind(|d| d.published_ref.is_some());
    let has_seal_refs = last_box_seal.is_some();
    let retained_tip = record
        .cleanup_reason
        .strip_prefix("retained worktree removal: ");
    let expected = if has_seal_refs && record.is_remote() && !record.base.is_empty() {
        Some(record.base.as_str())
    } else {
        retained_tip.or(if has_seal_refs {
            None
        } else {
            review_pin.as_deref()
        })
    };
    let local = refs(ctx.runner, &record.repo, None)?;
    // The box's mutable published branch can still be at base while the Mac
    // local ref has advanced to the immutable seal used by the landed review.
    // Accept that exact review member, fast-forwarded reviewer verdict, or
    // verified retained-removal pin, not an arbitrary later box seal.
    let retained_seal = retained_tip.filter(|tip| {
        record.is_remote() && Some(*tip) == last_box_seal.map(|done| done.sha.as_str())
    });
    let merged_seal = if retained_seal.is_some() {
        retained_seal
    } else if record.is_remote()
        && has_seal_refs
        && (!record.merged_review.is_empty() || !record.review_id.is_empty())
    {
        review_pin.as_deref().filter(|pin| {
            events.iter().any(|event| {
                event.thread == record.id
                    && event
                        .payload
                        .done
                        .as_ref()
                        .is_some_and(|done| done.sha == *pin && done.published_ref.is_some())
            })
        })
    } else {
        None
    };
    if let (Some(expected), Some(actual)) = (expected, local.get(&record.branch))
        && actual != expected
        && merged_seal != Some(actual.as_str())
    {
        bail!(
            "branch {} moved beyond its sealed cleanup tip",
            record.branch
        );
    }
    let (settings, _) = project.read_project_md()?;
    let row = settings.repos.iter().find(|row| row.path == record.repo);
    let url = if record.is_remote() {
        let profile = crate::remote::machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            record.machine_route(),
        )?;
        let (box_repo, url) =
            crate::threads::box_repo_row(&ctx.config_dir, &settings, &profile.label, &record.repo)?;
        let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
        let refname = crate::remote::quote(&format!("refs/heads/{}", record.branch));
        // The Mac and published mutable branch stay at the start tip, but
        // the box checkout advances as the lane commits its sealed result.
        let box_expected = last_box_seal.map(|done| done.sha.as_str()).or(expected);
        let pin = box_expected.map_or(String::new(), |sha| {
            format!(
                "if [ -n \"$old\" ] && [ \"$old\" != {} ]; then echo 'box branch moved beyond its sealed cleanup tip' >&2; exit 1; fi; ",
                crate::remote::quote(sha)
            )
        });
        let script = crate::remote::with_path(
            &machine.path,
            &format!(
                "cd {} || exit $?; worktrees=$(git worktree list --porcelain) || exit $?; if printf '%s\\n' \"$worktrees\" | grep -Fqx -- {}; then echo 'branch still checked out' >&2; exit 1; fi; old=$(git for-each-ref --format='%(objectname)' {}) || exit $?; {pin}if [ -n \"$old\" ]; then git update-ref -d {} \"$old\" || {{ after=$(git for-each-ref --format='%(objectname)' {}) || exit $?; [ -z \"$after\" ] || exit 1; }}; fi",
                crate::remote::quote(&box_repo),
                crate::remote::quote(&format!("branch refs/heads/{}", record.branch)),
                refname,
                refname,
                refname
            ),
        );
        let out = crate::remote::ssh(ctx.runner, &profile.target, &script, None, TIMEOUT)?;
        if !out.success() {
            bail!("box branch {}: {}", record.branch, out.error_text());
        }
        Some(url)
    } else {
        row.and_then(|r| r.publish_url.clone().or_else(|| r.push_remote.clone()))
    };
    let remote = match &url {
        Some(url) => refs(ctx.runner, &record.repo, Some(url))?,
        None => BTreeMap::new(),
    };
    if let (Some(expected), Some(actual)) = (expected, remote.get(&record.branch))
        && actual != expected
        && retained_seal != Some(actual.as_str())
    {
        bail!(
            "published branch {} moved beyond its sealed cleanup tip",
            record.branch
        );
    }
    // Every immutable seal owns its own publication. Delete all of them,
    // including superseded reviewer verdicts, before clearing cleanup_pending.
    if let Some(url) = &url {
        for event in events.into_iter().filter(|e| e.thread == record.id) {
            if let Some(done) = event.payload.done
                && let Some(reference) = done.published_ref
            {
                if reference != crate::ops::seal_ref(&record.branch, &done.sha) {
                    bail!("seal {} has an invalid publication ref", event.id);
                }
                if let Some(actual) = local.get(&reference) {
                    delete_local(ctx.runner, &record.repo, &reference, actual)?;
                }
                delete_remote(ctx.runner, &record.repo, url, &reference, &done.sha)?;
            }
        }
    }
    for (url, sha) in &record.review_sources {
        delete_remote(ctx.runner, &record.repo, url, &record.branch, sha)?;
    }
    if let Some(sha) = local.get(&record.branch) {
        delete_local(ctx.runner, &record.repo, &record.branch, sha)?;
    }
    if let (Some(url), Some(sha)) = (url, remote.get(&record.branch)) {
        delete_remote(ctx.runner, &record.repo, &url, &record.branch, sha)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    repo: String,
    branch: String,
    local: Option<String>,
    remote: Option<(String, String)>,
}

fn candidates(ctx: &Ctx, tolerate_unreachable: bool) -> Result<(Vec<Candidate>, Vec<String>)> {
    // Fetch each distinct remote once, in parallel; sequential ls-remote calls
    // previously serialized network latency across projects.
    let mut urls = BTreeSet::new();
    let mut repos = BTreeSet::new();
    for slug in project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        let (settings, _) = project.read_project_md()?;
        for row in settings.repos {
            if Path::new(&row.path).is_dir() {
                repos.insert(row.path.clone());
                if let Some(url) = row.publish_url.or(row.push_remote) {
                    urls.insert((row.path, url));
                }
            }
        }
    }
    let commands: Vec<_> = urls
        .iter()
        .map(|(repo, url)| {
            Cmd::new("git", Duration::from_secs(10))
                .args(["-C", repo, "ls-remote", "--heads", url])
                .env("GIT_TERMINAL_PROMPT", "0")
                .own_group()
        })
        .collect();
    let mut remotes = BTreeMap::new();
    let mut unreachable = Vec::new();
    for ((repo, url), output) in urls.into_iter().zip(ctx.runner.run_parallel(&commands)) {
        match output {
            Ok(output) if output.success() => {
                remotes.insert((repo, url), parse_refs(&output.stdout));
            }
            answer => {
                let detail = match answer {
                    Ok(output) => output.error_text(),
                    Err(error) => format!("{error:#}"),
                };
                if !tolerate_unreachable {
                    bail!("git ls-remote --heads in {repo}: {detail}");
                }
                unreachable.push(format!("{repo}: remote unreachable ({detail})"));
            }
        }
    }
    // Local ref inventories and checked-out branch lists are independent.
    // Query each repo once, in one parallel batch, even when multiple projects
    // name that same checkout.
    let commands: Vec<_> = repos
        .iter()
        .flat_map(|repo| {
            [
                Cmd::new("git", TIMEOUT).args([
                    "-C",
                    repo,
                    "for-each-ref",
                    "--format=%(objectname) %(refname)",
                    "refs/heads",
                ]),
                Cmd::new("git", TIMEOUT).args(["-C", repo, "worktree", "list", "--porcelain"]),
            ]
        })
        .collect();
    let mut local_by_repo = BTreeMap::new();
    let mut in_use_by_repo = BTreeMap::new();
    let mut answers = ctx.runner.run_parallel(&commands).into_iter();
    for repo in &repos {
        let refs = answers.next().expect("one refs answer per repo")?;
        if !refs.success() {
            bail!("git for-each-ref in {repo}: {}", refs.error_text());
        }
        let worktrees = answers.next().expect("one worktree answer per repo")?;
        if !worktrees.success() {
            bail!("git worktree list in {repo}: {}", worktrees.error_text());
        }
        local_by_repo.insert(repo.clone(), parse_refs(&refs.stdout));
        in_use_by_repo.insert(
            repo.clone(),
            worktrees
                .stdout
                .lines()
                .filter_map(|line| line.strip_prefix("branch refs/heads/").map(str::to_owned))
                .collect::<BTreeSet<_>>(),
        );
    }
    let mut result = Vec::new();
    let active_refs = active_refs(&ctx.root)?;
    let mut merged_by_repo = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for slug in project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        let (settings, _) = project.read_project_md()?;
        let threads = checked_threads(&project)?;

        for row in settings.repos {
            if !Path::new(&row.path).is_dir() {
                continue;
            }
            let local = &local_by_repo[&row.path];
            let in_use = &in_use_by_repo[&row.path];
            let remote_url = row.publish_url.as_ref().or(row.push_remote.as_ref());
            let remote = remote_url
                .and_then(|url| remotes.get(&(row.path.clone(), url.clone())))
                .cloned()
                .unwrap_or_default();
            let mut names: BTreeSet<_> = local.keys().chain(remote.keys()).cloned().collect();
            names.retain(|name| harness_ref(name) && !round_ref(name) && !in_use.contains(name));
            if names.is_empty() {
                continue;
            }
            let base = row.branch.as_deref().unwrap_or("main");
            // Traverse the base history once per repo/base. A merged branch
            // can point at an ancestor that is no longer any local ref tip;
            // remote-only refs can do so too, if the object is present here.
            let key = (row.path.clone(), base.to_owned());
            if !merged_by_repo.contains_key(&key) {
                let history = git(ctx.runner, &row.path, &["rev-list", base])?
                    .lines()
                    .map(str::to_owned)
                    .collect();
                merged_by_repo.insert(key.clone(), history);
            }
            let merged = &merged_by_repo[&key];
            for name in names {
                let resolved = threads.iter().any(|t| {
                    t.repo == row.path
                        && t.branch == name
                        && t.status == Status::Resolved
                        && !t.cleanup_pending
                });
                let landed = local
                    .get(&name)
                    .into_iter()
                    .chain(remote.get(&name))
                    .all(|sha| merged.contains(sha));
                // An open thread still owns its ref, even when it has not
                // diverged from the base yet.
                let active = active_refs.contains(&(row.path.clone(), name.clone()));
                if active || !resolved || !landed {
                    continue;
                }
                result.push(Candidate {
                    repo: row.path.clone(),
                    branch: name.clone(),
                    local: local.get(&name).cloned(),
                    remote: remote_url
                        .and_then(|url| remote.get(&name).map(|sha| (url.clone(), sha.clone()))),
                });
            }
        }
    }
    result.sort_by(|a, b| (&a.repo, &a.branch).cmp(&(&b.repo, &b.branch)));
    result.dedup_by(|a, b| a.repo == b.repo && a.branch == b.branch);
    Ok((result, unreachable))
}

// Round branches predate pile reviews. A pile branch has another path component.
fn round_ref(name: &str) -> bool {
    let Some(number) = name.strip_prefix("review/r") else {
        return false;
    };
    let (digits, suffix) = number
        .split_once('-')
        .map_or((number, None), |(a, b)| (a, Some(b)));
    !digits.is_empty()
        && digits.bytes().all(|c| c.is_ascii_digit())
        && suffix.is_none_or(|value| !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()))
}

fn round_worktrees(runner: &dyn Runner, repo: &str) -> Result<BTreeMap<String, String>> {
    let text = git(runner, repo, &["worktree", "list", "--porcelain"])?;
    let mut result = BTreeMap::new();
    for block in text.split("\n\n") {
        let path = block
            .lines()
            .find_map(|line| line.strip_prefix("worktree "));
        let branch = block
            .lines()
            .find_map(|line| line.strip_prefix("branch refs/heads/"));
        if let (Some(path), Some(branch)) = (path, branch) {
            let root = Path::new(repo).join(".worktrees");
            let path_buf = Path::new(path);
            let expected = branch.replacen("review/", "review-", 1);
            let matching_folder = path_buf
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name == expected
                        || name
                            .strip_prefix(&format!("{expected}-"))
                            .is_some_and(|suffix| {
                                !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit())
                            })
                });
            if round_ref(branch) && path_buf.parent() == Some(root.as_path()) && matching_folder {
                result.insert(branch.to_owned(), path.to_owned());
            }
        }
    }
    Ok(result)
}

fn sweep_rounds(ctx: &Ctx, mut log: impl FnMut(&str)) -> Result<()> {
    let mut rows = BTreeMap::<String, Vec<crate::project::Repo>>::new();
    for slug in project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        let (settings, _) = project.read_project_md()?;
        for row in settings.repos {
            rows.entry(row.path.clone()).or_default().push(row);
        }
    }
    for row in crate::harness::repos(&ctx.config_dir)? {
        rows.entry(row.path.clone()).or_default().push(row);
    }
    for (repo, repo_rows) in rows {
        if !Path::new(&repo).is_dir() {
            continue;
        }
        let base = repo_rows
            .iter()
            .find_map(|row| row.branch.as_deref())
            .unwrap_or("main");
        let worktrees = round_worktrees(ctx.runner, &repo)?;
        for (branch, sha) in refs(ctx.runner, &repo, None)? {
            if !round_ref(&branch) {
                continue;
            }
            let ancestry = ctx.runner.run(&Cmd::new("git", TIMEOUT).args([
                "-C",
                &repo,
                "merge-base",
                "--is-ancestor",
                &sha,
                base,
            ]))?;
            if !ancestry.success() {
                log(&format!(
                    "one-time branch sweep: {repo} {branch} keeps unmerged commits"
                ));
                continue;
            }
            if let Some(path) = worktrees.get(&branch) {
                let disposable =
                    crate::worktrees::disposable_for_rows(&ctx.config_dir, &repo, &repo_rows)?;
                let inspection =
                    crate::worktrees::inspect_local(ctx.runner, &repo, path, &disposable, false)?;
                if !inspection.dirty.is_empty() {
                    log(&format!(
                        "one-time branch sweep: {repo} {branch} keeps uncommitted changes"
                    ));
                    continue;
                }
                if !inspection.ignored_data.is_empty() {
                    let folders = inspection
                        .ignored_data
                        .iter()
                        .map(|entry| entry.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    log(&format!(
                        "one-time branch sweep: {repo} {branch} keeps ignored data: {folders}"
                    ));
                    continue;
                }
                crate::git::worktree_remove(ctx.runner, &repo, path)?;
            }
            // A checkout elsewhere still owns the branch. Only the named round
            // worktrees are eligible for automatic removal.
            if checked_out(ctx.runner, &repo)?.contains(&branch) {
                log(&format!(
                    "one-time branch sweep: {repo} {branch} keeps another checkout"
                ));
                continue;
            }
            delete_local(ctx.runner, &repo, &branch, &sha)?;
        }
    }
    Ok(())
}

/// Sweep merged, resolved lanes and local round leftovers. The v2 marker is
/// written after cleanup succeeds; retained worktrees and branches stay intact.
pub(crate) fn sweep_once(ctx: &Ctx, mut log: impl FnMut(&str)) -> Result<()> {
    let marker = ctx.root.join(".branch-sweep-v2.json");
    if marker.exists() {
        return Ok(());
    }
    let active = active_refs(&ctx.root)?;
    let mut kept = BTreeSet::new();
    for slug in project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        let (settings, _) = project.read_project_md()?;
        let records = checked_threads(&project)?;
        let mut remote_paths: BTreeMap<String, Vec<&Thread>> = BTreeMap::new();
        for record in &records {
            if record.status == Status::Resolved
                && !record.repo.is_empty()
                && harness_ref(&record.branch)
                && !round_ref(&record.branch)
                && record.kind == thread::Kind::Worktree
                && !record.worktree_path.is_empty()
                && record.is_remote()
            {
                remote_paths
                    .entry(record.machine_route().to_owned())
                    .or_default()
                    .push(record);
            }
        }
        let mut present = BTreeSet::new();
        for (route, paths) in remote_paths {
            let profile = crate::remote::machine_profile(
                ctx.runner,
                &ctx.env.herdr_bin(),
                &ctx.config_dir,
                &route,
            )?;
            let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
            for batch in paths.chunks(100) {
                let mut script = String::new();
                for record in batch {
                    script.push_str(&format!(
                        "if test -d {}; then printf '%s\\n' {}; fi\n",
                        crate::remote::quote(&record.worktree_path),
                        crate::remote::quote(&record.id),
                    ));
                }
                let script = crate::remote::with_path(&machine.path, &script);
                let output =
                    crate::remote::ssh(ctx.runner, &profile.target, &script, None, TIMEOUT)?;
                if !output.success() {
                    bail!(
                        "worktree inventory on {}: {}",
                        profile.label,
                        output.error_text()
                    );
                }
                present.extend(output.stdout.lines().map(str::to_owned));
            }
        }
        for record in &records {
            if record.status != Status::Resolved
                || record.repo.is_empty()
                || !harness_ref(&record.branch)
                || round_ref(&record.branch)
                || record.kind != thread::Kind::Worktree
                || record.worktree_path.is_empty()
                || (record.is_remote() && !present.contains(&record.id))
                || (!record.is_remote() && !Path::new(&record.worktree_path).exists())
            {
                continue;
            }
            let Some(row) = settings.repos.iter().find(|row| row.path == record.repo) else {
                continue;
            };
            let base = row.branch.as_deref().unwrap_or("main");
            let local = refs(ctx.runner, &record.repo, None)?;
            let url = row.publish_url.as_ref().or(row.push_remote.as_ref());
            let remote = if let Some(url) = url {
                refs(ctx.runner, &record.repo, Some(url))?
            } else {
                BTreeMap::new()
            };
            let local_tip = local.get(&record.branch);
            let remote_tip = remote.get(&record.branch);
            let Some(tip) = local_tip.or(remote_tip) else {
                continue;
            };
            if local_tip
                .zip(remote_tip)
                .is_some_and(|(local, remote)| local != remote)
            {
                continue;
            }
            if active.contains(&(record.repo.clone(), record.branch.clone())) {
                continue;
            }
            let cmd = Cmd::new("git", TIMEOUT).args([
                "-C",
                &record.repo,
                "merge-base",
                "--is-ancestor",
                tip,
                base,
            ]);
            let result = ctx.runner.run(&cmd)?;
            if !result.success() {
                continue;
            }
            let inspection = crate::threads::inspect_worktree_for_removal(ctx, &project, record)?;
            if !inspection.dirty.is_empty() || !inspection.ignored_data.is_empty() {
                if kept.insert((record.repo.clone(), record.branch.clone())) {
                    let reason = if !inspection.dirty.is_empty() {
                        "keeps uncommitted changes".to_owned()
                    } else {
                        format!(
                            "keeps ignored data: {}",
                            inspection
                                .ignored_data
                                .iter()
                                .map(|data| data.path.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    log(&format!(
                        "one-time branch sweep: {} {reason}; worktree and branch kept",
                        record.id
                    ));
                }
                continue;
            }
            crate::threads::remove_worktree(ctx, &project, record)?;
            resolved_thread(ctx, &project, record)?;
        }
    }
    // Include resolved, merged refs whose worktree was already removed.
    let (plan, _) = candidates(ctx, false)?;
    for item in plan {
        if kept.contains(&(item.repo.clone(), item.branch.clone())) {
            continue;
        }
        if let Some(sha) = &item.local {
            delete_local(ctx.runner, &item.repo, &item.branch, sha)?;
        }
        if let Some((url, sha)) = &item.remote {
            delete_remote(ctx.runner, &item.repo, url, &item.branch, sha)?;
        }
    }
    sweep_rounds(ctx, &mut log)?;
    project::write_json(&marker, &true)?;
    let old_marker = ctx.root.join(".branch-sweep-v1.json");
    if old_marker.exists() {
        std::fs::remove_file(old_marker)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }
    #[test]
    fn remote_deletion_marker_is_not_a_branch_tip() {
        let refs = parse_refs(
            "0000000000000000000000000000000000000000\trefs/heads/hp/demo/deleting\n\
             abcdef0123456789abcdef0123456789abcdef01\trefs/heads/hp/demo/other\n",
        );
        assert!(!refs.contains_key("hp/demo/deleting"));
        assert_eq!(
            refs["hp/demo/other"],
            "abcdef0123456789abcdef0123456789abcdef01"
        );
    }

    fn configured() -> (crate::testkit::Fx, tempfile::TempDir) {
        let fx = crate::testkit::fixture();
        let bare = tempfile::tempdir().unwrap();
        run(bare.path(), &["init", "-q", "--bare"]);
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos[0].push_remote = Some(bare.path().to_str().unwrap().into());
        settings.repos[0].branch = Some("main".into());
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        (fx, bare)
    }

    fn retained_lane(fx: &crate::testkit::Fx, branch: &str) -> Thread {
        let path = fx.world.home.path().join("retained");
        run(
            &fx.repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                path.to_str().unwrap(),
            ],
        );
        thread::allocate(&fx.project, |t| {
            t.repo = fx.repo.to_string_lossy().into_owned();
            t.branch = branch.into();
            t.worktree_path = path.to_string_lossy().into_owned();
            t.status = Status::Resolved;
        })
        .unwrap()
    }

    #[test]
    fn local_retained_checkout_needs_no_published_branch() {
        let (fx, bare) = configured();
        let mut record = retained_lane(&fx, "hp/demo/local");
        let tip = run(&fx.repo, &["rev-parse", &record.branch]);
        let remote = bare.path().to_str().unwrap();
        assert!(
            !refs(&crate::runner::RealRunner, &record.repo, Some(remote))
                .unwrap()
                .contains_key(&record.branch)
        );
        assert_eq!(
            require_published_tip(&fx.world.ctx(), &fx.project, &record).unwrap(),
            tip
        );
        run(&fx.repo, &["worktree", "remove", &record.worktree_path]);
        record.cleanup_reason = format!("retained worktree removal: {tip}");
        resolved_thread(&fx.world.ctx(), &fx.project, &record).unwrap();
        assert!(
            !refs(&crate::runner::RealRunner, &record.repo, None)
                .unwrap()
                .contains_key(&record.branch)
        );
    }

    #[test]
    fn local_retained_checkout_needs_no_publication_destination() {
        let fx = crate::testkit::fixture();
        let record = retained_lane(&fx, "hp/demo/no-remote");
        assert_eq!(
            require_published_tip(&fx.world.ctx(), &fx.project, &record).unwrap(),
            run(&fx.repo, &["rev-parse", &record.branch])
        );
    }

    #[test]
    fn local_retained_checkout_refuses_head_different_from_branch() {
        let (fx, _bare) = configured();
        let mut record = retained_lane(&fx, "hp/demo/moved");
        // A checkout at the recorded path can be replaced by a different repo
        // with the same branch name; its HEAD must not authorize deleting ours.
        let other = tempfile::tempdir().unwrap();
        run(other.path(), &["init", "-q", "-b", &record.branch]);
        run(other.path(), &["config", "user.name", "Test"]);
        run(other.path(), &["config", "user.email", "test@example.com"]);
        std::fs::write(other.path().join("README"), "different").unwrap();
        run(other.path(), &["add", "README"]);
        run(other.path(), &["commit", "-qm", "different"]);
        record.worktree_path = other.path().to_string_lossy().into_owned();
        let error = require_published_tip(&fx.world.ctx(), &fx.project, &record)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("has moved since the branch check"),
            "{error}"
        );
    }

    #[test]
    fn box_retained_checkout_refuses_moved_published_branch() {
        let (fx, bare) = configured();
        let branch = "hp/demo/box";
        let record = thread::allocate(&fx.project, |t| {
            t.repo = fx.repo.to_string_lossy().into_owned();
            t.branch = branch.into();
            t.worktree_path = "/box/retained".into();
            t.machine = "box".into();
            t.status = Status::Resolved;
        })
        .unwrap();
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos[0].box_path = Some("/box/repo".into());
        settings.repos[0].publish_url = Some(bare.path().to_string_lossy().into_owned());
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        let sealed = run(&fx.repo, &["rev-parse", "HEAD"]);
        run(&fx.repo, &["branch", branch, &sealed]);
        run(
            &fx.repo,
            &["push", "-q", bare.path().to_str().unwrap(), branch],
        );
        let moved = run(&fx.repo, &["commit-tree", "HEAD^{tree}", "-m", "moved"]);
        fx.world
            .runner
            .on("machine list --json", crate::runner::fake::ok("[]"));
        fx.world.runner.on_fn(
            |cmd| cmd.program == "ssh",
            move |_| {
                Ok(crate::runner::fake::ok(&format!(
                    "refs/heads/{branch}\n{moved}\n{moved}\n"
                )))
            },
        );
        let error = require_published_tip(&fx.world.ctx(), &fx.project, &record)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unpushed commits or has moved since publication"),
            "{error}"
        );
    }

    fn sealed_retained_box() -> (crate::testkit::Fx, tempfile::TempDir, Thread, String) {
        let (fx, bare) = configured();
        let branch = "hp/demo/sealed-box";
        let base = run(&fx.repo, &["rev-parse", "HEAD"]);
        run(&fx.repo, &["branch", branch, &base]);
        run(
            &fx.repo,
            &["push", "-q", bare.path().to_str().unwrap(), branch],
        );
        let box_repo = fx.world.home.path().join("box-repo");
        std::fs::create_dir(&box_repo).unwrap();
        run(&box_repo, &["init", "-q", "-b", "main"]);
        run(&box_repo, &["config", "user.name", "Test"]);
        run(&box_repo, &["config", "user.email", "test@example.com"]);
        run(
            &box_repo,
            &["fetch", "-q", fx.repo.to_str().unwrap(), "main"],
        );
        run(&box_repo, &["reset", "--hard", "FETCH_HEAD"]);
        let wt = fx.world.home.path().join("box-retained");
        run(
            &box_repo,
            &["worktree", "add", "-q", "-b", branch, wt.to_str().unwrap()],
        );
        let sha = crate::testkit::commit_file(&wt, "result.txt", "sealed\n", "sealed");
        let reference = crate::ops::seal_ref(branch, &sha);
        run(
            &box_repo,
            &[
                "push",
                "-q",
                bare.path().to_str().unwrap(),
                &format!("{sha}:refs/heads/{reference}"),
            ],
        );
        let record = thread::allocate(&fx.project, |t| {
            t.kind = crate::thread::Kind::Worktree;
            t.status = Status::Resolved;
            t.machine = "box".into();
            t.repo = fx.repo.to_string_lossy().into_owned();
            t.branch = branch.into();
            t.base = base;
            t.merged_sha = sha.clone();
            t.worktree_path = wt.to_string_lossy().into_owned();
            t.cwd = t.worktree_path.clone();
            t.workspace_id = "w2".into();
            t.tab_id = "w2:t1".into();
            t.pane_id = "w2:p1".into();
        })
        .unwrap();
        let id = fx.seal_done(&record.id, 1, 1, &sha, "sealed report");
        let mut event = crate::events::load(&fx.project, &id).unwrap();
        event.payload.done.as_mut().unwrap().published_ref = Some(reference);
        std::fs::write(
            fx.project
                .state_dir()
                .join("events")
                .join(format!("{id}.toml")),
            toml::to_string(&event).unwrap(),
        )
        .unwrap();
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos[0].box_path = Some(box_repo.to_string_lossy().into_owned());
        settings.repos[0].publish_url = Some(bare.path().to_string_lossy().into_owned());
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        fx.world
            .runner
            .on("machine list --json", crate::runner::fake::ok("[]"));
        fx.world.runner.on_fn(
            |cmd| cmd.program == "ssh",
            |cmd| {
                use crate::runner::Runner;
                crate::runner::RealRunner
                    .run(&Cmd::new("sh", cmd.timeout).args(["-c", cmd.args.last().unwrap()]))
            },
        );
        // Closing the coordinator must not hide the box's own herdr server.
        std::fs::remove_file(fx.project.coordinator().unwrap().socket).unwrap();
        (fx, bare, record, sha)
    }

    #[test]
    fn sealed_box_retained_checkout_is_removed_with_mutable_refs_at_base() {
        for (local_at_seal, published_at_seal) in [(false, false), (true, false), (true, true)] {
            let (fx, bare, record, sha) = sealed_retained_box();
            if local_at_seal {
                run(
                    &fx.repo,
                    &[
                        "fetch",
                        "-q",
                        bare.path().to_str().unwrap(),
                        &crate::ops::seal_ref(&record.branch, &sha),
                    ],
                );
                run(
                    &fx.repo,
                    &["update-ref", &format!("refs/heads/{}", record.branch), &sha],
                );
            }
            if published_at_seal {
                run(
                    &fx.repo,
                    &["push", "-q", bare.path().to_str().unwrap(), &record.branch],
                );
            }
            crate::threads::remove_kept_worktree(&fx.world.ctx(), "demo", &record.id).unwrap();
            assert!(!Path::new(&record.worktree_path).exists());
            let saved = thread::load(&fx.project, &record.id).unwrap();
            assert!(saved.worktree_path.is_empty());
            assert!(!saved.cleanup_pending);
            assert!(
                refs(
                    &crate::runner::RealRunner,
                    &record.repo,
                    Some(bare.path().to_str().unwrap())
                )
                .unwrap()
                .is_empty()
            );
            assert!(
                !refs(&crate::runner::RealRunner, &record.repo, None)
                    .unwrap()
                    .contains_key(&record.branch)
            );
            assert!(fx.world.runner.calls.borrow().iter().any(|cmd| {
                cmd.args.starts_with(&["--machine".into(), "box".into()])
                    && cmd.args.iter().any(|arg| arg == "list")
            }));
        }
    }

    #[test]
    fn sealed_box_retained_checkout_refuses_commit_beyond_seal() {
        let (fx, _bare, record, _sha) = sealed_retained_box();
        crate::testkit::commit_file(
            Path::new(&record.worktree_path),
            "later.txt",
            "later\n",
            "later",
        );
        let error = crate::threads::remove_kept_worktree(&fx.world.ctx(), "demo", &record.id)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unpushed commits or has moved since publication"),
            "{error}"
        );
        assert!(Path::new(&record.worktree_path).exists());
        assert!(
            !thread::load(&fx.project, &record.id)
                .unwrap()
                .cleanup_pending
        );
    }

    #[test]
    fn sealed_box_retained_checkout_requires_remote_seal() {
        let (fx, bare, record, sha) = sealed_retained_box();
        run(
            &fx.repo,
            &[
                "push",
                "-q",
                bare.path().to_str().unwrap(),
                &format!(":refs/heads/{}", crate::ops::seal_ref(&record.branch, &sha)),
            ],
        );
        assert!(crate::threads::remove_kept_worktree(&fx.world.ctx(), "demo", &record.id).is_err());
        assert!(Path::new(&record.worktree_path).exists());
    }

    #[test]
    fn closed_project_retained_checkout_without_pane_is_removed() {
        let fx = crate::testkit::fixture();
        let mut record = retained_lane(&fx, "hp/demo/closed");
        record = thread::update(&fx.project, &record.id, |t| {
            t.kind = crate::thread::Kind::Worktree;
            t.tab_id = "w2:t1".into();
            t.pane_id = "w2:p1".into();
            t.workspace_id = "w2".into();
        })
        .unwrap();
        fx.seal_done(
            &record.id,
            1,
            1,
            &run(&fx.repo, &["rev-parse", &record.branch]),
            "report",
        );
        std::fs::remove_file(fx.project.coordinator().unwrap().socket).unwrap();
        crate::threads::remove_kept_worktree(&fx.world.ctx(), "demo", &record.id).unwrap();
        assert!(!Path::new(&record.worktree_path).exists());
        assert!(
            thread::load(&fx.project, &record.id)
                .unwrap()
                .worktree_path
                .is_empty()
        );
        assert!(
            !fx.world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|cmd| cmd.args.iter().any(|arg| arg == "close"))
        );
    }

    #[test]
    fn closed_coordinator_does_not_hide_live_box_agent() {
        let (fx, _bare, record, _sha) = sealed_retained_box();
        *fx.world.agents.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::agent_json(
                &record.workspace_id,
                &record.tab_id,
                &record.pane_id,
                &record.cwd,
                &record.agent_name,
                "working"
            )
        );
        let error = crate::threads::remove_kept_worktree(&fx.world.ctx(), "demo", &record.id)
            .unwrap_err()
            .to_string();
        assert!(error.contains("worktree_in_use"), "{error}");
        assert!(Path::new(&record.worktree_path).exists());
    }

    #[test]
    fn resolved_lane_prunes_both_divergent_seal_refs() {
        let (fx, bare) = configured();
        let repo = fx.repo.to_string_lossy().into_owned();
        let remote = bare.path().to_str().unwrap();
        let branch = "hp/demo/t-1";
        let base = run(&fx.repo, &["rev-parse", "HEAD"]);
        run(&fx.repo, &["branch", branch, &base]);
        run(&fx.repo, &["push", "-q", remote, branch]);
        let record = thread::allocate(&fx.project, |t| {
            t.repo = repo.clone();
            t.branch = branch.into();
            t.status = Status::Resolved;
        })
        .unwrap();
        for (n, label) in [(1, "first"), (2, "second")] {
            run(&fx.repo, &["checkout", "-q", "--detach", &base]);
            run(&fx.repo, &["commit", "--allow-empty", "-qm", label]);
            let sha = run(&fx.repo, &["rev-parse", "HEAD"]);
            let reference = crate::ops::seal_ref(branch, &sha);
            run(
                &fx.repo,
                &[
                    "push",
                    "-q",
                    remote,
                    &format!("{sha}:refs/heads/{reference}"),
                ],
            );
            let id = fx.seal_done(&record.id, 1, n, &sha, label);
            let path = fx
                .project
                .state_dir()
                .join("events")
                .join(format!("{id}.toml"));
            let mut event = crate::events::load(&fx.project, &id).unwrap();
            event.payload.done.as_mut().unwrap().published_ref = Some(reference);
            std::fs::write(path, toml::to_string(&event).unwrap()).unwrap();
        }
        let runner = crate::runner::RealRunner;
        let before = refs(&runner, &repo, Some(remote)).unwrap();
        assert_eq!(before.len(), 3);
        resolved_thread(&fx.world.ctx(), &fx.project, &record).unwrap();
        let after = refs(&runner, &repo, Some(remote)).unwrap();
        assert!(
            !after
                .keys()
                .any(|name| name == branch || name.starts_with("seals/"))
        );
    }

    #[test]
    fn merged_box_cleanup_accepts_review_seal_but_refuses_a_later_local_commit() {
        for (reviewer, moved) in [(false, false), (false, true), (true, false), (true, true)] {
            let (fx, bare) = configured();
            let branch = if reviewer {
                "hp/demo/t-1-review-pile-review-1"
            } else {
                "hp/demo/t-1"
            };
            let remote = bare.path().to_str().unwrap();
            let base = run(&fx.repo, &["rev-parse", "HEAD"]);
            run(&fx.repo, &["branch", branch, &base]);
            run(&fx.repo, &["push", "-q", remote, branch]);
            let seal = run(&fx.repo, &["commit-tree", "HEAD^{tree}", "-m", "sealed"]);
            let reference = crate::ops::seal_ref(branch, &seal);
            run(
                &fx.repo,
                &[
                    "push",
                    "-q",
                    remote,
                    &format!("{seal}:refs/heads/{reference}"),
                ],
            );
            run(
                &fx.repo,
                &["update-ref", &format!("refs/heads/{branch}"), &seal],
            );
            let record = thread::allocate(&fx.project, |t| {
                t.repo = fx.repo.to_string_lossy().into_owned();
                t.branch = branch.into();
                t.base = base.clone();
                t.machine = "buildbox".into();
                if reviewer {
                    t.role = "reviewer".into();
                    t.review_id = "review-1".into();
                } else {
                    t.merged_review = "review-1".into();
                }
                t.status = Status::Resolved;
                t.cleanup_pending = true;
                t.cleanup_reason = "branch moved beyond its sealed cleanup tip".into();
            })
            .unwrap();
            let event_id = fx.seal_done(&record.id, 1, 1, &seal, "done");
            let path = fx
                .project
                .state_dir()
                .join("events")
                .join(format!("{event_id}.toml"));
            let mut event = crate::events::load(&fx.project, &event_id).unwrap();
            event.payload.done.as_mut().unwrap().published_ref = Some(reference.clone());
            std::fs::write(path, toml::to_string(&event).unwrap()).unwrap();
            crate::review::save(
                &fx.project,
                &crate::review::Review {
                    id: "review-1".into(),
                    repo: record.repo.clone(),
                    integration: "main".into(),
                    base: base.clone(),
                    candidate_branch: "review/demo/review-1".into(),
                    members: if reviewer {
                        vec![]
                    } else {
                        vec![crate::review::Member {
                            thread: record.id.clone(),
                            attempt: 1,
                            event: event_id,
                            sha: seal.clone(),
                            branch: branch.into(),
                            artifact: String::new(),
                        }]
                    },
                    gates: vec![],
                    selected_gates: vec![],
                    reviewer: reviewer.then(|| record.id.clone()),
                    phase: crate::review::Phase::Complete,
                    verdict: reviewer.then(|| crate::review::Verdict {
                        verdict: "approve".into(),
                        review: "review-1".into(),
                        candidate: seal.clone(),
                        without: Default::default(),
                        gates: vec![],
                    }),
                    verdict_event: String::new(),
                    reviewer_after: String::new(),
                    checked_event: String::new(),
                    retry_attempt: None,
                    retry_generation: 0,
                    moved: 0,
                    refresh_tip: None,
                    push_remote: None,
                    install_required: false,
                    fast_forward: true,
                    push: true,
                    install: true,
                    close: true,
                    prune: true,
                    attention: String::new(),
                    no_verdict_since: String::new(),
                    notices: vec![],
                },
            )
            .unwrap();
            let (mut settings, body) = fx.project.read_project_md().unwrap();
            settings.repos[0].box_path = Some("/box/repo".into());
            settings.repos[0].publish_url = Some(remote.into());
            std::fs::write(
                fx.project.project_md(),
                format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
            )
            .unwrap();
            let config = fx.world.home.path().join("cfg/config.toml");
            let original = std::fs::read_to_string(&config).unwrap();
            std::fs::write(config, format!("{original}{}", crate::remote::TEST_MACHINE)).unwrap();
            fx.world
                .runner
                .on("machine list --json", crate::runner::fake::ok("[]"));
            fx.world.runner.on_fn(
                |cmd| cmd.program == "ssh",
                |_| Ok(crate::runner::fake::ok("")),
            );
            if moved {
                let beyond = run(
                    &fx.repo,
                    &["commit-tree", "HEAD^{tree}", "-p", &seal, "-m", "beyond"],
                );
                run(
                    &fx.repo,
                    &["update-ref", &format!("refs/heads/{branch}"), &beyond],
                );
                let error = resolved_thread(&fx.world.ctx(), &fx.project, &record).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("moved beyond its sealed cleanup tip"),
                    "{error:#}"
                );
                assert_eq!(
                    refs(fx.world.ctx().runner, &record.repo, None).unwrap()[branch],
                    beyond
                );
                crate::threads::retry_pending_cleanup(&fx.world.ctx(), &fx.project).unwrap();
                assert!(
                    thread::load(&fx.project, &record.id)
                        .unwrap()
                        .cleanup_pending
                );
                assert_eq!(
                    refs(fx.world.ctx().runner, &record.repo, None).unwrap()[branch],
                    beyond
                );
                assert!(
                    refs(fx.world.ctx().runner, &record.repo, Some(remote))
                        .unwrap()
                        .contains_key(&reference)
                );
            } else {
                crate::threads::retry_pending_cleanup(&fx.world.ctx(), &fx.project).unwrap();
                let finished = thread::load(&fx.project, &record.id).unwrap();
                assert!(!finished.cleanup_pending, "{}", finished.cleanup_reason);
                assert!(
                    fx.world
                        .runner
                        .calls
                        .borrow()
                        .iter()
                        .any(|cmd| { cmd.program == "ssh" && cmd.display().contains("rm -rf --") })
                );
                let local = refs(fx.world.ctx().runner, &record.repo, None).unwrap();
                let published = refs(fx.world.ctx().runner, &record.repo, Some(remote)).unwrap();
                assert!(!local.contains_key(branch));
                assert!(!published.contains_key(branch));
                assert!(!published.contains_key(&reference));
            }
        }
    }

    #[test]
    fn one_time_sweep_only_removes_merged_resolved_lanes() {
        let (fx, bare) = configured();
        let repo = fx.repo.to_string_lossy().into_owned();
        let done_path = fx.world.home.path().join("done-worktree");
        for (name, status, merged) in [
            ("hp/demo/done", Status::Resolved, true),
            ("hp/demo/open", Status::Open, true),
            ("hp/demo/unmerged", Status::Resolved, false),
        ] {
            run(&fx.repo, &["branch", name, "main"]);
            if name == "hp/demo/done" {
                run(
                    &fx.repo,
                    &["worktree", "add", "-q", done_path.to_str().unwrap(), name],
                );
            }
            if !merged {
                let extra = fx.world.home.path().join("extra");
                run(
                    &fx.repo,
                    &["worktree", "add", "-q", extra.to_str().unwrap(), name],
                );
                run(&extra, &["commit", "--allow-empty", "-qm", "not merged"]);
            }
            run(
                &fx.repo,
                &["push", "-q", bare.path().to_str().unwrap(), name],
            );
            thread::allocate(&fx.project, |t| {
                t.repo = repo.clone();
                t.branch = name.into();
                t.status = status;
                if name == "hp/demo/done" || !merged {
                    let path = if merged {
                        done_path.clone()
                    } else {
                        fx.world.home.path().join("extra")
                    };
                    t.worktree_path = path.to_string_lossy().into_owned();
                    t.kind = thread::Kind::Worktree;
                }
            })
            .unwrap();
        }
        let ctx = fx.world.ctx();
        sweep_once(&ctx, |_| {}).unwrap();
        assert!(!done_path.exists());
        assert!(fx.world.home.path().join("extra").exists());
        assert!(
            !refs(ctx.runner, &repo, None)
                .unwrap()
                .contains_key("hp/demo/done")
        );
        let published = refs(ctx.runner, &repo, Some(bare.path().to_str().unwrap())).unwrap();
        assert!(!published.contains_key("hp/demo/done"));
        assert!(published.contains_key("hp/demo/open"));
        assert!(published.contains_key("hp/demo/unmerged"));
        assert!(
            refs(ctx.runner, &repo, None)
                .unwrap()
                .contains_key("hp/demo/open")
        );
        assert!(
            refs(ctx.runner, &repo, None)
                .unwrap()
                .contains_key("hp/demo/unmerged")
        );
        sweep_once(&ctx, |_| {}).unwrap();
    }

    #[test]
    fn one_time_sweep_skips_retained_data_and_finishes_cleaning() {
        let (fx, bare) = configured();
        let repo = fx.repo.to_string_lossy().into_owned();
        let remote = bare.path().to_str().unwrap();
        let mut logs = Vec::new();
        for (name, folder) in [
            ("hp/demo/data", "data-worktree"),
            ("hp/demo/clean", "clean-worktree"),
        ] {
            let path = fx.world.home.path().join(folder);
            run(&fx.repo, &["branch", name, "main"]);
            run(
                &fx.repo,
                &["worktree", "add", "-q", path.to_str().unwrap(), name],
            );
            run(&fx.repo, &["push", "-q", remote, name]);
            thread::allocate(&fx.project, |t| {
                t.repo = repo.clone();
                t.branch = name.into();
                t.status = Status::Resolved;
                t.kind = thread::Kind::Worktree;
                t.worktree_path = path.to_string_lossy().into_owned();
            })
            .unwrap();
        }
        std::fs::write(fx.world.home.path().join("data-worktree/kept-data"), "keep").unwrap();
        use std::io::Write;
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(fx.repo.join(".git/info/exclude"))
                .unwrap(),
            "kept-data"
        )
        .unwrap();
        assert_eq!(
            run(
                &fx.world.home.path().join("data-worktree"),
                &["check-ignore", "kept-data"]
            ),
            "kept-data"
        );

        let ctx = fx.world.ctx();
        sweep_once(&ctx, |message| logs.push(message.to_owned())).unwrap();
        assert!(
            fx.world
                .home
                .path()
                .join("data-worktree/kept-data")
                .exists()
        );
        assert!(!fx.world.home.path().join("clean-worktree").exists());
        let local = refs(ctx.runner, &repo, None).unwrap();
        let published = refs(ctx.runner, &repo, Some(remote)).unwrap();
        for branches in [local, published] {
            assert!(branches.contains_key("hp/demo/data"));
            assert!(!branches.contains_key("hp/demo/clean"));
        }
        assert!(ctx.root.join(".branch-sweep-v2.json").exists());
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("keeps ignored data: kept-data"));
        sweep_once(&ctx, |message| logs.push(message.to_owned())).unwrap();
        assert_eq!(logs.len(), 1);
    }

    #[test]
    fn round_sweep_removes_only_merged_local_round_and_clean_worktree() {
        let (fx, bare) = configured();
        let repo = fx.repo.to_str().unwrap();
        let remote = bare.path().to_str().unwrap();
        let path = fx.repo.join(".worktrees/review-r19");
        run(&fx.repo, &["branch", "review/r19", "main"]);
        run(
            &fx.repo,
            &[
                "worktree",
                "add",
                "-q",
                path.to_str().unwrap(),
                "review/r19",
            ],
        );
        run(&fx.repo, &["branch", "review/r20", "main"]);
        let unmerged = fx.world.home.path().join("unmerged");
        run(
            &fx.repo,
            &[
                "worktree",
                "add",
                "-q",
                unmerged.to_str().unwrap(),
                "review/r20",
            ],
        );
        run(&unmerged, &["commit", "--allow-empty", "-qm", "new round"]);
        run(&fx.repo, &["branch", "review/pile/review-1", "main"]);
        for branch in ["review/r19", "review/r20", "review/pile/review-1"] {
            run(&fx.repo, &["push", "-q", remote, branch]);
        }
        let old = fx.world.ctx().root.join(".branch-sweep-v1.json");
        std::fs::write(&old, "true").unwrap();
        let mut logs = Vec::new();
        sweep_once(&fx.world.ctx(), |line| logs.push(line.to_owned())).unwrap();
        assert!(!old.exists());
        assert!(fx.world.ctx().root.join(".branch-sweep-v2.json").exists());
        assert!(!path.exists());
        let local = refs(fx.world.ctx().runner, repo, None).unwrap();
        assert!(!local.contains_key("review/r19"));
        assert!(local.contains_key("review/r20"));
        assert!(local.contains_key("review/pile/review-1"));
        let published = refs(fx.world.ctx().runner, repo, Some(remote)).unwrap();
        assert!(published.contains_key("review/r19"));
        assert!(published.contains_key("review/r20"));
        assert!(published.contains_key("review/pile/review-1"));
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("review/r20 keeps unmerged commits"));
    }

    #[test]
    fn round_sweep_keeps_changed_and_data_worktrees() {
        let (fx, _bare) = configured();
        let repo = fx.repo.to_str().unwrap();
        for (branch, file) in [("review/r21", "change"), ("review/r22-2", "data")] {
            let path = fx
                .repo
                .join(".worktrees")
                .join(branch.replace("review/", "review-"));
            run(&fx.repo, &["branch", branch, "main"]);
            run(
                &fx.repo,
                &["worktree", "add", "-q", path.to_str().unwrap(), branch],
            );
            std::fs::write(path.join(file), "keep").unwrap();
        }
        std::fs::write(fx.repo.join(".git/info/exclude"), "data\n").unwrap();
        let mut logs = Vec::new();
        sweep_once(&fx.world.ctx(), |line| logs.push(line.to_owned())).unwrap();
        let local = refs(fx.world.ctx().runner, repo, None).unwrap();
        assert!(local.contains_key("review/r21"));
        assert!(local.contains_key("review/r22-2"));
        assert_eq!(logs.len(), 2);
        assert!(
            logs.iter()
                .any(|line| line.contains("review/r21 keeps uncommitted changes"))
        );
        assert!(
            logs.iter()
                .any(|line| line.contains("review/r22-2 keeps ignored data: data"))
        );
    }

    #[test]
    fn compare_and_delete_refuses_a_checked_out_branch() {
        let fx = crate::testkit::fixture();
        let branch = "hp/demo/reviewer";
        run(&fx.repo, &["branch", branch, "main"]);
        run(&fx.repo, &["checkout", "-q", branch]);
        let sha = run(&fx.repo, &["rev-parse", branch]);
        let repo = fx.repo.to_str().unwrap();
        let runner = crate::runner::RealRunner;
        let error = delete_local(&runner, repo, branch, &sha).unwrap_err();
        assert!(error.to_string().contains("still checked out"), "{error:#}");
        assert_eq!(refs(&runner, repo, None).unwrap().get(branch), Some(&sha));
    }

    #[test]
    fn concurrent_cleanup_of_same_thread_is_idempotent() {
        let (fx, bare) = configured();
        let name = "hp/demo/t-concurrent";
        run(&fx.repo, &["branch", name, "main"]);
        run(
            &fx.repo,
            &["push", "-q", bare.path().to_str().unwrap(), name],
        );
        let repo = fx.repo.to_string_lossy().into_owned();
        let record = thread::allocate(&fx.project, |t| {
            t.repo = repo.clone();
            t.branch = name.into();
            t.status = Status::Resolved;
        })
        .unwrap();
        let env = crate::paths::Env::for_test(fx.world.home.path(), &[]);
        let base = fx.world.ctx();
        let root = base.root;
        let config_dir = base.config_dir;
        std::thread::scope(|scope| {
            let cleanup = || {
                let runner = crate::runner::RealRunner;
                let ctx = crate::paths::Ctx {
                    env: &env,
                    root: root.clone(),
                    config_dir: config_dir.clone(),
                    runner: &runner,
                    detached_ticker: false,
                };
                resolved_thread(&ctx, &fx.project, &record)
            };
            let a = scope.spawn(cleanup);
            let b = scope.spawn(cleanup);
            a.join().unwrap().unwrap();
            b.join().unwrap().unwrap();
        });
        assert!(
            !refs(fx.world.ctx().runner, &repo, None)
                .unwrap()
                .contains_key(name)
        );
        assert!(
            !refs(
                fx.world.ctx().runner,
                &repo,
                Some(bare.path().to_str().unwrap())
            )
            .unwrap()
            .contains_key(name)
        );
    }

    #[test]
    fn remote_deletion_leases_tip_that_moves_after_check() {
        use crate::runner::fake::{FakeRunner, fail, ok};
        use std::cell::Cell;
        use std::rc::Rc;

        let runner = FakeRunner::new();
        let expected = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let moved = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let tip = Rc::new(Cell::new(expected));
        let advertised = tip.clone();
        runner.on_fn(
            |cmd| cmd.display().contains("ls-remote --heads"),
            move |_| {
                Ok(ok(&format!(
                    "{}\trefs/heads/hp/demo/t-1\n",
                    advertised.get()
                )))
            },
        );
        let pushed = tip.clone();
        runner.on_fn(
            |cmd| cmd.args.iter().any(|arg| arg == "push"),
            move |cmd| {
                // New work arrives after ls-remote but before the server
                // processes the push. Only a lease protects that new tip.
                pushed.set(moved);
                if cmd.args.iter().any(|arg| {
                    arg == &format!("--force-with-lease=refs/heads/hp/demo/t-1:{expected}")
                }) {
                    Ok(fail(1, "stale info"))
                } else {
                    pushed.set("");
                    Ok(ok(""))
                }
            },
        );

        let error = delete_remote(
            &runner,
            "/repo",
            "https://example.com/repo.git",
            "hp/demo/t-1",
            expected,
        )
        .unwrap_err();

        assert!(crate::refusal::is(&error));
        assert!(
            error.to_string().contains(&format!(
                "moved from {expected} to {moved}; not removing it"
            )),
            "{error:#}"
        );
        assert_eq!(tip.get(), moved);
        assert_eq!(runner.count("ls-remote --heads"), 2);
        let calls = runner.calls.borrow();
        let push = calls
            .iter()
            .find(|cmd| cmd.args.iter().any(|arg| arg == "push"))
            .unwrap();
        assert_eq!(
            push.args,
            [
                "-C",
                "/repo",
                "push",
                &format!("--force-with-lease=refs/heads/hp/demo/t-1:{expected}"),
                "https://example.com/repo.git",
                ":refs/heads/hp/demo/t-1",
            ]
        );
    }

    #[test]
    fn checked_cleanup_against_a_fake_remote() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let bare = temp.path().join("remote.git");
        std::fs::create_dir(&repo).unwrap();
        Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&bare)
            .status()
            .unwrap();
        run(&repo, &["init", "-q"]);
        run(&repo, &["config", "user.email", "a@b.c"]);
        run(&repo, &["config", "user.name", "A"]);
        run(&repo, &["commit", "--allow-empty", "-qm", "initial"]);
        run(&repo, &["branch", "hp/demo/t-1"]);
        let path = repo.to_str().unwrap();
        let url = bare.to_str().unwrap();
        let sha = run(&repo, &["rev-parse", "hp/demo/t-1"]);
        run(&repo, &["push", "-q", url, "hp/demo/t-1"]);
        let runner = crate::runner::RealRunner;
        assert_eq!(refs(&runner, path, Some(url)).unwrap()["hp/demo/t-1"], sha);
        run(&repo, &["checkout", "-q", "hp/demo/t-1"]);
        run(&repo, &["commit", "--allow-empty", "-qm", "new work"]);
        run(&repo, &["push", "-q", url, "hp/demo/t-1"]);
        let moved = delete_remote(&runner, path, url, "hp/demo/t-1", &sha).unwrap_err();
        assert!(crate::refusal::is(&moved));
        let moved_sha = run(&repo, &["rev-parse", "HEAD"]);
        run(&repo, &["checkout", "-q", "--detach", &moved_sha]);
        delete_remote(&runner, path, url, "hp/demo/t-1", &moved_sha).unwrap();
        delete_local(&runner, path, "hp/demo/t-1", &moved_sha).unwrap();
        assert!(
            !refs(&runner, path, Some(url))
                .unwrap()
                .contains_key("hp/demo/t-1")
        );
    }
}
