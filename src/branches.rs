//! Prune harness refs only after their lane is resolved or their commit landed.
//! Remote deletion uses a lease, so a new push cannot be erased by a stale plan.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

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
        Some(actual) if actual != expected => Err(crate::refusal::error(format!(
            "branch {branch} moved; not removing it"
        ))),
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
        return Err(crate::refusal::error(format!(
            "published branch {branch} moved from {expected} to {}; not removing it",
            current[branch]
        )));
    }
    let lease = format!("--force-with-lease=refs/heads/{branch}:{expected}");
    let deletion = format!(":refs/heads/{branch}");
    let out = deletion_command(runner, repo, &["push", &lease, url, &deletion])?;
    if out.success() {
        return Ok(());
    }
    match refs(runner, repo, Some(url))?.get(branch) {
        None => Ok(()),
        Some(actual) if actual != expected => Err(crate::refusal::error(format!(
            "published branch {branch} moved from {expected} to {actual}; not removing it"
        ))),
        _ => {
            bail!(
                "git push {lease} {url} {deletion} in {repo}: {}",
                out.error_text()
            );
        }
    }
}

/// Refuse an explicit retained-worktree removal if its checkout contains
/// commits not at the published branch tip. No implicit force-push or loss.
pub(crate) fn require_published_tip(
    ctx: &Ctx,
    project: &Project,
    record: &Thread,
) -> Result<String> {
    if record.branch.is_empty() || !harness_ref(&record.branch) {
        bail!("{} has no owned branch", record.id);
    }
    let (settings, _) = project.read_project_md()?;
    let (tip, url) = if record.is_remote() {
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
        (tip.to_string(), Some(url))
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
        let url = settings
            .repos
            .iter()
            .find(|row| row.path == record.repo)
            .and_then(|row| row.publish_url.clone().or_else(|| row.push_remote.clone()));
        (tip.trim().to_string(), url)
    };
    let Some(url) = url else {
        bail!(
            "no publication destination for {}; cannot prove commits are pushed",
            record.branch
        );
    };
    if refs(ctx.runner, &record.repo, Some(&url))?.get(&record.branch) != Some(&tip)
        || refs(ctx.runner, &record.repo, None)?
            .get(&record.branch)
            .is_some_and(|local| local != &tip)
    {
        bail!(
            "branch {} has unpushed commits or has moved since publication",
            record.branch
        );
    }
    Ok(tip)
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
    let expected = record
        .cleanup_reason
        .strip_prefix("retained worktree removal: ")
        .or(review_pin.as_deref());
    let local = refs(ctx.runner, &record.repo, None)?;
    if let (Some(expected), Some(actual)) = (expected, local.get(&record.branch))
        && actual != expected
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
        let pin = expected.map_or(String::new(), |sha| {
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
    {
        bail!(
            "published branch {} moved beyond its sealed cleanup tip",
            record.branch
        );
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
    let mut merged_by_repo = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for slug in project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        let (settings, _) = project.read_project_md()?;
        let threads = checked_threads(&project)?;
        let reviews = crate::review::list(&project)?;
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
            names.retain(|name| harness_ref(name) && !in_use.contains(name));
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
                let closed_review = reviews
                    .iter()
                    .any(|r| r.repo == row.path && r.phase.closed() && r.candidate_branch == name);
                let landed = local
                    .get(&name)
                    .into_iter()
                    .chain(remote.get(&name))
                    .all(|sha| merged.contains(sha));
                // An open thread still owns its ref, even when it has not
                // diverged from the base yet.
                let active = threads.iter().any(|t| {
                    t.repo == row.path && t.branch == name && t.status != Status::Resolved
                }) || reviews
                    .iter()
                    .any(|r| r.repo == row.path && !r.phase.closed() && r.candidate_branch == name);
                if active || !(resolved || closed_review || landed) {
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

fn fingerprint(plan: &[Candidate]) -> String {
    let text = format!("{plan:?}");
    format!("{:x}", Sha256::digest(text.as_bytes()))[..16].to_string()
}

/// Doctor only offers an exact, short-lived plan. Re-run doctor if any ref or
/// project status changed since the printed command was built.
pub(crate) fn doctor(ctx: &Ctx, prune: Option<&str>) -> Result<String> {
    let (plan, unreachable) = candidates(ctx, prune.is_none())?;
    let id = fingerprint(&plan);
    if let Some(requested) = prune {
        if requested != id || plan.is_empty() {
            bail!("branch plan changed; run `ha doctor` again");
        }
        for item in &plan {
            if let Some(sha) = &item.local {
                delete_local(ctx.runner, &item.repo, &item.branch, sha)?;
            }
            if let Some((url, sha)) = &item.remote {
                delete_remote(ctx.runner, &item.repo, url, &item.branch, sha)?;
            }
        }
        return Ok(format!(
            "removed {} leftover harness branches\n",
            plan.len()
        ));
    }
    let mut text = String::new();
    for warning in &unreachable {
        text.push_str(&format!("[warn] branch inventory: {warning}\n"));
    }
    if plan.is_empty() {
        text.push_str(if unreachable.is_empty() {
            "leftover harness branches: none\n"
        } else {
            "leftover harness branches: remote inventory incomplete\n"
        });
        return Ok(text);
    }
    text.push_str(&format!("leftover harness branches ({}):\n", plan.len()));
    for item in &plan {
        text.push_str(&format!("  {}: {}\n", item.repo, item.branch));
    }
    if unreachable.is_empty() {
        text.push_str(&format!(
            "Remove exactly these: ha doctor --prune-branches {id}\n"
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::process::Command;

    struct CountingRunner(Cell<usize>);
    impl Runner for CountingRunner {
        fn run(&self, cmd: &Cmd) -> Result<crate::runner::Output> {
            if cmd.program == "git" && cmd.args.iter().any(|arg| arg == "rev-list") {
                self.0.set(self.0.get() + 1);
            }
            crate::runner::RealRunner.run(cmd)
        }
    }
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
    fn local_refs_and_checked_out_branches_share_a_parallel_batch() {
        struct Batches(std::cell::RefCell<Vec<usize>>);
        impl Runner for Batches {
            fn run(&self, cmd: &Cmd) -> Result<crate::runner::Output> {
                crate::runner::RealRunner.run(cmd)
            }
            fn run_parallel(&self, commands: &[Cmd]) -> Vec<Result<crate::runner::Output>> {
                self.0.borrow_mut().push(commands.len());
                commands.iter().map(|cmd| self.run(cmd)).collect()
            }
        }
        let (fx, _bare) = configured();
        let batches = Batches(std::cell::RefCell::new(Vec::new()));
        let ctx = fx.world.ctx();
        let ctx = Ctx {
            runner: &batches,
            ..ctx
        };
        doctor(&ctx, None).unwrap();
        assert_eq!(*batches.0.borrow(), vec![1, 2]);
    }

    #[test]
    fn doctor_bounds_hung_remote_and_keeps_local_inventory() {
        struct HungRemote;
        impl Runner for HungRemote {
            fn run(&self, cmd: &Cmd) -> Result<crate::runner::Output> {
                if cmd.args.iter().any(|arg| arg == "ls-remote") {
                    assert!(cmd.own_group);
                    return crate::runner::RealRunner.run(
                        &Cmd::new("sh", Duration::from_millis(250))
                            .args(["-c", "sleep 30 & exit 0"])
                            .own_group(),
                    );
                }
                crate::runner::RealRunner.run(cmd)
            }
        }
        let (fx, _bare) = configured();
        let ctx = fx.world.ctx();
        let ctx = Ctx {
            runner: &HungRemote,
            ..ctx
        };
        let start = std::time::Instant::now();
        let result = doctor(&ctx, None).unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(
            result.contains("remote unreachable (timed out)"),
            "{result}"
        );
        assert!(result.contains("remote inventory incomplete"), "{result}");
        assert!(!result.contains("--prune-branches"));
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

    #[test]
    fn resolved_lane_and_doctor_prune_only_eligible_refs() {
        let (fx, bare) = configured();
        for name in ["hp/demo/t-1", "hp/demo/t-2"] {
            run(&fx.repo, &["branch", name, "main"]);
            run(
                &fx.repo,
                &["push", "-q", bare.path().to_str().unwrap(), name],
            );
        }
        let repo = fx.repo.to_string_lossy().into_owned();
        let done = thread::allocate(&fx.project, |t| {
            t.repo = repo.clone();
            t.branch = "hp/demo/t-1".into();
            t.status = Status::Resolved;
        })
        .unwrap();
        let _active = thread::allocate(&fx.project, |t| {
            t.repo = repo.clone();
            t.branch = "hp/demo/t-2".into();
            t.status = Status::Open;
        })
        .unwrap();
        let ctx = fx.world.ctx();
        let plan = doctor(&ctx, None).unwrap();
        assert!(plan.contains("hp/demo/t-1"));
        assert!(!plan.contains("hp/demo/t-2"));
        let key = plan.split("--prune-branches ").nth(1).unwrap().trim();
        assert!(doctor(&ctx, Some("wrong")).is_err());
        doctor(&ctx, Some(key)).unwrap();
        assert!(
            !refs(ctx.runner, &repo, Some(bare.path().to_str().unwrap()))
                .unwrap()
                .contains_key("hp/demo/t-1")
        );
        assert!(
            refs(ctx.runner, &repo, Some(bare.path().to_str().unwrap()))
                .unwrap()
                .contains_key("hp/demo/t-2")
        );
        // The normal resolve path uses the same deletion against the fake remote.
        thread::update(&fx.project, &done.id, |t| t.branch = "hp/demo/t-2".into()).unwrap();
        let updated = thread::load(&fx.project, &done.id).unwrap();
        assert!(resolved_thread(&ctx, &fx.project, &updated).is_err());
        assert!(
            refs(ctx.runner, &repo, Some(bare.path().to_str().unwrap()))
                .unwrap()
                .contains_key("hp/demo/t-2")
        );
        thread::update(&fx.project, &_active.id, |t| t.status = Status::Resolved).unwrap();
        resolved_thread(&ctx, &fx.project, &updated).unwrap();
        assert!(
            !refs(ctx.runner, &repo, Some(bare.path().to_str().unwrap()))
                .unwrap()
                .contains_key("hp/demo/t-2")
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
    fn inventory_checks_base_history_once_for_many_branches() {
        let (fx, _bare) = configured();
        for index in 0..30 {
            run(&fx.repo, &["branch", &format!("hp/demo/t-{index}"), "main"]);
        }
        let runner = CountingRunner(Cell::new(0));
        let original = fx.world.ctx();
        let ctx = Ctx {
            runner: &runner,
            ..original
        };
        let plan = doctor(&ctx, None).unwrap();
        assert!(plan.contains("hp/demo/t-29"));
        assert_eq!(runner.0.get(), 1);
    }

    #[test]
    fn remote_tip_behind_base_with_no_local_ref_is_still_landed() {
        let (fx, bare) = configured();
        let old = run(&fx.repo, &["rev-parse", "main"]);
        run(
            &fx.repo,
            &[
                "push",
                "-q",
                bare.path().to_str().unwrap(),
                &format!("{old}:refs/heads/hp/demo/t-old"),
            ],
        );
        run(
            &fx.repo,
            &["commit", "--allow-empty", "-qm", "advance main"],
        );
        assert!(
            !refs(fx.world.ctx().runner, fx.repo.to_str().unwrap(), None)
                .unwrap()
                .contains_key("hp/demo/t-old")
        );
        assert!(
            doctor(&fx.world.ctx(), None)
                .unwrap()
                .contains("hp/demo/t-old")
        );
    }

    #[test]
    fn leased_cleanup_against_a_fake_remote() {
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
        run(
            &repo,
            &[
                "push",
                "-q",
                "--force",
                url,
                &format!("{sha}:refs/heads/hp/demo/t-1"),
            ],
        );
        run(&repo, &["checkout", "-q", "--detach", &sha]);
        run(&repo, &["branch", "-f", "hp/demo/t-1", &sha]);
        delete_remote(&runner, path, url, "hp/demo/t-1", &sha).unwrap();
        delete_local(&runner, path, "hp/demo/t-1", &sha).unwrap();
        assert!(
            !refs(&runner, path, Some(url))
                .unwrap()
                .contains_key("hp/demo/t-1")
        );
    }
}
