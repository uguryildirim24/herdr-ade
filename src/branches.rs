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
    Ok(output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let sha = fields.next()?;
            let name = fields.next()?.strip_prefix("refs/heads/")?;
            Some((name.to_string(), sha.to_string()))
        })
        .collect())
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

fn delete_local(runner: &dyn Runner, repo: &str, branch: &str, expected: &str) -> Result<()> {
    if !refs(runner, repo, None)?.contains_key(branch) {
        return Ok(());
    }
    if refs(runner, repo, None)?.get(branch).map(String::as_str) != Some(expected) {
        bail!("branch {branch} moved; not removing it");
    }
    // Another cleanup may have removed this owned ref since the snapshot.
    // Only absence is success; a moved ref or any other Git failure remains a refusal.
    if let Err(error) = git(runner, repo, &["branch", "-D", "--", branch]) {
        if error
            .to_string()
            .ends_with(&format!("error: branch '{branch}' not found"))
            && !refs(runner, repo, None)?.contains_key(branch)
        {
            return Ok(());
        }
        return Err(error);
    }
    Ok(())
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
        bail!("published branch {branch} moved; not removing it");
    }
    let lease = format!("--force-with-lease=refs/heads/{branch}:{expected}");
    let deletion = format!(":refs/heads/{branch}");
    git(runner, repo, &["push", &lease, url, &deletion])?;
    Ok(())
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
    let local = refs(ctx.runner, &record.repo, None)?;
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
        let branch = crate::remote::quote(&record.branch);
        let missing = crate::remote::quote(&format!("error: branch '{}' not found", record.branch));
        let script = crate::remote::with_path(
            &machine.path,
            &format!(
                "cd {} || exit $?; git show-ref --verify --quiet {refname}; status=$?; if [ \"$status\" -eq 0 ]; then error=$(git branch -D -- {branch} 2>&1) || {{ git show-ref --verify --quiet {refname}; status=$?; if [ \"$error\" = {missing} ] && [ \"$status\" -eq 1 ]; then :; else printf '%s\\n' \"$error\" >&2; exit 1; fi; }}; elif [ \"$status\" -ne 1 ]; then exit \"$status\"; fi",
                crate::remote::quote(&box_repo),
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
    if let Some(sha) = local.get(&record.branch) {
        delete_local(ctx.runner, &record.repo, &record.branch, sha)?;
    }
    if let Some(url) = url {
        let remote = refs(ctx.runner, &record.repo, Some(&url))?;
        if let Some(sha) = remote.get(&record.branch) {
            delete_remote(ctx.runner, &record.repo, &url, &record.branch, sha)?;
        }
    }
    Ok(())
}

/// Includes superseded review refs whose reviewer is no longer the round's
/// current reviewer. The closed round is the authority for these names.
pub(crate) fn closed_round(
    ctx: &Ctx,
    project: &Project,
    record: &crate::contracts::RoundRecord,
) -> Result<()> {
    if !record.phase.closed() || record.repo.is_empty() {
        return Ok(());
    }
    let (settings, _) = project.read_project_md()?;
    let url = settings
        .repos
        .iter()
        .find(|r| r.path == record.repo)
        .and_then(|r| r.publish_url.as_ref().or(r.push_remote.as_ref()));
    let local = refs(ctx.runner, &record.repo, None)?;
    let remote = match url {
        Some(url) => refs(ctx.runner, &record.repo, Some(url))?,
        None => BTreeMap::new(),
    };
    let root = format!("review/{}", record.round);
    let numbered = format!("{root}-");
    let mut names = BTreeSet::new();
    names.extend(
        local
            .keys()
            .chain(remote.keys())
            .filter(|name| {
                *name == &root
                    || name.strip_prefix(&numbered).is_some_and(|suffix| {
                        suffix
                            .parse::<u32>()
                            .is_ok_and(|number| number >= 2 && number.to_string() == suffix)
                    })
            })
            .cloned(),
    );
    names.extend(record.review_branch.iter().cloned());
    names.extend(record.previous_review_branch.iter().cloned());
    if let Some(batch) = &record.batch {
        names.extend(batch.selection_review_branch.iter().cloned());
        names.extend(batch.review_branch.iter().cloned());
    }
    let in_use = checked_out(ctx.runner, &record.repo)?;
    let threads = checked_threads(project)?;
    let rounds = crate::round::checked_list(project)?;
    for name in names.into_iter().filter(|name| harness_ref(name)) {
        if in_use.contains(&name) {
            bail!("review branch {name} is still checked out; not removing it anywhere");
        }
        // An unfinished reviewer must never lose a branch on a batch retry.
        if threads
            .iter()
            .any(|t| t.repo == record.repo && t.branch == name && t.status != Status::Resolved)
            || rounds.iter().any(|r| {
                r.repo == record.repo
                    && !r.phase.closed()
                    && (r.review_branch.as_deref() == Some(&name)
                        || r.previous_review_branch.as_deref() == Some(&name))
            })
        {
            continue;
        }
        if let Some(sha) = local.get(&name) {
            delete_local(ctx.runner, &record.repo, &name, sha)?;
        }
        if let Some(url) = url
            && let Some(sha) = remote.get(&name)
        {
            delete_remote(ctx.runner, &record.repo, url, &name, sha)?;
        }
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

fn candidates(ctx: &Ctx) -> Result<Vec<Candidate>> {
    let mut result = Vec::new();
    for slug in project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        let (settings, _) = project.read_project_md()?;
        let threads = checked_threads(&project)?;
        let rounds = crate::round::checked_list(&project)?;
        for row in settings.repos {
            if !Path::new(&row.path).is_dir() {
                continue;
            }
            let local = refs(ctx.runner, &row.path, None)?;
            let in_use = checked_out(ctx.runner, &row.path)?;
            let remote_url = row.publish_url.as_ref().or(row.push_remote.as_ref());
            let remote = match remote_url {
                Some(url) => refs(ctx.runner, &row.path, Some(url))?,
                None => BTreeMap::new(),
            };
            let base = row.branch.as_deref().unwrap_or("main");
            let mut names: BTreeSet<_> = local.keys().chain(remote.keys()).cloned().collect();
            names.retain(|name| harness_ref(name) && !in_use.contains(name));
            for name in names {
                let resolved = threads.iter().any(|t| {
                    t.repo == row.path
                        && t.branch == name
                        && t.status == Status::Resolved
                        && !t.cleanup_pending
                });
                let closed_review = rounds.iter().any(|r| {
                    r.repo == row.path
                        && r.phase.closed()
                        && (r.review_branch.as_deref() == Some(&name)
                            || r.previous_review_branch.as_deref() == Some(&name)
                            || r.batch.as_ref().is_some_and(|b| {
                                b.review_branch.as_deref() == Some(&name)
                                    || b.selection_review_branch.as_deref() == Some(&name)
                            }))
                });
                let landed = local
                    .get(&name)
                    .into_iter()
                    .chain(remote.get(&name))
                    .all(|sha| {
                        crate::git::is_ancestor(ctx.runner, &row.path, sha, base).unwrap_or(false)
                    });
                // An open thread still owns its ref, even when it has not
                // diverged from the base yet.
                let active = threads.iter().any(|t| {
                    t.repo == row.path && t.branch == name && t.status != Status::Resolved
                }) || rounds.iter().any(|r| {
                    r.repo == row.path
                        && !r.phase.closed()
                        && (r.review_branch.as_deref() == Some(&name)
                            || r.previous_review_branch.as_deref() == Some(&name)
                            || r.batch.as_ref().is_some_and(|b| {
                                b.review_branch.as_deref() == Some(&name)
                                    || b.selection_review_branch.as_deref() == Some(&name)
                            }))
                });
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
    Ok(result)
}

fn fingerprint(plan: &[Candidate]) -> String {
    let text = format!("{plan:?}");
    format!("{:x}", Sha256::digest(text.as_bytes()))[..16].to_string()
}

/// Doctor only offers an exact, short-lived plan. Re-run doctor if any ref or
/// project status changed since the printed command was built.
pub(crate) fn doctor(ctx: &Ctx, prune: Option<&str>) -> Result<String> {
    let plan = candidates(ctx)?;
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
    if plan.is_empty() {
        return Ok("leftover harness branches: none\n".into());
    }
    let mut text = format!("leftover harness branches ({}):\n", plan.len());
    for item in &plan {
        text.push_str(&format!("  {}: {}\n", item.repo, item.branch));
    }
    text.push_str(&format!(
        "Remove exactly these: ha doctor --prune-branches {id}\n"
    ));
    Ok(text)
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
    fn configured() -> (crate::round::testkit::Fx, tempfile::TempDir) {
        let fx = crate::round::testkit::fixture();
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
    fn a_closed_round_removes_its_current_and_superseded_reviews() {
        let (fx, bare) = configured();
        for name in ["review/r1", "review/r1-2", "review/r1-3"] {
            run(&fx.repo, &["branch", name, "main"]);
            run(
                &fx.repo,
                &["push", "-q", bare.path().to_str().unwrap(), name],
            );
        }
        let round = crate::contracts::RoundRecord {
            phase: crate::contracts::RoundPhase::Merged,
            repo: fx.repo.to_string_lossy().into_owned(),
            round: "r1".into(),
            review_branch: Some("review/r1-3".into()),
            previous_review_branch: Some("review/r1-2".into()),
            ..Default::default()
        };
        closed_round(&fx.world.ctx(), &fx.project, &round).unwrap();
        assert!(
            refs(fx.world.ctx().runner, &round.repo, None)
                .unwrap()
                .keys()
                .all(|n| !n.starts_with("review/"))
        );
        assert!(
            refs(
                fx.world.ctx().runner,
                &round.repo,
                Some(bare.path().to_str().unwrap())
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn a_checked_out_review_is_not_deleted_remotely() {
        let (fx, bare) = configured();
        let url = bare.path().to_str().unwrap();
        run(&fx.repo, &["branch", "review/r1", "main"]);
        run(&fx.repo, &["push", "-q", url, "review/r1"]);
        run(&fx.repo, &["checkout", "-q", "review/r1"]);
        let round = crate::contracts::RoundRecord {
            round: "r1".into(),
            phase: crate::contracts::RoundPhase::Merged,
            repo: fx.repo.to_string_lossy().into_owned(),
            review_branch: Some("review/r1".into()),
            ..Default::default()
        };
        assert!(closed_round(&fx.world.ctx(), &fx.project, &round).is_err());
        assert!(
            refs(fx.world.ctx().runner, &round.repo, Some(url))
                .unwrap()
                .contains_key("review/r1")
        );
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
        assert!(delete_remote(&runner, path, url, "hp/demo/t-1", &sha).is_err());
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
