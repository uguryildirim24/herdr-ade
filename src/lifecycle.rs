//! pause, resume, archive, unarchive and delete.

use std::collections::{BTreeMap, BTreeSet};
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

#[derive(Deserialize, Default)]
#[serde(default)]
struct ProLane {
    name: String,
    cwd: String,
    parent: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ProTurn {
    tag: String,
    lane: String,
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

fn same_path(a: &str, b: &str) -> bool {
    let a = Path::new(a);
    let b = Path::new(b);
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn other_projects_using(ctx: &Ctx, slug: &str, repo: &Repo) -> Vec<String> {
    let mut users = Vec::new();
    for other_slug in crate::project::list_slugs(&ctx.root) {
        if other_slug == slug {
            continue;
        }
        let Ok(other) = Project::load(&ctx.root, &other_slug) else {
            continue;
        };
        let Ok((settings, _)) = other.read_project_md() else {
            continue;
        };
        if settings.repos.iter().any(|candidate| {
            same_path(&candidate.path, &repo.path)
                || (repo.box_path.is_some()
                    && candidate.machine == repo.machine
                    && candidate.box_path == repo.box_path)
        }) {
            users.push(other_slug);
        }
    }
    users
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

fn stop_pro_lanes(
    ctx: &Ctx,
    project_paths: &[PathBuf],
    thread_names: &BTreeSet<String>,
    coordinator_pane: Option<&str>,
) -> Result<()> {
    let root = ctx.root.join("pro-bridge");
    let lanes = root.join("lanes");
    let mut names = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(&lanes) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(lane) = toml::from_str::<ProLane>(&text) else {
                continue;
            };
            let cwd = Path::new(&lane.cwd);
            let belongs = thread_names.contains(&lane.name)
                || lane
                    .parent
                    .as_deref()
                    .zip(coordinator_pane)
                    .is_some_and(|(parent, coordinator)| parent == coordinator)
                || project_paths.iter().any(|base| cwd == base);
            if belongs {
                names.insert(lane.name);
            }
        }
    }

    let mut turn_tags: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(root.join("turns")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            if let Ok(turn) = toml::from_str::<ProTurn>(&text)
                && names.contains(&turn.lane)
            {
                turn_tags.insert(turn.tag, turn.lane);
            }
        }
    }

    for name in &names {
        let out = ctx
            .runner
            .run(&Cmd::new("herdr-pro", Duration::from_secs(20)).args(["stop", name]))?;
        if !out.success() {
            bail!("could not stop Pro lane `{name}`: {}", out.error_text());
        }
        println!("stopped Pro lane: {name}");
        trash(ctx, &lanes.join(format!("{name}.toml")), "Pro lane record")?;
    }
    for (tag, _) in turn_tags {
        for path in [
            root.join("turns").join(format!("{tag}.toml")),
            root.join("turns").join(format!("{tag}.collector.lock")),
            root.join("packets").join(format!("{tag}.md")),
            root.join("inflight").join(format!("{tag}.lock")),
        ] {
            trash(ctx, &path, "Pro bridge file")?;
        }
    }
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
    let threads = thread::list(&project);
    let coordinator = project.coordinator();

    let mut project_paths = vec![project.dir()];
    project_paths.extend(
        threads
            .iter()
            .filter(|thread| !thread.worktree_path.is_empty())
            .map(|thread| PathBuf::from(&thread.worktree_path)),
    );
    let pro_names: BTreeSet<String> = threads
        .iter()
        .filter(|thread| thread.role == "pro" || thread.launch.kind == "pro")
        .flat_map(|thread| [thread.agent_name.clone(), thread.agent.clone()])
        .filter(|name| !name.is_empty())
        .collect();

    if preview {
        println!("Archive keeps `{slug}` and its files available for unarchive.");
        println!("Delete removes these explicitly recorded resources:");
        println!("  project record: {}", project.dir().display());
        for repo in &settings.repos {
            let shared = other_projects_using(ctx, slug, repo);
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

    let open_rounds: Vec<_> = crate::round::list(&project)
        .into_iter()
        .filter(|record| !record.phase.closed())
        .map(|record| record.round)
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
    for round in open_rounds {
        println!("cancelled round: {round}");
    }

    stop_pro_lanes(
        ctx,
        &project_paths,
        &pro_names,
        coordinator.as_ref().map(|record| record.pane_id.as_str()),
    )?;

    let mut owned_repos = Vec::new();
    for repo in &settings.repos {
        let users = other_projects_using(ctx, slug, repo);
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

    let mut github = BTreeSet::new();
    for repo in &owned_repos {
        if let Some(url) = repo.publish_url.as_deref().and_then(github_name) {
            github.insert(url);
        }
        for record in threads
            .iter()
            .filter(|thread| same_path(&thread.repo, &repo.path))
        {
            if let Some(name) = github_name(&record.origin) {
                github.insert(name);
            }
        }
    }
    if delete_github {
        for name in &github {
            let completed = format!("github:{name}");
            if delete_intent.completed.contains(&completed) {
                println!("already removed GitHub repo: {name}");
                continue;
            }
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
        // Historical repository rows may have no publish URL. `gh` resolves
        // the repository from its checkout, before that checkout is trashed.
        if github.is_empty() {
            for repo in &owned_repos {
                if !Path::new(&repo.path).exists() {
                    continue;
                }
                let completed = format!("github-path:{}", repo.path);
                if delete_intent.completed.contains(&completed) {
                    println!("already removed GitHub repo for: {}", repo.path);
                    continue;
                }
                let out = ctx.runner.run(
                    &Cmd::new("gh", Duration::from_secs(60))
                        .args(["repo", "delete", "--yes"])
                        .cwd(&repo.path),
                )?;
                if !out.success() {
                    bail!(
                        "could not delete the GitHub repo for {}: {}",
                        repo.path,
                        out.error_text()
                    );
                }
                delete_intent.completed.insert(completed);
                crate::project::write_atomic(
                    &intent_path,
                    toml::to_string(&delete_intent)?.as_bytes(),
                )?;
                println!("removed GitHub repo for: {}", repo.path);
            }
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

    let mut session_cwds: BTreeSet<PathBuf> = project_paths.into_iter().collect();
    session_cwds.extend(
        threads
            .iter()
            .filter(|thread| !thread.cwd.is_empty())
            .map(|thread| PathBuf::from(&thread.cwd)),
    );
    let pi_names: BTreeSet<String> = session_cwds
        .iter()
        .map(|path| pi_session_name(path))
        .collect();
    let claude_names: BTreeSet<String> = session_cwds
        .iter()
        .map(|path| claude_session_name(path))
        .collect();
    for (base, exact_names) in [
        (ctx.root.join("pi/agent/sessions"), pi_names),
        (ctx.env.home.join(".claude/projects"), claude_names),
    ] {
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                if exact_names.contains(entry.file_name().to_string_lossy().as_ref()) {
                    trash(ctx, &entry.path(), "agent session logs")?;
                }
            }
        }
    }

    {
        // No project writer can land after this point. The trash command moves
        // the directory atomically on macOS, while the open lock inode remains
        // valid until this scope ends.
        let _lock = project.lock()?;
        trash(ctx, &project.dir(), "project record")?;
    }

    // Older versions parked deleted projects here. Nothing reads that holding
    // folder now; preserve its contents in the system Trash and remove the
    // redundant second trash location.
    trash(ctx, &ctx.root.join(".trash"), "old trash holding folder")?;
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
                path: repo.display().to_string(),
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
    fn delete_removes_sessions_pro_files_and_github_only_when_explicit() {
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

        let pro = world.root.join("pro-bridge");
        for dir in ["lanes", "turns", "packets"] {
            std::fs::create_dir_all(pro.join(dir)).unwrap();
        }
        std::fs::write(
            pro.join("lanes/pro-demo.toml"),
            "name = \"pro-demo\"\ncwd = \"/somewhere\"\nparent = \"w1:p1\"\n",
        )
        .unwrap();
        std::fs::write(
            pro.join("turns/pro-demo-01.toml"),
            "tag = \"pro-demo-01\"\nlane = \"pro-demo\"\n",
        )
        .unwrap();
        std::fs::write(pro.join("packets/pro-demo-01.md"), "packet").unwrap();
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

        world.runner.on("herdr-pro stop pro-demo", ok(""));
        world.runner.on("gh repo delete acme/demo --yes", ok(""));
        world.runner.on("/usr/bin/trash", ok(""));

        delete(&world.ctx(), "demo", true, false).unwrap();

        assert_eq!(world.runner.count("herdr-pro stop pro-demo"), 1);
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
        assert!(calls.iter().any(|call| {
            call.program == "/usr/bin/trash"
                && call
                    .args
                    .iter()
                    .any(|arg| arg.ends_with("pro-bridge/packets/pro-demo-01.md"))
        }));
    }

    #[test]
    fn archive_clears_tokens_and_blocks_pause() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        let ctx = world.ctx();
        set_status(&ctx, "demo", Status::Archived).unwrap();
        assert_eq!(project.status(), Status::Archived);
        let calls = world.runner.calls.borrow();
        let clear = calls
            .iter()
            .find(|c| c.display().contains("--clear-token"))
            .expect("tokens cleared");
        assert!(clear.display().contains("w1:p1"));
        drop(calls);
        assert!(set_status(&ctx, "demo", Status::Paused).is_err());
        set_status(&ctx, "demo", Status::Active).unwrap();
        assert_eq!(project.status(), Status::Active);
    }
}
