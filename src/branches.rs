//! Prune harness refs only after their lane is resolved or their commit landed.
//! Remote deletion checks the advertised tip and atomically leases the deletion.
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::paths::Ctx;
use crate::project::Project;
use crate::repo::Git;
use crate::runner::Runner;
use crate::thread::{self, Status, Thread};

const TIMEOUT: Duration = Duration::from_secs(40);

fn refs(runner: &dyn Runner, repo: &str, remote: Option<&str>) -> Result<BTreeMap<String, String>> {
    let git = Git::new(runner, repo).with_timeout(TIMEOUT);
    let output = if let Some(remote) = remote {
        git.run(&["ls-remote", "--heads", remote])?
    } else {
        git.run(&[
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            "refs/heads",
        ])?
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
    let output =
        Git::new(runner, repo)
            .with_timeout(TIMEOUT)
            .run(&["worktree", "list", "--porcelain"])?;
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

fn delete_local(runner: &dyn Runner, repo: &str, branch: &str, expected: &str) -> Result<()> {
    if checked_out(runner, repo)?.contains(branch) {
        bail!("branch {branch} is still checked out; not removing it");
    }
    let name = format!("refs/heads/{branch}");
    // update-ref compares the old value atomically. Unlike branch -D it cannot
    // lose a check/delete race to another cleanup process.
    let out = Git::new(runner, repo).with_timeout(TIMEOUT).output(&[
        "update-ref",
        "-d",
        &name,
        expected,
    ])?;
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
    let out = Git::new(runner, repo)
        .with_timeout(TIMEOUT)
        .output(&["push", &lease, url, &deletion])?;
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
        let latest_seal =
            crate::events::latest_done_event(&events, &record.id, record.attempt.max(1))
                .and_then(|event| event.payload.done.as_ref())
                .filter(|done| done.published_ref.is_some());
        let sealed = latest_seal.is_some_and(|done| {
            let reference = crate::ops::seal_ref(&record.branch, &done.sha);
            done.sha == tip
                && done.published_ref.as_deref() == Some(reference.as_str())
                && remote.get(&reference) == Some(&tip)
        });
        let matches_tip = |actual: &String| {
            actual == &tip || (sealed && !record.base.is_empty() && actual == &record.base)
        };
        if !sealed && (latest_seal.is_some() || remote.get(&record.branch) != Some(&tip))
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
        let git = Git::new(ctx.runner, &record.worktree_path).with_timeout(TIMEOUT);
        let checked_out = git.run(&["symbolic-ref", "--quiet", "HEAD"])?;
        if checked_out.trim() != format!("refs/heads/{}", record.branch) {
            bail!(
                "{} no longer checks out {}; not removing it",
                record.worktree_path,
                record.branch
            );
        }
        let head = git.run(&["rev-parse", "HEAD"])?;
        let tip = Git::new(ctx.runner, &record.repo)
            .with_timeout(TIMEOUT)
            .run(&[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", record.branch),
            ])?;
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
    let last_box_seal =
        crate::events::latest_done_event(&events, &record.id, record.attempt.max(1))
            .and_then(|event| event.payload.done.as_ref())
            .filter(|done| done.published_ref.is_some());
    let has_seal_refs = last_box_seal.is_some();
    let retained_tip = record
        .retirement
        .as_ref()
        .filter(|request| request.authority == crate::thread::RetirementAuthority::Retained)
        .map(|request| request.retained_tip.as_str())
        .filter(|tip| !tip.is_empty());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Cmd;
    use std::path::Path;
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
    fn timed_out_deletion_is_not_success_even_if_a_retry_could_find_absence() {
        let runner = crate::runner::fake::FakeRunner::new();
        runner.on("update-ref", crate::runner::fake::timeout());
        runner.on("worktree list", crate::runner::fake::ok(""));
        runner.on("for-each-ref", crate::runner::fake::ok(""));
        assert!(delete_local(&runner, "/repo", "hp/demo/t-1", "abc").is_err());
        assert_eq!(runner.count("for-each-ref"), 0);
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
        record.retirement = Some(crate::thread::RetirementRequest {
            authority: crate::thread::RetirementAuthority::Retained,
            retained_tip: tip,
            ..Default::default()
        });
        resolved_thread(&fx.world.ctx(), &fx.project, &record).unwrap();
        assert!(
            !refs(&crate::runner::RealRunner, &record.repo, None)
                .unwrap()
                .contains_key(&record.branch)
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
        assert!(require_published_tip(&fx.world.ctx(), &fx.project, &record).is_err());
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
        assert!(require_published_tip(&fx.world.ctx(), &fx.project, &record).is_err());
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
        let root = fx.world.home.path().to_path_buf();
        let env = crate::paths::Env::for_test(&root, &[]);
        fx.world.runner.on_fn(
            |cmd| cmd.program == "ssh",
            move |cmd| {
                use crate::runner::Runner;
                if cmd.display().contains("HERDR_ADE_BOX_INPUT") {
                    let ctx = crate::paths::Ctx {
                        env: &env,
                        root: root.clone(),
                        config_dir: root.join("cfg"),
                        runner: &crate::runner::RealRunner,
                        detached_ticker: false,
                    };
                    return crate::box_helper::tests::respond(&ctx, cmd.stdin.as_deref().unwrap());
                }
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
            // Ref retirement is not the last owed effect. A failed scratch
            // teardown must keep the typed pin and obligation for the ticker.
            struct ScratchFailure<'a>(&'a dyn crate::runner::Runner, std::cell::Cell<bool>);
            impl crate::runner::Runner for ScratchFailure<'_> {
                fn run(&self, cmd: &Cmd) -> Result<crate::runner::Output> {
                    if self.1.get()
                        && cmd.program == "ssh"
                        && cmd
                            .args
                            .last()
                            .is_some_and(|script| script.contains("state=$(herdr session list"))
                    {
                        return Ok(crate::runner::fake::fail(1, "busy"));
                    }
                    self.0.run(cmd)
                }
            }
            let runner = ScratchFailure(&fx.world.runner, std::cell::Cell::new(true));
            let mut ctx = fx.world.ctx();
            ctx.runner = &runner;
            let error = crate::threads::remove_kept_worktree(&ctx, "demo", &record.id).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("could not delete scratch session")
            );
            assert!(!Path::new(&record.worktree_path).exists());
            let pending = thread::load(&fx.project, &record.id).unwrap();
            assert!(pending.cleanup_pending);
            assert_eq!(pending.retirement.unwrap().retained_tip, sha);
            runner.1.set(false);
            crate::threads::retry_pending_cleanup(&ctx, &fx.project).unwrap();
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
    fn retained_box_worktree_with_a_linked_folder_can_be_removed() {
        let (fx, _bare, record, _sha) = sealed_retained_box();
        let folder =
            Path::new(&record.worktree_path).join(format!(".herdr-project/demo-{}", record.id));
        std::fs::create_dir_all(folder.join("library/nested")).unwrap();
        std::fs::write(folder.join("library/before.png"), b"before").unwrap();
        std::fs::write(folder.join("library/nested/after.png"), b"after").unwrap();
        let report = "Screenshots: [library](library/)\n";
        std::fs::write(folder.join("report.md"), report).unwrap();
        let exclude = run(
            Path::new(&record.worktree_path),
            &["rev-parse", "--git-path", "info/exclude"],
        );
        std::fs::write(exclude.trim(), ".herdr-project/\n").unwrap();
        thread::update(&fx.project, &record.id, |t| {
            t.thread_dir = folder.to_string_lossy().into_owned();
        })
        .unwrap();
        // Set up the immutable done with its linked report, retaining the
        // published seal ref supplied by the fixture.
        let mut event = crate::events::list(&fx.project)
            .into_iter()
            .find(|event| event.thread == record.id && event.payload.done.is_some())
            .unwrap();
        event.payload.done.as_mut().unwrap().artifact =
            thread::store_artifact(&fx.project, report.as_bytes()).unwrap();
        std::fs::write(
            fx.project
                .state_dir()
                .join("events")
                .join(format!("{}.toml", event.id)),
            toml::to_string(&event).unwrap(),
        )
        .unwrap();

        crate::threads::remove_kept_worktree(&fx.world.ctx(), "demo", &record.id).unwrap();
        assert!(!Path::new(&record.worktree_path).exists());
        let saved = thread::load(&fx.project, &record.id).unwrap();
        assert!(saved.worktree_path.is_empty());
        assert!(!saved.final_report_hash.is_empty());
        for bytes in [b"before".as_slice(), b"after".as_slice()] {
            let hash = thread::sha256_hex(bytes);
            assert_eq!(
                std::fs::read(crate::events::artifact_path(&fx.project, &hash)).unwrap(),
                bytes
            );
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
        assert!(crate::threads::remove_kept_worktree(&fx.world.ctx(), "demo", &record.id).is_err());
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
                    gates_note: String::new(),
                    selected_gates: vec![],
                    reviewer: reviewer.then(|| record.id.clone()),
                    phase: crate::review::Phase::Complete,
                    verdict: reviewer.then(|| crate::review::Verdict {
                        verdict: "approve".into(),
                        evidence_only: false,
                        withdrawn_only: false,
                        review: "review-1".into(),
                        candidate: seal.clone(),
                        without: Default::default(),
                        gates: vec![],
                        gates_note: String::new(),
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
                    merged_at: String::new(),
                    installed_at: String::new(),
                    push: true,
                    install: true,
                    install_result: String::new(),
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
                assert!(resolved_thread(&fx.world.ctx(), &fx.project, &record).is_err());
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
    fn compare_and_delete_refuses_a_checked_out_branch() {
        let fx = crate::testkit::fixture();
        let branch = "hp/demo/reviewer";
        run(&fx.repo, &["branch", branch, "main"]);
        run(&fx.repo, &["checkout", "-q", branch]);
        let sha = run(&fx.repo, &["rev-parse", branch]);
        let repo = fx.repo.to_str().unwrap();
        let runner = crate::runner::RealRunner;
        assert!(delete_local(&runner, repo, branch, &sha).is_err());
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
