//! pause, resume, archive, unarchive and delete.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::coordinator;
use crate::herdr::Herdr;
use crate::paths::Ctx;
use crate::project::{Project, Repo, Status};
use crate::remote;
use crate::runner::Cmd;
use crate::thread;
use crate::threads::{self, SessionView};

/// (what, pane id) of every recorded pane that is alive in the project's session.
fn alive_panes(project: &Project, view: &SessionView) -> Vec<(String, String, String)> {
    let mut alive = Vec::new();
    if let Some(record) = project.coordinator() {
        let agent = view
            .agents
            .iter()
            .find(|a| coordinator::agent_matches(&record, a));
        if agent.is_some()
            || view
                .panes
                .iter()
                .any(|p| coordinator::pane_matches(&record, p))
        {
            alive.push((
                "coordinator".to_string(),
                record.pane_id.clone(),
                agent.map(|a| a.agent_status.clone()).unwrap_or_default(),
            ));
        }
    }
    let now = jiff::Timestamp::now();
    for t in thread::list(project) {
        if t.status == thread::Status::Resolved || t.is_remote() {
            continue;
        }
        let live = thread::live_state(&t, &view.agents, &view.panes, now);
        if live.pane_exists {
            alive.push((
                t.id.clone(),
                t.pane_id.clone(),
                live.agent_state.unwrap_or_default(),
            ));
        }
    }
    alive
}

pub(crate) fn set_status(ctx: &Ctx, slug: &str, status: Status) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let current = project.status();
    match (current, status) {
        (Status::Archived, Status::Paused) => bail!("`{slug}` is archived; `unarchive` it first"),
        (Status::Archived, Status::Active)
        | (_, Status::Archived)
        | (_, Status::Paused)
        | (Status::Paused, Status::Active)
        | (Status::Active, Status::Active) => {}
    }
    project.set_status(status)?;
    println!("`{slug}` is now {status}");

    let view = threads::session_view(ctx, &project);
    match status {
        Status::Paused => {
            println!(
                "The ticker skips it and `thread start` is refused. Running agents are not interrupted."
            );
            if let Some(view) = &view {
                for (what, pane, _) in alive_panes(&project, view)
                    .into_iter()
                    .filter(|(_, _, s)| s == "working")
                {
                    println!("  still working: {what} (pane {pane})");
                }
            }
        }
        Status::Archived => {
            println!(
                "It is hidden from `list` and `overview`, the ticker skips it, and `open` is refused until `unarchive`."
            );
            if let Some(view) = &view {
                for (_, pane, _) in alive_panes(&project, view) {
                    let _ = view
                        .herdr
                        .pane_clear_tokens(&pane, &["project", "thread", "review", "rank"]);
                }
            }
        }
        Status::Active => {}
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct DeleteIntent {
    slug: String,
    github: bool,
    created: String,
    repos: Vec<String>,
    worktrees: Vec<String>,
    #[serde(default)]
    completed: BTreeSet<String>,
}

struct OtherProjectOwnership {
    slug: String,
    repos: Vec<Repo>,
    threads: Vec<thread::Thread>,
    paths: BTreeSet<PathBuf>,
}

fn same_path(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let a = Path::new(a);
    let b = Path::new(b);
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn paths_overlap(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let a = std::fs::canonicalize(a).unwrap_or_else(|_| PathBuf::from(a));
    let b = std::fs::canonicalize(b).unwrap_or_else(|_| PathBuf::from(b));
    a.starts_with(&b) || b.starts_with(&a)
}

fn readable_threads(project: &Project) -> Result<Vec<thread::Thread>> {
    let (threads, errors) = thread::list_with_errors(project);
    if errors.is_empty() {
        Ok(threads)
    } else {
        bail!(
            "cannot prove ownership while {} has unreadable thread records: {}",
            project.slug,
            errors
                .iter()
                .map(|error| format!("{error:#}"))
                .collect::<Vec<_>>()
                .join("; ")
        )
    }
}

fn owned_paths(project: &Project, threads: &[thread::Thread]) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::from([project.dir()]);
    paths.extend(
        threads
            .iter()
            .flat_map(|record| [&record.worktree_path, &record.cwd])
            .filter(|path| !path.is_empty())
            .map(PathBuf::from),
    );
    paths
}

fn other_ownership(ctx: &Ctx, slug: &str) -> Result<Vec<OtherProjectOwnership>> {
    let mut owners = Vec::new();
    for other_slug in crate::project::list_slugs(&ctx.root) {
        if other_slug == slug {
            continue;
        }
        let project = Project::load(&ctx.root, &other_slug)?;
        let (settings, _) = project.read_project_md()?;
        let threads = readable_threads(&project)?;
        owners.push(OtherProjectOwnership {
            slug: other_slug,
            repos: settings.repos,
            paths: owned_paths(&project, &threads),
            threads,
        });
    }
    Ok(owners)
}

fn other_projects_using_repo(others: &[OtherProjectOwnership], repo: &Repo) -> Vec<String> {
    others
        .iter()
        .filter(|owner| {
            owner.repos.iter().any(|candidate| {
                paths_overlap(&candidate.path, &repo.path)
                    || candidate
                        .box_path
                        .as_deref()
                        .zip(repo.box_path.as_deref())
                        .is_some_and(|(a, b)| paths_overlap(a, b))
            }) || owner.threads.iter().any(|record| {
                paths_overlap(&record.repo, &repo.path)
                    || repo.box_path.as_deref().is_some_and(|box_path| {
                        record.is_remote() && paths_overlap(&record.worktree_path, box_path)
                    })
            })
        })
        .map(|owner| owner.slug.clone())
        .collect()
}

fn other_projects_using_path(others: &[OtherProjectOwnership], path: &Path) -> Vec<String> {
    let path = path.to_string_lossy();
    others
        .iter()
        .filter(|owner| {
            owner
                .repos
                .iter()
                .any(|repo| paths_overlap(&repo.path, &path))
                || owner
                    .paths
                    .iter()
                    .any(|candidate| paths_overlap(&candidate.to_string_lossy(), &path))
        })
        .map(|owner| owner.slug.clone())
        .collect()
}

fn trash(ctx: &Ctx, path: &Path, what: &str) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let out = ctx
        .runner
        .run(&Cmd::new("/usr/bin/trash", Duration::from_secs(60)).arg(path.to_string_lossy()))?;
    if !out.success() {
        bail!(
            "could not move {} to the macOS Trash: {}",
            path.display(),
            out.error_text()
        );
    }
    println!("removed {what}: {}", path.display());
    Ok(())
}

fn old_copy_for_slug(name: &str, slug: &str) -> bool {
    let Some(stamp) = name
        .strip_prefix(slug)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    let bytes = stamp.as_bytes();
    bytes.len() == 16
        && bytes[8] == b'T'
        && bytes[15] == b'Z'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 15) || byte.is_ascii_digit())
}

fn clean_old_trash(ctx: &Ctx, slug: &str) -> Result<()> {
    let holding = ctx.root.join(".trash");
    let entries = match std::fs::read_dir(&holding) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut entries: Vec<_> = entries.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut kept = false;
    for entry in entries {
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| old_copy_for_slug(name, slug))
        {
            trash(ctx, &entry.path(), "old copy of this project")?;
        } else {
            kept = true;
            println!(
                "kept another project's old copy: {}",
                entry.path().display()
            );
        }
    }
    if kept {
        println!(
            "kept old trash holding folder because it still contains other project copies: {}",
            holding.display()
        );
    } else {
        trash(ctx, &holding, "empty old trash holding folder")?;
    }
    Ok(())
}

fn github_name(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(rest);
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn checkout_github_name(ctx: &Ctx, path: &str) -> Result<String> {
    let out = ctx.runner.run(
        &Cmd::new("git", Duration::from_secs(40)).args(["-C", path, "remote", "get-url", "origin"]),
    )?;
    if !out.success() {
        bail!("could not resolve origin: {}", out.error_text());
    }
    github_name(&out.stdout)
        .ok_or_else(|| anyhow::anyhow!("origin is not a GitHub repo: {}", out.stdout.trim()))
}

fn github_names(repos: &[Repo], threads: &[thread::Thread]) -> BTreeSet<String> {
    repos
        .iter()
        .filter_map(|repo| repo.publish_url.as_deref().and_then(github_name))
        .chain(
            threads
                .iter()
                .filter(|record| repos.iter().any(|repo| same_path(&record.repo, &repo.path)))
                .filter_map(|record| github_name(&record.origin)),
        )
        .collect()
}

fn machines_for_repo(
    ctx: &Ctx,
    repo: &Repo,
    threads: &[thread::Thread],
) -> Result<BTreeSet<String>> {
    let mut machines = BTreeSet::new();
    if let Some(machine) = &repo.machine {
        machines.insert(machine.clone());
    }
    for record in threads
        .iter()
        .filter(|record| record.is_remote() && same_path(&record.repo, &repo.path))
    {
        machines.insert(record.machine_route().to_string());
    }
    for (id, machine) in remote::machine_declarations(&ctx.config_dir)? {
        if machine.repos.iter().any(|candidate| {
            same_path(&candidate.path, &repo.path)
                && candidate.box_path.as_deref() == repo.box_path.as_deref()
        }) {
            machines.insert(id);
        }
    }
    if machines.is_empty() && repo.box_path.is_some() {
        let config = ctx.config_dir.join("config.toml");
        if let Ok(text) = std::fs::read_to_string(config)
            && let Ok(table) = text.parse::<toml::Table>()
            && let Some(machine) = table
                .get("dispatch")
                .and_then(toml::Value::as_table)
                .and_then(|dispatch| dispatch.get("machine"))
                .and_then(toml::Value::as_str)
            && !machine.is_empty()
            && machine != "local"
        {
            machines.insert(machine.to_string());
        }
    }
    Ok(machines)
}

fn prune_local_worktrees(ctx: &Ctx, repo: &str) -> Result<()> {
    if !Path::new(repo).is_dir() {
        return Ok(());
    }
    let out = ctx
        .runner
        .run(&Cmd::new("git", Duration::from_secs(30)).args(["-C", repo, "worktree", "prune"]))?;
    if !out.success() {
        bail!(
            "could not reconcile worktrees in {repo}: {}",
            out.error_text()
        );
    }
    println!("reconciled worktrees in kept repo: {repo}");
    Ok(())
}

fn remote_remove(ctx: &Ctx, machine: &str, path: &str, what: &str) -> Result<()> {
    if !Path::new(path).is_absolute() || path == "/" {
        bail!("refusing to remove unsafe box path `{path}`");
    }
    let profile =
        remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, machine)?;
    let script = format!("rm -rf -- {}", remote::quote(path));
    let out = remote::ssh(
        ctx.runner,
        &profile.target,
        &script,
        None,
        Duration::from_secs(60),
    )?;
    if !out.success() {
        bail!("could not remove {path} on {machine}: {}", out.error_text());
    }
    println!("removed {what} on {machine}: {path}");
    Ok(())
}

fn prune_remote_worktrees(ctx: &Ctx, machine: &str, repo: &str) -> Result<()> {
    if !Path::new(repo).is_absolute() || repo == "/" {
        bail!("refusing unsafe box repo path `{repo}`");
    }
    let profile =
        remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, machine)?;
    let script = format!("git -C {} worktree prune", remote::quote(repo));
    let out = remote::ssh(
        ctx.runner,
        &profile.target,
        &script,
        None,
        Duration::from_secs(30),
    )?;
    if !out.success() {
        bail!(
            "could not reconcile worktrees in {repo} on {machine}: {}",
            out.error_text()
        );
    }
    println!("reconciled worktrees in kept repo on {machine}: {repo}");
    Ok(())
}

fn pi_session_name(path: &Path) -> String {
    format!(
        "--{}--",
        path.to_string_lossy().trim_matches('/').replace('/', "-")
    )
}

fn claude_session_name(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn close_workspace(herdr: &Herdr<'_>, workspace: &str, label: &str) -> Result<()> {
    if workspace.is_empty() {
        return Ok(());
    }
    match herdr.workspace_close(workspace) {
        Ok(()) => println!("closed workspace {workspace}{label}"),
        Err(error) if error.code == "workspace_not_found" => {}
        Err(error) => return Err(anyhow::anyhow!("{error}")),
    }
    Ok(())
}

fn close_tab(herdr: &Herdr<'_>, tab: &str, label: &str) -> Result<()> {
    if tab.is_empty() {
        return Ok(());
    }
    match herdr.tab_close(tab) {
        Ok(()) => println!("closed tab {tab}{label}"),
        Err(error) if error.code == "tab_not_found" => {}
        Err(error) => return Err(anyhow::anyhow!("{error}")),
    }
    Ok(())
}

/// Permanently removes a project and everything attributable only to it.
/// Local files go through macOS Trash; `archive` is the reversible operation.
pub(crate) fn delete(ctx: &Ctx, slug: &str, delete_github: bool, preview: bool) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let (settings, _) = project.read_project_md()?;
    let threads = readable_threads(&project)?;
    let others = other_ownership(ctx, slug)?;
    let coordinator = project.coordinator();

    let record_users = other_projects_using_path(&others, &project.dir());
    if !record_users.is_empty() {
        bail!(
            "cannot delete `{slug}` because its project record overlaps resources owned by {}",
            record_users.join(", ")
        );
    }

    let project_paths: Vec<_> = owned_paths(&project, &threads).into_iter().collect();

    if preview {
        println!("Archive keeps `{slug}` and its files available for unarchive.");
        println!("Delete removes these explicitly recorded resources:");
        println!("  project record: {}", project.dir().display());
        for repo in &settings.repos {
            let shared = other_projects_using_repo(&others, repo);
            if shared.is_empty() {
                println!("  owned repo: {}", repo.path);
                if let Some(box_path) = &repo.box_path {
                    println!("  owned box repo: {box_path}");
                }
            } else {
                println!("  kept shared repo: {} ({})", repo.path, shared.join(", "));
            }
        }
        for lane in &threads {
            println!("  lane: {}", lane.id);
        }
        println!(
            "  GitHub: {}",
            if delete_github {
                "delete project-owned repositories"
            } else {
                "keep repositories"
            }
        );
        return Ok(());
    }

    let intent_path = project.state_dir().join("delete.toml");
    let mut delete_intent = if intent_path.is_file() {
        let text = std::fs::read_to_string(&intent_path)?;
        let intent: DeleteIntent = toml::from_str(&text)?;
        if intent.github != delete_github {
            bail!(
                "delete is already in progress with GitHub deletion {}; retry with the same choice",
                if intent.github { "on" } else { "off" }
            );
        }
        intent
    } else {
        let intent = DeleteIntent {
            slug: slug.to_string(),
            github: delete_github,
            created: crate::project::now(),
            repos: settings
                .repos
                .iter()
                .map(|repo| repo.path.clone())
                .collect(),
            worktrees: threads
                .iter()
                .filter(|thread| !thread.worktree_path.is_empty())
                .map(|thread| thread.worktree_path.clone())
                .collect(),
            completed: BTreeSet::new(),
        };
        crate::project::write_atomic(&intent_path, toml::to_string(&intent)?.as_bytes())?;
        println!("recorded deletion plan: {}", intent_path.display());
        intent
    };

    let open_reviews: Vec<_> = crate::review::list(&project)?
        .into_iter()
        .filter(|record| !record.phase.closed())
        .map(|record| record.id)
        .collect();

    if coordinator.is_none()
        && threads.iter().any(|thread| {
            !thread.workspace_id.is_empty()
                || !thread.tab_id.is_empty()
                || !thread.pane_id.is_empty()
        })
    {
        bail!("cannot stop the project's lanes because it has no coordinator session record");
    }
    if let Some(record) = &coordinator {
        let herdr = Herdr::new(ctx.env.herdr_bin(), &record.socket, ctx.runner);
        let mut workspaces = BTreeSet::new();
        let mut tabs = BTreeSet::new();
        if !record.workspace_id.is_empty() {
            workspaces.insert((String::new(), record.workspace_id.clone()));
        }
        for lane in &threads {
            let machine = lane.machine_route().to_string();
            if lane.kind == thread::Kind::Adopted {
                if !lane.tab_id.is_empty() {
                    tabs.insert((machine, lane.tab_id.clone()));
                }
            } else if !lane.workspace_id.is_empty() {
                workspaces.insert((machine, lane.workspace_id.clone()));
            }
        }
        for (machine, tab) in tabs {
            let label = if machine.is_empty() {
                String::new()
            } else {
                format!(" on {machine}")
            };
            close_tab(&herdr.on_machine(&machine), &tab, &label)?;
        }
        for (machine, workspace) in workspaces {
            let label = if machine.is_empty() {
                String::new()
            } else {
                format!(" on {machine}")
            };
            close_workspace(&herdr.on_machine(&machine), &workspace, &label)?;
        }
    }
    for record in &threads {
        println!("stopped lane: {}", record.id);
    }
    for review in open_reviews {
        println!("cancelled review: {review}");
    }

    let mut owned_repos = Vec::new();
    for repo in &settings.repos {
        let users = other_projects_using_repo(&others, repo);
        if users.is_empty() {
            owned_repos.push(repo.clone());
        } else {
            println!(
                "kept shared repo {} (also listed by {})",
                repo.path,
                users.join(", ")
            );
        }
    }

    // A shared checkout stays, but this project's worktrees do not.
    for record in &threads {
        if record.worktree_path.is_empty() {
            continue;
        }
        let path = Path::new(&record.worktree_path);
        let covered_by_owned_repo = owned_repos
            .iter()
            .any(|repo| path.starts_with(Path::new(&repo.path)));
        if covered_by_owned_repo {
            continue;
        }
        let users = other_projects_using_path(&others, path);
        if !users.is_empty() {
            println!(
                "kept shared project worktree {} (also owned by {})",
                path.display(),
                users.join(", ")
            );
            continue;
        }
        if record.is_remote() {
            remote_remove(
                ctx,
                record.machine_route(),
                &record.worktree_path,
                "project worktree",
            )?;
            if let Some(box_repo) = Path::new(&record.worktree_path)
                .parent()
                .and_then(Path::parent)
                .and_then(Path::to_str)
            {
                prune_remote_worktrees(ctx, record.machine_route(), box_repo)?;
            }
        } else {
            trash(ctx, path, "project worktree")?;
            prune_local_worktrees(ctx, &record.repo)?;
        }
    }

    let mut github_candidates = github_names(&owned_repos, &threads);
    if delete_github && github_candidates.is_empty() {
        // Historical rows may not record a publish URL. Resolve only origin,
        // never gh's default remote (which may instead be upstream).
        for repo in &owned_repos {
            match checkout_github_name(ctx, &repo.path) {
                Ok(name) => {
                    github_candidates.insert(name);
                }
                Err(error) => {
                    println!("kept GitHub repo for {}: {error:#}", repo.path);
                }
            }
        }
    }
    let mut other_github: BTreeSet<_> = others
        .iter()
        .flat_map(|owner| github_names(&owner.repos, &owner.threads))
        .collect();
    if delete_github {
        for owner in &others {
            other_github.extend(
                owner
                    .threads
                    .iter()
                    .filter_map(|record| github_name(&record.origin)),
            );
            other_github.extend(
                owner
                    .repos
                    .iter()
                    .filter_map(|repo| checkout_github_name(ctx, &repo.path).ok()),
            );
        }
    }
    let github: BTreeSet<_> = github_candidates
        .difference(&other_github)
        .cloned()
        .collect();
    for name in github_candidates.intersection(&other_github) {
        println!("kept shared GitHub repo: {name}");
    }
    if delete_github {
        for name in &github {
            let completed = format!("github:{name}");
            if delete_intent.completed.contains(&completed) {
                println!("already removed GitHub repo: {name}");
                continue;
            }
            println!("removing GitHub repo: {name}");
            let out = ctx.runner.run(
                &Cmd::new("gh", Duration::from_secs(60)).args(["repo", "delete", name, "--yes"]),
            )?;
            if !out.success() {
                bail!("could not delete GitHub repo {name}: {}", out.error_text());
            }
            delete_intent.completed.insert(completed);
            crate::project::write_atomic(
                &intent_path,
                toml::to_string(&delete_intent)?.as_bytes(),
            )?;
            println!("removed GitHub repo: {name}");
        }
    } else if github.is_empty() {
        println!("GitHub repositories remain; pass --github to remove project-owned copies.");
    } else {
        for name in &github {
            println!("kept GitHub repo: {name} (pass --github to remove it)");
        }
    }

    let mut removed_box_repos = BTreeSet::new();
    for repo in &owned_repos {
        trash(
            ctx,
            Path::new(&repo.path),
            "project repo (including worktrees)",
        )?;
        if let Some(box_path) = &repo.box_path {
            for machine in machines_for_repo(ctx, repo, &threads)? {
                if removed_box_repos.insert((machine.clone(), box_path.clone())) {
                    remote_remove(ctx, &machine, box_path, "box repo (including worktrees)")?;
                }
            }
        }
    }

    let session_cwds: BTreeSet<PathBuf> = project_paths.into_iter().collect();
    let pi_names: BTreeSet<String> = session_cwds
        .iter()
        .map(|path| pi_session_name(path))
        .collect();
    let claude_names: BTreeSet<String> = session_cwds
        .iter()
        .map(|path| claude_session_name(path))
        .collect();
    let other_pi_names: BTreeSet<String> = others
        .iter()
        .flat_map(|owner| owner.paths.iter().map(|path| pi_session_name(path)))
        .collect();
    let other_claude_names: BTreeSet<String> = others
        .iter()
        .flat_map(|owner| owner.paths.iter().map(|path| claude_session_name(path)))
        .collect();
    for (base, exact_names, shared_names) in [
        (ctx.root.join("pi/agent/sessions"), pi_names, other_pi_names),
        (
            ctx.env.home.join(".claude/projects"),
            claude_names,
            other_claude_names,
        ),
    ] {
        for shared in exact_names.intersection(&shared_names) {
            println!(
                "kept shared agent session logs: {}",
                base.join(shared).display()
            );
        }
        let exact_names: BTreeSet<_> = exact_names.difference(&shared_names).cloned().collect();
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                if exact_names.contains(entry.file_name().to_string_lossy().as_ref()) {
                    trash(ctx, &entry.path(), "agent session logs")?;
                }
            }
        }
    }

    // Older versions parked deleted projects here. Remove this slug's copies,
    // but keep and name every other project's retained copy; the redundant
    // holding folder goes only when no retained copy still needs it. Do this
    // before the project record: a cleanup failure must leave the deletion
    // intent available for retry.
    clean_old_trash(ctx, slug)?;

    {
        // No project writer can land after this point. The trash command moves
        // the directory atomically on macOS, while the open lock inode remains
        // valid until this scope ends.
        let _lock = project.lock()?;
        trash(ctx, &project.dir(), "project record")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::ok;
    use crate::scenarios::World;

    #[test]
    fn delete_stops_everything_and_uses_the_system_trash() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let thread = world.thread(&project, world.home.path(), |t| {
            t.kind = thread::Kind::Tab;
            t.worktree_path.clear();
            t.branch.clear();
        });
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            crate::scenarios::pane_json("w2", "w2:t1", "w2:p1", &thread.cwd)
        );
        world.runner.on("/usr/bin/trash", ok(""));

        delete(&world.ctx(), "demo", false, false).unwrap();

        assert_eq!(world.runner.count("workspace close w1"), 1);
        assert!(world.runner.count("tab close") + world.runner.count("workspace close w2") >= 1);
        assert_eq!(world.runner.count("/usr/bin/trash"), 1);
        assert_eq!(world.runner.count("gh repo delete"), 0);
    }

    #[test]
    fn delete_preview_changes_nothing() {
        let world = World::new();
        let project = world.project("demo", "a.sock");

        delete(&world.ctx(), "demo", false, true).unwrap();

        assert!(project.project_md().is_file());
        assert!(!project.state_dir().join("delete.toml").exists());
        assert_eq!(world.runner.count("/usr/bin/trash"), 0);
        assert_eq!(world.runner.count("workspace close"), 0);
    }

    #[test]
    fn delete_keeps_a_repo_listed_by_another_project() {
        let world = World::new();
        let repo = world.home.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let first = crate::project::create(
            &world.root,
            "first",
            "",
            vec![Repo {
                path: repo.display().to_string(),
                ..Repo::default()
            }],
        )
        .unwrap();
        crate::project::create(
            &world.root,
            "second",
            "",
            vec![Repo {
                // Trashing the parent would also trash this separately owned
                // nested checkout, so overlap is shared ownership too.
                path: repo.join("nested").display().to_string(),
                ..Repo::default()
            }],
        )
        .unwrap();
        world.runner.on("/usr/bin/trash", ok(""));

        delete(&world.ctx(), &first.slug, false, false).unwrap();

        let calls = world.runner.calls.borrow();
        assert!(!calls.iter().any(|call| {
            call.program == "/usr/bin/trash"
                && call
                    .args
                    .iter()
                    .any(|arg| arg == &repo.display().to_string())
        }));
    }

    #[test]
    fn delete_refuses_before_trashing_a_repo_inside_its_record_that_another_project_owns() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let nested = project.dir().join("other-project-repo");
        std::fs::create_dir_all(&nested).unwrap();
        crate::project::create(
            &world.root,
            "second",
            "",
            vec![Repo {
                path: nested.display().to_string(),
                ..Repo::default()
            }],
        )
        .unwrap();
        world.runner.on("/usr/bin/trash", ok(""));

        assert!(delete(&world.ctx(), "demo", false, false).is_err());
        assert_eq!(world.runner.count("/usr/bin/trash"), 0);
        assert!(!project.state_dir().join("delete.toml").exists());
    }

    #[test]
    fn delete_keeps_a_github_repo_used_by_another_project() {
        let world = World::new();
        let first_repo = world.home.path().join("first-repo");
        let second_repo = world.home.path().join("second-repo");
        std::fs::create_dir_all(&first_repo).unwrap();
        std::fs::create_dir_all(&second_repo).unwrap();
        let expected_first_repo = std::fs::canonicalize(&first_repo).unwrap();
        crate::project::create(
            &world.root,
            "first",
            "",
            vec![Repo {
                path: first_repo.display().to_string(),
                publish_url: Some("https://github.com/acme/shared.git".into()),
                ..Repo::default()
            }],
        )
        .unwrap();
        crate::project::create(
            &world.root,
            "second",
            "",
            vec![Repo {
                path: second_repo.display().to_string(),
                publish_url: Some("git@github.com:acme/shared.git".into()),
                ..Repo::default()
            }],
        )
        .unwrap();
        world.runner.on("/usr/bin/trash", ok(""));

        delete(&world.ctx(), "first", true, false).unwrap();

        assert_eq!(world.runner.count("gh repo delete"), 0);
        assert!(world.runner.calls.borrow().iter().any(|call| {
            call.program == "/usr/bin/trash"
                && call
                    .args
                    .iter()
                    .any(|arg| arg == &expected_first_repo.display().to_string())
        }));
    }

    #[test]
    fn delete_fallback_names_origin_not_upstream_and_records_identity() {
        let world = World::new();
        let repo = world.home.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let project = crate::project::create(
            &world.root,
            "demo",
            "",
            vec![Repo {
                path: repo.display().to_string(),
                ..Repo::default()
            }],
        )
        .unwrap();
        world.runner.on_fn(
            |cmd| cmd.display().contains("remote get-url"),
            |cmd| {
                Ok(ok(if cmd.args.last().unwrap() == "origin" {
                    "git@github.com:acme/demo.git\n"
                } else {
                    "https://github.com/another/upstream.git\n"
                }))
            },
        );
        world.runner.on("gh repo delete", ok(""));
        world.runner.on("/usr/bin/trash", ok(""));

        delete(&world.ctx(), "demo", true, false).unwrap();

        assert_eq!(world.runner.count("remote get-url origin"), 1);
        assert_eq!(world.runner.count("gh repo delete"), 1);
        let calls = world.runner.calls.borrow();
        let deletion = calls.iter().find(|cmd| cmd.program == "gh").unwrap();
        assert_eq!(deletion.args, ["repo", "delete", "acme/demo", "--yes"]);
        let intent: DeleteIntent = toml::from_str(
            &std::fs::read_to_string(project.state_dir().join("delete.toml")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            intent.completed,
            BTreeSet::from(["github:acme/demo".into()])
        );
    }

    #[test]
    fn delete_fallback_keeps_names_another_project_resolves() {
        for source in ["publish", "origin", "thread"] {
            let world = World::new();
            let first_repo = world.home.path().join("first-repo");
            let second_repo = world.home.path().join("second-repo");
            for path in [&first_repo, &second_repo] {
                std::fs::create_dir_all(path).unwrap();
            }
            crate::project::create(
                &world.root,
                "first",
                "",
                vec![Repo {
                    path: first_repo.display().to_string(),
                    ..Repo::default()
                }],
            )
            .unwrap();
            let second = crate::project::create(
                &world.root,
                "second",
                "",
                vec![Repo {
                    path: second_repo.display().to_string(),
                    publish_url: (source == "publish")
                        .then(|| "https://github.com/acme/shared.git".into()),
                    ..Repo::default()
                }],
            )
            .unwrap();
            if source == "thread" {
                thread::allocate(&second, |record| {
                    record.repo = second_repo.display().to_string();
                    record.origin = "ssh://git@github.com/acme/shared.git".into();
                })
                .unwrap();
            }
            world.runner.on(
                &format!("-C {} remote get-url origin", first_repo.display()),
                ok("git@github.com:acme/shared.git\n"),
            );
            world.runner.on(
                &format!("-C {} remote get-url origin", second_repo.display()),
                if source == "origin" {
                    ok("https://github.com/acme/shared.git\n")
                } else {
                    crate::runner::fake::fail(2, "no origin")
                },
            );
            world.runner.on("/usr/bin/trash", ok(""));

            delete(&world.ctx(), "first", true, false).unwrap();

            assert_eq!(world.runner.count("gh repo delete"), 0, "{source}");
        }
    }

    #[test]
    fn delete_fallback_keeps_unresolvable_origins_and_continues() {
        let world = World::new();
        let mut repos = Vec::new();
        for name in ["missing", "not-github", "spawn-error", "valid"] {
            let path = world.home.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            repos.push(Repo {
                path: path.display().to_string(),
                ..Repo::default()
            });
            let query = format!("-C {} remote get-url origin", path.display());
            if name == "spawn-error" {
                world.runner.on_fn(
                    move |cmd| cmd.display().contains(&query),
                    |_| Err(anyhow::anyhow!("could not run git")),
                );
            } else {
                world.runner.on(
                    &query,
                    match name {
                        "missing" => crate::runner::fake::fail(2, "No such remote 'origin'"),
                        "not-github" => ok("https://example.com/acme/keep.git\n"),
                        _ => ok("https://github.com/acme/delete.git\n"),
                    },
                );
            }
        }
        crate::project::create(&world.root, "demo", "", repos).unwrap();
        world.runner.on("gh repo delete acme/delete --yes", ok(""));
        world.runner.on("/usr/bin/trash", ok(""));

        delete(&world.ctx(), "demo", true, false).unwrap();

        assert_eq!(world.runner.count("remote get-url origin"), 4);
        assert_eq!(world.runner.count("gh repo delete"), 1);
        assert_eq!(world.runner.count("gh repo delete acme/delete --yes"), 1);
    }

    #[test]
    fn delete_keeps_other_projects_old_trash_copies() {
        let world = World::new();
        world.project("demo", "a.sock");
        let holding = world.root.join(".trash");
        let own = holding.join("demo-20260901T000000Z");
        let other = holding.join("demo-other-20260901T000000Z");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        world.runner.on("/usr/bin/trash", ok(""));

        delete(&world.ctx(), "demo", false, false).unwrap();

        let calls = world.runner.calls.borrow();
        assert!(calls.iter().any(|call| {
            call.program == "/usr/bin/trash"
                && call
                    .args
                    .iter()
                    .any(|arg| arg == &own.display().to_string())
        }));
        assert!(!calls.iter().any(|call| {
            call.program == "/usr/bin/trash"
                && call.args.iter().any(|arg| {
                    arg == &other.display().to_string() || arg == &holding.display().to_string()
                })
        }));
        let old_copy = calls
            .iter()
            .position(|call| {
                call.args
                    .iter()
                    .any(|arg| arg == &own.display().to_string())
            })
            .unwrap();
        let project_record = calls
            .iter()
            .position(|call| {
                call.args
                    .iter()
                    .any(|arg| arg == &world.root.join("demo").display().to_string())
            })
            .unwrap();
        assert!(old_copy < project_record);
    }

    #[test]
    fn delete_removes_sessions_and_github_only_when_explicit() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let repo = world.home.path().join("demo-code");
        std::fs::create_dir_all(&repo).unwrap();
        let (mut settings, body) = project.read_project_md().unwrap();
        settings.repos.push(Repo {
            path: repo.display().to_string(),
            publish_url: Some("https://github.com/acme/demo.git".into()),
            ..Repo::default()
        });
        std::fs::write(
            project.project_md(),
            format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();

        let pi_session = world
            .root
            .join("pi/agent/sessions")
            .join(pi_session_name(&project.dir()));
        let claude_session = world
            .home
            .path()
            .join(".claude/projects")
            .join(claude_session_name(&project.dir()));
        std::fs::create_dir_all(&pi_session).unwrap();
        std::fs::create_dir_all(&claude_session).unwrap();

        world.runner.on("gh repo delete acme/demo --yes", ok(""));
        world.runner.on("/usr/bin/trash", ok(""));

        delete(&world.ctx(), "demo", true, false).unwrap();

        assert_eq!(world.runner.count("gh repo delete acme/demo --yes"), 1);
        let calls = world.runner.calls.borrow();
        for path in [repo, pi_session, claude_session] {
            assert!(calls.iter().any(|call| {
                call.program == "/usr/bin/trash"
                    && call
                        .args
                        .iter()
                        .any(|arg| arg == &path.display().to_string())
            }));
        }
    }
}
