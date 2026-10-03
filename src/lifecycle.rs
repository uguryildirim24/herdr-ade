//! pause, resume, archive, unarchive and delete.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
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
    let _binding = project.coordinator_lock()?;
    let current = project.status();
    match (current, status) {
        (Status::Archived, Status::Paused) => bail!("`{slug}` is archived; `unarchive` it first"),
        (Status::Archived, Status::Active)
        | (_, Status::Archived)
        | (_, Status::Paused)
        | (Status::Paused, Status::Active)
        | (Status::Active, Status::Active) => {}
    }
    if status != Status::Archived {
        let path = deletion_path(&ctx.root, slug);
        if path.exists() {
            let intent: DeleteIntent = toml::from_str(&std::fs::read_to_string(path)?)?;
            if intent.started && !intent.finished() {
                bail!("`{slug}` is being deleted; finish the persisted deletion first");
            }
        }
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
    #[serde(default)]
    steps: Option<Vec<DeleteStep>>,
    #[serde(default)]
    topology: String,
    #[serde(default)]
    started: bool,
}

impl DeleteIntent {
    fn finished(&self) -> bool {
        self.steps.as_ref().is_some_and(|steps| {
            !steps.is_empty() && (0..steps.len()).all(|i| self.completed.contains(&i.to_string()))
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum DeleteStep {
    Workspace {
        socket: String,
        machine: String,
        id: String,
    },
    Tab {
        socket: String,
        machine: String,
        id: String,
    },
    Trash {
        path: String,
        machine: String,
        identity: String,
        what: String,
    },
    Prune {
        repo: String,
        machine: String,
        identity: String,
    },
    Github {
        name: String,
        identity: String,
    },
}

struct OtherProjectOwnership {
    slug: String,
    repos: Vec<Repo>,
    threads: Vec<thread::Thread>,
    paths: BTreeSet<PathBuf>,
    coordinator: Option<crate::project::Coordinator>,
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
    let (slugs, errors) = crate::project::list_slugs_with_errors(&ctx.root);
    if !errors.is_empty() {
        bail!(
            "cannot prove ownership: {}",
            errors
                .iter()
                .map(|e| format!("{e:#}"))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    for other_slug in slugs {
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
            coordinator: readable_coordinator(&project)?,
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
            owner.repos.iter().any(|repo| {
                paths_overlap(&repo.path, &path)
                    || repo
                        .box_path
                        .as_deref()
                        .is_some_and(|p| paths_overlap(p, &path))
            }) || owner
                .paths
                .iter()
                .any(|candidate| paths_overlap(&candidate.to_string_lossy(), &path))
        })
        .map(|owner| owner.slug.clone())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trash {
    Mac,
    Gio,
    Put,
}

impl Trash {
    fn probe_script(platform: &str) -> Result<&'static str> {
        match platform {
            "macos" => Ok("test -x /usr/bin/trash && printf /usr/bin/trash"),
            "linux" => Ok(
                "if command -v gio >/dev/null 2>&1; then printf gio; elif command -v trash-put >/dev/null 2>&1; then printf trash-put; else exit 1; fi",
            ),
            _ => bail!("system trash is not supported on {platform}"),
        }
    }

    fn from_probe(out: &crate::runner::Output) -> Result<Self> {
        if out.success() {
            match out.stdout.trim() {
                "/usr/bin/trash" => return Ok(Self::Mac),
                "gio" => return Ok(Self::Gio),
                "trash-put" => return Ok(Self::Put),
                _ => {}
            }
        }
        bail!(
            "system trash is unavailable: macOS needs /usr/bin/trash; Linux needs gio or trash-put; nothing was deleted"
        )
    }

    fn local(ctx: &Ctx) -> Result<Self> {
        let script = Self::probe_script(std::env::consts::OS)?;
        let out = ctx
            .runner
            .run(&Cmd::new("sh", Duration::from_secs(5)).args(["-c", script]))?;
        Self::from_probe(&out)
    }

    fn remote(ctx: &Ctx, machine: &str) -> Result<Self> {
        let profile =
            remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, machine)?;
        let script = format!(
            "case \"$(uname -s)\" in Darwin) {} ;; Linux) {} ;; *) exit 1 ;; esac",
            Self::probe_script("macos")?,
            Self::probe_script("linux")?,
        );
        let out = remote::ssh(
            ctx.runner,
            &profile.target,
            &script,
            None,
            Duration::from_secs(10),
        )?;
        Self::from_probe(&out).map_err(|error| anyhow::anyhow!("on {machine}: {error}"))
    }

    fn program(self) -> &'static str {
        match self {
            Self::Mac => "/usr/bin/trash",
            Self::Gio => "gio",
            Self::Put => "trash-put",
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            Self::Mac => &[],
            Self::Gio => &["trash", "--"],
            Self::Put => &["--"],
        }
    }
}

fn trash(ctx: &Ctx, tool: Trash, path: &Path, what: &str) -> Result<()> {
    if std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(());
    }
    let out = ctx.runner.run(
        &Cmd::new(tool.program(), Duration::from_secs(60))
            .args(tool.args().iter().copied())
            .arg(path.to_string_lossy()),
    )?;
    if !out.success() {
        bail!(
            "could not move {} to the system trash: {}",
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

fn checkout_origin(ctx: &Ctx, path: &str) -> Result<String> {
    crate::repo::Git::new(ctx.runner, path)
        .with_timeout(Duration::from_secs(40))
        .run(&["remote", "get-url", "origin"])
        .context("could not resolve origin")
}

fn checkout_github_name(ctx: &Ctx, path: &str) -> Result<String> {
    let origin = checkout_origin(ctx, path)?;
    github_name(&origin).ok_or_else(|| anyhow::anyhow!("origin is not a GitHub repo: {origin}"))
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
    crate::repo::Git::new(ctx.runner, repo)
        .with_timeout(Duration::from_secs(30))
        .run(&["worktree", "prune"])
        .with_context(|| format!("could not reconcile worktrees in {repo}"))?;
    println!("reconciled worktrees in kept repo: {repo}");
    Ok(())
}

fn remote_remove(ctx: &Ctx, tool: Trash, machine: &str, path: &str, what: &str) -> Result<()> {
    if !Path::new(path).is_absolute() || path == "/" {
        bail!("refusing to remove unsafe box path `{path}`");
    }
    let profile =
        remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, machine)?;
    let quoted = remote::quote(path);
    let script = format!(
        "if [ -e {quoted} ] || [ -L {quoted} ]; then {} {} {quoted}; fi",
        tool.program(),
        tool.args().join(" "),
    );
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

fn readable_coordinator(project: &Project) -> Result<Option<crate::project::Coordinator>> {
    match std::fs::read(project.record_file("coordinator.json")) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

// Only resource bindings, not progress/status, qualify the snapshot. A new
// lane or rebound terminal cannot silently become part of an earlier preview.
fn topology(ctx: &Ctx, project: &Project, threads: &[thread::Thread]) -> Result<String> {
    let coordinator = readable_coordinator(project)?.map(|c| {
        (
            crate::ticker::socket_inode(Path::new(&c.socket)),
            c.socket,
            c.workspace_id,
            c.tab_id,
            c.pane_id,
            c.generation,
        )
    });
    let mut targets = BTreeMap::new();
    for t in threads.iter().filter(|t| t.is_remote()) {
        targets.insert(
            t.machine_route(),
            remote::machine_profile(
                ctx.runner,
                &ctx.env.herdr_bin(),
                &ctx.config_dir,
                t.machine_route(),
            )?
            .target,
        );
    }
    let lanes: Vec<_> = threads
        .iter()
        .map(|t| {
            (
                &t.id,
                t.kind,
                t.machine_route(),
                &t.workspace_id,
                &t.tab_id,
                &t.pane_id,
                &t.repo,
                &t.worktree_path,
                &t.cwd,
                &t.origin,
            )
        })
        .collect();
    Ok(thread::sha256_hex(&serde_json::to_vec(&(
        coordinator,
        lanes,
        targets,
    ))?))
}

fn file_identity(ctx: &Ctx, machine: &str, path: &str) -> Result<Option<String>> {
    if machine.is_empty() {
        match std::fs::symlink_metadata(path) {
            #[cfg(unix)]
            Ok(metadata) => {
                use std::os::unix::fs::MetadataExt;
                Ok(Some(format!("{}:{}", metadata.dev(), metadata.ino())))
            }
            #[cfg(not(unix))]
            Ok(_) => bail!("deletion identity is unsupported on this platform"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    } else {
        if !Path::new(path).is_absolute() || path == "/" {
            bail!("refusing unsafe box path `{path}`");
        }
        let profile =
            remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, machine)?;
        let path = remote::quote(path);
        let script = format!(
            "if [ -e {path} ] || [ -L {path} ]; then case \"$(uname -s)\" in Darwin) stat -f '%d:%i' {path} ;; Linux) stat -c '%d:%i' -- {path} ;; *) exit 1 ;; esac; else printf missing; fi"
        );
        let out = remote::ssh(
            ctx.runner,
            &profile.target,
            &script,
            None,
            Duration::from_secs(10),
        )?;
        let identity = out.stdout.trim();
        if !out.success() || identity.is_empty() {
            bail!(
                "cannot prove file identity on {machine}: {}",
                out.error_text()
            );
        }
        Ok((identity != "missing").then(|| format!("{}:{identity}", profile.target)))
    }
}

fn github_identity(ctx: &Ctx, name: &str) -> Result<String> {
    let out = ctx
        .runner
        .run(&Cmd::new("gh", Duration::from_secs(40)).args([
            "api",
            &format!("repos/{name}"),
            "--jq",
            ".node_id",
        ]))?;
    if !out.success() || out.stdout.trim().is_empty() || out.stdout.trim() == "null" {
        bail!(
            "cannot prove GitHub identity for {name}: {}",
            out.error_text()
        );
    }
    Ok(out.stdout.trim().to_string())
}

fn recorded_github(others: &[OtherProjectOwnership]) -> BTreeSet<String> {
    others
        .iter()
        .flat_map(|o| {
            github_names(&o.repos, &o.threads)
                .into_iter()
                .chain(o.threads.iter().filter_map(|t| github_name(&t.origin)))
        })
        .collect()
}

fn other_github(ctx: &Ctx, others: &[OtherProjectOwnership]) -> Result<BTreeSet<String>> {
    let mut names = recorded_github(others);
    for repo in others.iter().flat_map(|o| &o.repos) {
        // Origin and publish URL can name different repositories. A successful
        // non-GitHub origin is evidence; an unreadable origin is not.
        if let Some(name) = github_name(&checkout_origin(ctx, &repo.path)?) {
            names.insert(name);
        }
    }
    Ok(names)
}

fn terminal_users(
    others: &[OtherProjectOwnership],
    socket: &str,
    machine: &str,
    id: &str,
    workspace: bool,
) -> Vec<String> {
    others
        .iter()
        .filter(|owner| {
            if owner
                .coordinator
                .as_ref()
                .is_some_and(|c| c.socket != socket)
            {
                return false;
            }
            (machine.is_empty()
                && owner.coordinator.as_ref().is_some_and(|c| {
                    if workspace {
                        c.workspace_id == id
                    } else {
                        c.tab_id == id
                    }
                }))
                || owner.threads.iter().any(|t| {
                    t.machine_route() == machine
                        && if workspace {
                            t.workspace_id == id
                        } else {
                            t.tab_id == id
                        }
                })
        })
        .map(|owner| owner.slug.clone())
        .collect()
}

fn add_trash(
    ctx: &Ctx,
    steps: &mut Vec<DeleteStep>,
    path: &Path,
    machine: &str,
    what: &str,
) -> Result<()> {
    let path = path.to_string_lossy().into_owned();
    if let Some(identity) = file_identity(ctx, machine, &path)?
        && !steps.iter().any(|step| matches!(step, DeleteStep::Trash { path: p, machine: m, .. } if p == &path && m == machine))
    {
        steps.push(DeleteStep::Trash { path, machine: machine.into(), identity, what: what.into() });
    }
    Ok(())
}

fn build_plan(ctx: &Ctx, project: &Project, github: bool) -> Result<DeleteIntent> {
    let (settings, _) = project.read_project_md()?;
    let threads = readable_threads(project)?;
    let others = other_ownership(ctx, &project.slug)?;
    if !other_projects_using_path(&others, &project.dir()).is_empty() {
        bail!(
            "cannot delete `{}`: another project owns resources inside its record",
            project.slug
        );
    }
    let owned: Vec<_> = settings
        .repos
        .iter()
        .filter(|repo| {
            let mut users = other_projects_using_repo(&others, repo);
            users.extend(other_projects_using_path(&others, Path::new(&repo.path)));
            if let Some(path) = &repo.box_path {
                users.extend(other_projects_using_path(&others, Path::new(path)));
            }
            users.sort();
            users.dedup();
            if !users.is_empty() {
                println!("kept shared repo {} ({})", repo.path, users.join(", "));
            }
            users.is_empty()
        })
        .cloned()
        .collect();
    let mut steps = Vec::new();
    if let Some(c) = readable_coordinator(project)? {
        let mut workspaces = BTreeSet::new();
        let mut tabs: BTreeSet<(String, String)> = BTreeSet::new();
        if !c.workspace_id.is_empty() {
            workspaces.insert((String::new(), c.workspace_id.clone()));
        }
        for t in &threads {
            if t.kind == thread::Kind::Adopted {
                if !t.tab_id.is_empty() {
                    tabs.insert((t.machine_route().into(), t.tab_id.clone()));
                }
            } else if !t.workspace_id.is_empty() {
                workspaces.insert((t.machine_route().into(), t.workspace_id.clone()));
            }
        }
        // A workspace containing adopted/shared work stays; close only this
        // project's exclusive tabs in it, including the coordinator's tab.
        workspaces.retain(|(machine, id)| {
            let shared = !terminal_users(&others, &c.socket, machine, id, true).is_empty()
                || threads.iter().any(|t| {
                    t.kind == thread::Kind::Adopted
                        && t.workspace_id == *id
                        && t.machine_route() == machine
                });
            if shared {
                if machine.is_empty() && c.workspace_id == *id && !c.tab_id.is_empty() {
                    tabs.insert((machine.clone(), c.tab_id.clone()));
                }
                for t in &threads {
                    if t.workspace_id == *id && t.machine_route() == machine && !t.tab_id.is_empty()
                    {
                        tabs.insert((machine.clone(), t.tab_id.clone()));
                    }
                }
            }
            !shared
        });
        for (machine, id) in tabs {
            if terminal_users(&others, &c.socket, &machine, &id, false).is_empty() {
                steps.push(DeleteStep::Tab {
                    socket: c.socket.clone(),
                    machine,
                    id,
                });
            }
        }
        for (machine, id) in workspaces {
            if terminal_users(&others, &c.socket, &machine, &id, true).is_empty()
                && !threads.iter().any(|t| {
                    t.kind == thread::Kind::Adopted
                        && t.workspace_id == id
                        && t.machine_route() == machine
                })
            {
                steps.push(DeleteStep::Workspace {
                    socket: c.socket.clone(),
                    machine,
                    id,
                });
            }
        }
    } else if threads
        .iter()
        .any(|t| !t.workspace_id.is_empty() || !t.tab_id.is_empty() || !t.pane_id.is_empty())
    {
        bail!("cannot stop the project's lanes because it has no coordinator session record");
    }
    for t in &threads {
        if t.worktree_path.is_empty() || t.kind == thread::Kind::Adopted {
            continue;
        }
        let machine = if t.is_remote() { t.machine_route() } else { "" };
        let path = Path::new(&t.worktree_path);
        let covered = owned.iter().any(|r| {
            if machine.is_empty() {
                path.starts_with(&r.path)
            } else {
                r.box_path.as_ref().is_some_and(|p| path.starts_with(p))
            }
        });
        if covered || !other_projects_using_path(&others, path).is_empty() {
            continue;
        }
        add_trash(ctx, &mut steps, path, machine, "project worktree")?;
        let repo = if machine.is_empty() {
            Some(t.repo.as_str())
        } else {
            path.parent().and_then(Path::parent).and_then(Path::to_str)
        };
        if let Some(repo) = repo
            && let Some(identity) = file_identity(ctx, machine, repo)?
        {
            steps.push(DeleteStep::Prune {
                repo: repo.into(),
                machine: machine.into(),
                identity,
            });
        }
    }
    if github {
        let mut names = github_names(&owned, &threads);
        if names.is_empty() {
            for repo in &owned {
                match checkout_github_name(ctx, &repo.path) {
                    Ok(name) => {
                        names.insert(name);
                    }
                    Err(e) => println!("kept GitHub repo for {}: {e:#}", repo.path),
                }
            }
        }
        let recorded = recorded_github(&others);
        let candidates: BTreeSet<_> = names.difference(&recorded).cloned().collect();
        // No origin reads are needed if recorded ownership already excludes
        // every GitHub effect. Otherwise every ownership read must succeed.
        let shared = if candidates.is_empty() {
            recorded
        } else {
            other_github(ctx, &others)?
        };
        for name in candidates.difference(&shared) {
            steps.push(DeleteStep::Github {
                name: name.clone(),
                identity: github_identity(ctx, name)?,
            });
        }
    }
    for repo in &owned {
        add_trash(
            ctx,
            &mut steps,
            Path::new(&repo.path),
            "",
            "project repo (including worktrees)",
        )?;
        if let Some(path) = &repo.box_path {
            for machine in machines_for_repo(ctx, repo, &threads)? {
                add_trash(
                    ctx,
                    &mut steps,
                    Path::new(path),
                    &machine,
                    "box repo (including worktrees)",
                )?;
            }
        }
    }
    let paths = owned_paths(project, &threads);
    for (base, naming) in [
        (
            ctx.root.join("pi/agent/sessions"),
            pi_session_name as fn(&Path) -> String,
        ),
        (
            ctx.env.home.join(".claude/projects"),
            claude_session_name as fn(&Path) -> String,
        ),
    ] {
        let shared: BTreeSet<_> = others
            .iter()
            .flat_map(|o| o.paths.iter().map(|p| naming(p)))
            .collect();
        for name in paths
            .iter()
            .map(|p| naming(p))
            .collect::<BTreeSet<_>>()
            .difference(&shared)
        {
            add_trash(ctx, &mut steps, &base.join(name), "", "agent session logs")?;
        }
    }
    let holding = ctx.root.join(".trash");
    match std::fs::read_dir(&holding) {
        Ok(entries) => {
            let mut entries = entries.collect::<std::io::Result<Vec<_>>>()?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| old_copy_for_slug(n, &project.slug))
                {
                    add_trash(
                        ctx,
                        &mut steps,
                        &entry.path(),
                        "",
                        "old copy of this project",
                    )?;
                }
            }
            // The holding directory is not a finite target: a later copy could
            // belong to another project. Leave the empty container, not its data.
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let record = std::fs::canonicalize(project.dir())?;
    let journal = std::fs::canonicalize(ctx.root.join(".deletions"))?;
    for step in &steps {
        if let DeleteStep::Trash { path, machine, .. } = step
            && machine.is_empty()
        {
            // Identity pins the inode; containment must resolve aliases too.
            let target = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
            if record.starts_with(&target) {
                bail!("planned target would remove the project record before completion: {path}");
            }
            if journal.starts_with(&target) {
                bail!("planned target would remove the deletion journal: {path}");
            }
        }
    }
    add_trash(ctx, &mut steps, &project.dir(), "", "project record")?;
    Ok(DeleteIntent {
        slug: project.slug.clone(),
        github,
        created: crate::project::now(),
        repos: settings
            .repos
            .iter()
            .flat_map(|r| std::iter::once(r.path.clone()).chain(r.box_path.clone()))
            .collect(),
        worktrees: threads.iter().map(|t| t.worktree_path.clone()).collect(),
        completed: BTreeSet::new(),
        steps: Some(steps),
        topology: topology(ctx, project, &threads)?,
        started: false,
    })
}

fn check_step(ctx: &Ctx, project: &Project, plan: &DeleteIntent, step: &DeleteStep) -> Result<()> {
    let others = other_ownership(ctx, &project.slug)?;
    let users = match step {
        DeleteStep::Workspace {
            socket,
            machine,
            id,
        } => terminal_users(&others, socket, machine, id, true),
        DeleteStep::Tab {
            socket,
            machine,
            id,
        } => terminal_users(&others, socket, machine, id, false),
        DeleteStep::Github { name, identity } => {
            if github_identity(ctx, name)? != *identity {
                bail!("GitHub identity changed: {name}");
            }
            if other_github(ctx, &others)?.contains(name) {
                vec!["another project".into()]
            } else {
                Vec::new()
            }
        }
        DeleteStep::Trash {
            path,
            machine,
            identity,
            what,
        } => {
            if let Some(current) = file_identity(ctx, machine, path)?
                && current != *identity
            {
                bail!("file identity changed: {path}");
            }
            let mut users = other_projects_using_path(&others, Path::new(path));
            if what == "agent session logs" {
                for owner in &others {
                    if owner.paths.iter().any(|p| {
                        Path::new(path).file_name().is_some_and(|name| {
                            name == pi_session_name(p).as_str()
                                || name == claude_session_name(p).as_str()
                        })
                    }) {
                        users.push(owner.slug.clone());
                    }
                }
            }
            // Additions to our settings are not covered by an old parent target.
            for repo in project.read_project_md()?.0.repos {
                for candidate in std::iter::once(&repo.path).chain(repo.box_path.as_ref()) {
                    if !plan.repos.contains(candidate) && paths_overlap(candidate, path) {
                        bail!("new repo overlaps planned deletion: {candidate}");
                    }
                }
            }
            users
        }
        DeleteStep::Prune {
            repo,
            machine,
            identity,
        } => {
            if let Some(current) = file_identity(ctx, machine, repo)?
                && current != *identity
            {
                bail!("kept repo identity changed: {repo}");
            }
            Vec::new()
        }
    };
    if !users.is_empty() {
        bail!(
            "planned resource is now owned by {}: {step:?}",
            users.join(", ")
        );
    }
    Ok(())
}

fn deletion_path(root: &Path, slug: &str) -> PathBuf {
    root.join(".deletions").join(format!("{slug}.toml"))
}

fn save_plan(path: &Path, plan: &DeleteIntent) -> Result<()> {
    crate::project::write_atomic(path, toml::to_string(plan)?.as_bytes())
}

/// Preview freezes exactly the steps execution will use. Execution keeps both
/// lifecycle locks through the effects, and archives before the first effect:
/// allocations serialize with deletion and a failed delete cannot restart work.
pub(crate) fn delete(ctx: &Ctx, slug: &str, delete_github: bool, preview: bool) -> Result<()> {
    crate::project::validate_slug(slug)?;
    let path = deletion_path(&ctx.root, slug);
    std::fs::create_dir_all(path.parent().expect("journal directory"))?;
    let _journal = crate::project::lock_file(&path.with_extension("lock"))?;
    let mut saved = match std::fs::read_to_string(&path) {
        Ok(text) => Some(toml::from_str::<DeleteIntent>(&text)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(plan) = &saved {
        let steps = plan.steps.as_ref().ok_or_else(|| anyhow::anyhow!("historical deletion intent has no identity-qualified execution plan; nothing was deleted"))?;
        if plan.slug != slug
            || !matches!(steps.last(), Some(DeleteStep::Trash { path: p, machine, what, .. }) if p == &ctx.root.join(slug).to_string_lossy() && machine.is_empty() && what == "project record")
            || plan.completed.iter().any(|key| {
                key.parse::<usize>().is_err()
                    || key.parse::<usize>().is_ok_and(|i| i >= steps.len())
            })
        {
            bail!("invalid deletion plan for `{slug}`; nothing was deleted");
        }
        let new_incarnation = plan.finished()
            && match steps.last() {
                Some(DeleteStep::Trash {
                    path: p, identity, ..
                }) => file_identity(ctx, "", p)?.is_some_and(|current| current != *identity),
                _ => false,
            };
        if plan.github != delete_github && !new_incarnation && !(preview && !plan.started) {
            bail!(
                "delete is already planned for `{}` with GitHub deletion {}; retry with the same choice",
                plan.slug,
                plan.github
            );
        }
    }
    // The journal is outside the record, so even the final move has a durable
    // completion. If the process died between that move and its receipt, absence
    // proves the record is gone; no destructive command needs repeating.
    if let Some(plan) = saved.as_mut()
        && plan.started
        && let Some(steps) = &plan.steps
        && let Some(DeleteStep::Trash {
            path: record, what, ..
        }) = steps.last()
        && what == "project record"
        && record == &ctx.root.join(slug).to_string_lossy()
        && (0..steps.len() - 1).all(|i| plan.completed.contains(&i.to_string()))
        && file_identity(ctx, "", record)?.is_none()
    {
        plan.completed.insert((steps.len() - 1).to_string());
        save_plan(&path, plan)?;
        return Ok(());
    }
    let project = Project::load(&ctx.root, slug)?;
    let _binding = project.coordinator_lock()?;
    let _records = project.lock()?;
    // Historical records still deserialize, but their unqualified intent cannot
    // be upgraded using today's resource lists: that would execute a stale plan.
    let legacy = project.record_file("delete.toml");
    if legacy.exists() {
        let _: DeleteIntent = toml::from_str(&std::fs::read_to_string(legacy)?)?;
        bail!(
            "historical deletion intent has no identity-qualified execution plan; nothing was deleted"
        );
    }
    let mut plan = match saved {
        // A fresh preview may replace an unstarted snapshot. Once effects have
        // begun, even preview must render the frozen plan, never expand it.
        Some(plan) if preview && !plan.started => build_plan(ctx, &project, delete_github)?,
        Some(plan) if plan.finished() => match plan.steps.as_ref().and_then(|steps| steps.last()) {
            Some(DeleteStep::Trash {
                path: record,
                identity,
                what,
                ..
            }) if what == "project record"
                && file_identity(ctx, "", record)?.as_ref() != Some(identity) =>
            {
                build_plan(ctx, &project, delete_github)?
            }
            _ => plan,
        },
        Some(plan) => plan,
        None => build_plan(ctx, &project, delete_github)?,
    };
    let steps = plan.steps.as_ref().ok_or_else(|| anyhow::anyhow!("historical deletion intent has no identity-qualified execution plan; nothing was deleted"))?;
    if preview {
        save_plan(&path, &plan)?;
        println!("Archive keeps `{slug}` and its files available for unarchive.");
        println!("Persisted deletion plan: {}", path.display());
        for (i, step) in steps.iter().enumerate() {
            println!(
                "  {}: {step:?}{}",
                i + 1,
                if plan.completed.contains(&i.to_string()) {
                    " (completed)"
                } else {
                    ""
                }
            );
        }
        println!(
            "  GitHub: {}",
            if plan.github {
                "delete only listed repositories"
            } else {
                "keep repositories"
            }
        );
        return Ok(());
    }
    if topology(ctx, &project, &readable_threads(&project)?)? != plan.topology {
        bail!("project resource bindings changed since preview; refusing stale deletion plan");
    }
    // Preflight all remaining targets and trash tools before closing anything.
    let local_trash = Trash::local(ctx)?;
    let mut remote_trash = BTreeMap::new();
    for (i, step) in steps.iter().enumerate() {
        if plan.completed.contains(&i.to_string()) {
            continue;
        }
        check_step(ctx, &project, &plan, step)?;
        if let DeleteStep::Trash { machine, .. } = step
            && !machine.is_empty()
            && !remote_trash.contains_key(machine)
        {
            remote_trash.insert(machine.clone(), Trash::remote(ctx, machine)?);
        }
    }
    plan.started = true;
    save_plan(&path, &plan)?;
    crate::project::write_json(
        &project.record_file("project.json"),
        &serde_json::json!({"status": "archived"}),
    )?;
    for i in 0..plan.steps.as_ref().expect("checked").len() {
        let key = i.to_string();
        if plan.completed.contains(&key) {
            continue;
        }
        let step = &plan.steps.as_ref().expect("checked")[i];
        check_step(ctx, &project, &plan, step)?;
        match step {
            DeleteStep::Workspace {
                socket,
                machine,
                id,
            } => close_workspace(
                &Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner).on_machine(machine),
                id,
                machine,
            )?,
            DeleteStep::Tab {
                socket,
                machine,
                id,
            } => close_tab(
                &Herdr::new(ctx.env.herdr_bin(), socket, ctx.runner).on_machine(machine),
                id,
                machine,
            )?,
            DeleteStep::Trash {
                path,
                machine,
                what,
                ..
            } => {
                if machine.is_empty() {
                    trash(ctx, local_trash, Path::new(path), what)?;
                } else {
                    remote_remove(ctx, remote_trash[machine], machine, path, what)?;
                }
            }
            DeleteStep::Prune { repo, machine, .. } => {
                if file_identity(ctx, machine, repo)?.is_some() {
                    if machine.is_empty() {
                        prune_local_worktrees(ctx, repo)?;
                    } else {
                        prune_remote_worktrees(ctx, machine, repo)?;
                    }
                }
            }
            DeleteStep::Github { name, .. } => {
                let out = ctx.runner.run(
                    &Cmd::new("gh", Duration::from_secs(60))
                        .args(["repo", "delete", name, "--yes"]),
                )?;
                if !out.success() {
                    bail!("could not delete GitHub repo {name}: {}", out.error_text());
                }
            }
        }
        plan.completed.insert(key);
        save_plan(&path, &plan)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::ok;
    use crate::scenarios::World;

    fn system_trash() -> &'static str {
        if cfg!(target_os = "macos") {
            "/usr/bin/trash"
        } else {
            "gio"
        }
    }

    fn trash_calls(world: &World) -> usize {
        world
            .runner
            .calls
            .borrow()
            .iter()
            .filter(|call| call.program == system_trash())
            .count()
    }

    fn mock_trash(world: &World) {
        let script = Trash::probe_script(std::env::consts::OS).unwrap();
        world.runner.on_fn(
            move |cmd| cmd.program == "sh" && cmd.args == ["-c", script],
            |_| Ok(ok(system_trash())),
        );
        world
            .runner
            .on_fn(|cmd| cmd.program == system_trash(), |_| Ok(ok("")));
        world.runner.on("gh api", ok("fixture-node-id"));
    }

    #[test]
    fn platform_trash_selection_and_missing_tool() {
        use crate::runner::{RealRunner, Runner};
        let dir = tempfile::tempdir().unwrap();
        let script = Trash::probe_script("linux").unwrap();
        let probe = || {
            RealRunner
                .run(
                    &Cmd::new("/bin/sh", Duration::from_secs(5))
                        .args(["-c", script])
                        .env("PATH", dir.path().to_string_lossy()),
                )
                .unwrap()
        };
        assert!(Trash::from_probe(&probe()).is_err());
        for (name, expected) in [("trash-put", Trash::Put), ("gio", Trash::Gio)] {
            let path = dir.path().join(name);
            std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            assert_eq!(Trash::from_probe(&probe()).unwrap(), expected);
        }
        assert_eq!(
            Trash::probe_script("macos").unwrap(),
            "test -x /usr/bin/trash && printf /usr/bin/trash"
        );
        assert_eq!(
            Trash::from_probe(&ok("/usr/bin/trash")).unwrap(),
            Trash::Mac
        );
    }

    #[test]
    fn missing_trash_refuses_before_any_effect() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        world.runner.on_fn(
            |cmd| cmd.program == "sh",
            |_| Ok(crate::runner::fake::fail(1, "")),
        );
        let error = delete(&world.ctx(), "demo", true, false).unwrap_err();
        assert!(error.to_string().contains("system trash is unavailable"));
        assert!(!deletion_path(&world.root, &project.slug).exists());
        assert_eq!(project.status(), Status::Active);
        assert!(project.project_md().exists());
        assert_eq!(world.runner.count("workspace close"), 0);
        assert_eq!(world.runner.count("gh repo delete"), 0);
        assert_eq!(trash_calls(&world), 0);
    }

    #[test]
    fn remote_trash_is_checked_before_effects_and_never_uses_rm() {
        for available in [false, true] {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            world.thread(&project, world.home.path(), |t| {
                t.machine = "box".into();
                t.worktree_path = "/home/agent/projects/lane".into();
            });
            mock_trash(&world);
            world.runner.on("machine list", ok("[]"));
            world.runner.on_fn(
                |cmd| cmd.program == "ssh",
                move |cmd| {
                    if cmd.display().contains("stat -c") {
                        Ok(ok("1:42"))
                    } else if cmd.display().contains("uname -s") {
                        Ok(if available {
                            ok("gio")
                        } else {
                            crate::runner::fake::fail(1, "")
                        })
                    } else {
                        Ok(ok(""))
                    }
                },
            );
            let result = delete(&world.ctx(), "demo", false, false);
            if available {
                result.unwrap();
                assert!(world.runner.calls.borrow().iter().any(|cmd| {
                    cmd.program == "ssh"
                        && cmd.display().contains("gio trash --")
                        && cmd.display().contains("/home/agent/projects/lane")
                }));
            } else {
                let error = result.unwrap_err();
                assert!(error.to_string().contains("on box"), "{error:#}");
                assert!(!deletion_path(&world.root, &project.slug).exists());
                assert_eq!(project.status(), Status::Active);
                assert_eq!(world.runner.count("workspace close"), 0);
                assert_eq!(trash_calls(&world), 0);
            }
            assert!(
                !world
                    .runner
                    .calls
                    .borrow()
                    .iter()
                    .any(|cmd| cmd.display().contains("rm -rf"))
            );
        }
    }

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
        mock_trash(&world);

        delete(&world.ctx(), "demo", false, false).unwrap();

        assert_eq!(world.runner.count("workspace close w1"), 1);
        assert!(world.runner.count("tab close") + world.runner.count("workspace close w2") >= 1);
        assert_eq!(trash_calls(&world), 1);
        assert_eq!(world.runner.count("gh repo delete"), 0);
    }

    #[test]
    fn delete_preview_persists_the_plan_without_destructive_effects() {
        let world = World::new();
        let project = world.project("demo", "a.sock");

        delete(&world.ctx(), "demo", false, true).unwrap();

        assert!(project.project_md().is_file());
        assert!(deletion_path(&world.root, &project.slug).exists());
        assert_eq!(trash_calls(&world), 0);
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
        mock_trash(&world);

        delete(&world.ctx(), &first.slug, false, false).unwrap();

        let calls = world.runner.calls.borrow();
        assert!(!calls.iter().any(|call| {
            call.program == system_trash()
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
        mock_trash(&world);

        assert!(delete(&world.ctx(), "demo", false, false).is_err());
        assert_eq!(trash_calls(&world), 0);
        assert!(!deletion_path(&world.root, &project.slug).exists());
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
        mock_trash(&world);

        delete(&world.ctx(), "first", true, false).unwrap();

        assert_eq!(world.runner.count("gh repo delete"), 0);
        assert!(world.runner.calls.borrow().iter().any(|call| {
            call.program == system_trash()
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
        mock_trash(&world);

        delete(&world.ctx(), "demo", true, false).unwrap();

        assert_eq!(world.runner.count("remote get-url origin"), 1);
        assert_eq!(world.runner.count("gh repo delete"), 1);
        let calls = world.runner.calls.borrow();
        let deletion = calls
            .iter()
            .find(|cmd| cmd.program == "gh" && cmd.args.first().is_some_and(|a| a == "repo"))
            .unwrap();
        assert_eq!(deletion.args, ["repo", "delete", "acme/demo", "--yes"]);
        let intent: DeleteIntent = toml::from_str(
            &std::fs::read_to_string(deletion_path(&world.root, &project.slug)).unwrap(),
        )
        .unwrap();
        let steps = intent.steps.as_ref().unwrap();
        let github = steps
            .iter()
            .position(|s| matches!(s, DeleteStep::Github { name, .. } if name == "acme/demo"))
            .unwrap();
        assert!(intent.completed.contains(&github.to_string()));
        assert_eq!(intent.completed.len(), steps.len());
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
            mock_trash(&world);

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
        mock_trash(&world);

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
        mock_trash(&world);

        delete(&world.ctx(), "demo", false, false).unwrap();

        let calls = world.runner.calls.borrow();
        assert!(calls.iter().any(|call| {
            call.program == system_trash()
                && call
                    .args
                    .iter()
                    .any(|arg| arg == &own.display().to_string())
        }));
        assert!(!calls.iter().any(|call| {
            call.program == system_trash()
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

    fn persisted(world: &World, slug: &str) -> DeleteIntent {
        toml::from_str(&std::fs::read_to_string(deletion_path(&world.root, slug)).unwrap()).unwrap()
    }

    fn trashed(world: &World, path: &Path) -> usize {
        world
            .runner
            .calls
            .borrow()
            .iter()
            .filter(|cmd| {
                cmd.program == system_trash()
                    && cmd.args.last() == Some(&path.to_string_lossy().into_owned())
            })
            .count()
    }

    #[test]
    fn preview_and_execution_use_identical_steps_and_leave_later_additions() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let repo = world.home.path().join("old-repo");
        std::fs::create_dir_all(&repo).unwrap();
        world.add_repo(&project, repo.to_str().unwrap());
        delete(&world.ctx(), "demo", false, true).unwrap();
        let preview = persisted(&world, "demo");
        let later = world.home.path().join("later-repo");
        let later_copy = world.root.join(".trash/demo-20260902T000000Z");
        let later_logs = world
            .root
            .join("pi/agent/sessions")
            .join(pi_session_name(&project.dir()));
        for path in [&later, &later_copy, &later_logs] {
            std::fs::create_dir_all(path).unwrap();
        }
        world.add_repo(&project, later.to_str().unwrap());
        mock_trash(&world);
        delete(&world.ctx(), "demo", false, false).unwrap();
        let executed = persisted(&world, "demo");
        assert_eq!(
            serde_json::to_value(&preview.steps).unwrap(),
            serde_json::to_value(&executed.steps).unwrap()
        );
        assert_eq!(preview.created, executed.created);
        assert_eq!(trashed(&world, &repo), 1);
        for path in [&later, &later_copy, &later_logs] {
            assert_eq!(trashed(&world, path), 0);
        }
        assert_eq!(executed.completed.len(), executed.steps.unwrap().len());
        assert_eq!(world.runner.count("gh repo delete"), 0);
    }

    #[test]
    fn new_nested_repo_refuses_the_old_parent_target() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let repo = world.home.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        world.add_repo(&project, repo.to_str().unwrap());
        delete(&world.ctx(), "demo", false, true).unwrap();
        world.add_repo(&project, repo.join("new-nested").to_str().unwrap());
        mock_trash(&world);
        let error = delete(&world.ctx(), "demo", false, false).unwrap_err();
        assert!(error.to_string().contains("new repo overlaps"));
        assert_eq!(trash_calls(&world), 0);
        assert_eq!(world.runner.count("workspace close"), 0);
    }

    #[test]
    fn newly_shared_target_is_refused_before_execution() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let repo = world.home.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        world.add_repo(&project, repo.to_str().unwrap());
        delete(&world.ctx(), "demo", false, true).unwrap();
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
        mock_trash(&world);
        assert!(
            delete(&world.ctx(), "demo", false, false)
                .unwrap_err()
                .to_string()
                .contains("now owned")
        );
        assert_eq!(trash_calls(&world), 0);
        assert_eq!(world.runner.count("workspace close"), 0);
    }

    #[test]
    fn ownership_is_reloaded_after_every_effect_and_failed_delete_stays_archived() {
        let mut world = World::new();
        world.runner = crate::runner::fake::FakeRunner::new();
        let project = world.project("demo", "a.sock");
        let repo = world.home.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        world.add_repo(&project, repo.to_str().unwrap());
        let root = world.root.clone();
        world.runner.on_fn(
            |cmd| cmd.display().contains("workspace close"),
            move |_| {
                crate::project::create(
                    &root,
                    "second",
                    "",
                    vec![Repo {
                        path: repo.display().to_string(),
                        ..Repo::default()
                    }],
                )?;
                Ok(ok(r#"{"result":{}}"#))
            },
        );
        mock_trash(&world);
        assert!(
            delete(&world.ctx(), "demo", false, false)
                .unwrap_err()
                .to_string()
                .contains("now owned")
        );
        assert_eq!(trash_calls(&world), 0);
        assert_eq!(project.status(), Status::Archived);
        assert!(persisted(&world, "demo").completed.contains("0"));
        // A start that passed its initial active check before deletion must
        // recheck after acquiring the allocation lock, even after a failed effect.
        assert!(thread::allocate(&project, |_| {}).is_err());
        assert!(
            set_status(&world.ctx(), "demo", Status::Active)
                .unwrap_err()
                .to_string()
                .contains("being deleted")
        );
    }

    #[test]
    fn resume_skips_completed_workspace_github_and_trash_steps() {
        use std::cell::Cell;
        use std::rc::Rc;
        let mut world = World::new();
        world.runner = crate::runner::fake::FakeRunner::new();
        world.runner.on("workspace close", ok(r#"{"result":{}}"#));
        let project = world.project("demo", "a.sock");
        let first = world.home.path().join("first");
        let second = world.home.path().join("second");
        for path in [&first, &second] {
            std::fs::create_dir_all(path).unwrap();
            world.add_repo(&project, path.to_str().unwrap());
        }
        let gone = Rc::new(Cell::new(false));
        let flag = gone.clone();
        world.runner.on_fn(
            |cmd| cmd.display().contains("remote get-url origin"),
            move |_| {
                assert!(!flag.get(), "must not rebuild persisted GitHub targets");
                Ok(ok("https://github.com/acme/demo.git"))
            },
        );
        let flag = gone.clone();
        world.runner.on_fn(
            |cmd| cmd.program == "gh" && cmd.args.first().is_some_and(|a| a == "api"),
            move |_| {
                assert!(!flag.get(), "must skip finished GitHub identity reads");
                Ok(ok("fixture-node-id"))
            },
        );
        world.runner.on("gh repo delete", ok(""));
        let crash = Rc::new(Cell::new(true));
        let flag = crash.clone();
        let target = second.to_string_lossy().into_owned();
        world.runner.on_fn(
            move |cmd| cmd.program == system_trash() && cmd.args.last() == Some(&target),
            move |_| {
                if flag.replace(false) {
                    anyhow::bail!("fixture crash");
                }
                Ok(ok(""))
            },
        );
        mock_trash(&world);
        assert!(
            delete(&world.ctx(), "demo", true, false)
                .unwrap_err()
                .to_string()
                .contains("fixture crash")
        );
        let interrupted = persisted(&world, "demo");
        assert!(interrupted.started);
        assert_eq!(project.status(), Status::Archived);
        assert_eq!(trashed(&world, &first), 1);
        // If execution rebuilt GitHub candidates it would now fail. Finished
        // GitHub steps must not even need to re-resolve their deleted identity.
        gone.set(true);
        delete(&world.ctx(), "demo", true, false).unwrap();
        assert_eq!(world.runner.count("workspace close"), 1);
        assert_eq!(world.runner.count("gh repo delete"), 1);
        assert_eq!(trashed(&world, &first), 1);
        assert_eq!(trashed(&world, &second), 2); // failed attempt, then success
        let resumed = persisted(&world, "demo");
        assert_eq!(resumed.completed.len(), resumed.steps.unwrap().len());
    }

    #[test]
    fn replaced_file_and_github_identities_are_refused() {
        use std::cell::Cell;
        use std::rc::Rc;
        for github in [false, true] {
            let world = World::new();
            let repo = world.home.path().join("repo");
            std::fs::create_dir_all(&repo).unwrap();
            crate::project::create(
                &world.root,
                "demo",
                "",
                vec![Repo {
                    path: repo.display().to_string(),
                    publish_url: Some("https://github.com/acme/demo.git".into()),
                    ..Repo::default()
                }],
            )
            .unwrap();
            let changed = Rc::new(Cell::new(false));
            let flag = changed.clone();
            world.runner.on_fn(
                |cmd| cmd.program == "gh" && cmd.args.first().is_some_and(|a| a == "api"),
                move |_| {
                    Ok(ok(if flag.get() {
                        "replacement-node"
                    } else {
                        "fixture-node-id"
                    }))
                },
            );
            mock_trash(&world);
            delete(&world.ctx(), "demo", github, true).unwrap();
            if github {
                changed.set(true);
            } else {
                std::fs::rename(&repo, world.home.path().join("original-repo")).unwrap();
                std::fs::create_dir_all(&repo).unwrap();
            }
            assert!(
                delete(&world.ctx(), "demo", github, false)
                    .unwrap_err()
                    .to_string()
                    .contains("identity changed")
            );
            assert_eq!(trash_calls(&world), 0);
            assert_eq!(world.runner.count("gh repo delete"), 0);
        }
    }

    #[test]
    fn changed_remote_identity_is_refused() {
        use std::cell::Cell;
        use std::rc::Rc;
        let world = World::new();
        let project = world.project("demo", "a.sock");
        world.thread(&project, world.home.path(), |t| {
            t.machine = "box".into();
            t.worktree_path = "/home/agent/lane".into();
        });
        let changed = Rc::new(Cell::new(false));
        let flag = changed.clone();
        world.runner.on("machine list", ok("[]"));
        world.runner.on_fn(
            |cmd| cmd.program == "ssh",
            move |cmd| {
                Ok(ok(if cmd.display().contains("stat -c") {
                    if flag.get() { "1:99" } else { "1:42" }
                } else {
                    "gio"
                }))
            },
        );
        delete(&world.ctx(), "demo", false, true).unwrap();
        changed.set(true);
        mock_trash(&world);
        assert!(
            delete(&world.ctx(), "demo", false, false)
                .unwrap_err()
                .to_string()
                .contains("identity changed")
        );
        assert_eq!(world.runner.count("workspace close"), 0);
        assert!(
            !world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|c| c.program == "ssh" && c.display().contains("gio trash"))
        );
    }

    #[test]
    fn github_choice_is_frozen_and_new_lane_bindings_refuse_stale_plan() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        delete(&world.ctx(), "demo", false, true).unwrap();
        assert!(
            delete(&world.ctx(), "demo", true, false)
                .unwrap_err()
                .to_string()
                .contains("same choice")
        );
        world.thread(&project, world.home.path(), |t| {
            t.worktree_path.clear();
        });
        mock_trash(&world);
        assert!(
            delete(&world.ctx(), "demo", false, false)
                .unwrap_err()
                .to_string()
                .contains("stale deletion plan")
        );
        assert_eq!(world.runner.count("workspace close"), 0);
        assert_eq!(trash_calls(&world), 0);
    }

    #[test]
    fn adopted_and_shared_workspaces_keep_other_tabs_and_adopted_files() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let adopted = world.home.path().join("adopted");
        std::fs::create_dir_all(&adopted).unwrap();
        world.thread(&project, &adopted, |t| {
            t.kind = thread::Kind::Adopted;
            t.workspace_id = "w1".into();
            t.tab_id = "w1:t2".into();
        });
        let other = world.project("second", "a.sock");
        other
            .update_coordinator(|c| {
                c.tab_id = "w1:t3".into();
                c.pane_id = "w1:p3".into();
            })
            .unwrap();
        mock_trash(&world);
        delete(&world.ctx(), "demo", false, false).unwrap();
        assert_eq!(world.runner.count("workspace close"), 0);
        assert_eq!(world.runner.count("tab close w1:t1"), 1);
        assert_eq!(world.runner.count("tab close w1:t2"), 1);
        assert_eq!(world.runner.count("tab close w1:t3"), 0);
        assert_eq!(trashed(&world, &adopted), 0);
    }

    #[test]
    fn ownership_reads_fail_closed_including_discovery_and_terminal_records() {
        for source in ["project", "thread", "coordinator", "discovery"] {
            let world = World::new();
            world.project("demo", "a.sock");
            let other = world.project("second", "b.sock");
            match source {
                "project" => std::fs::write(other.project_md(), "broken").unwrap(),
                "thread" => {
                    std::fs::create_dir_all(other.record_dir("threads")).unwrap();
                    std::fs::write(other.record_dir("threads").join("t-0001.toml"), "broken")
                        .unwrap();
                }
                "coordinator" => {
                    std::fs::write(other.record_file("coordinator.json"), "broken").unwrap()
                }
                _ => {
                    std::fs::create_dir_all(world.root.join("broken/PROJECT.md")).unwrap();
                }
            }
            mock_trash(&world);
            assert!(
                delete(&world.ctx(), "demo", false, false).is_err(),
                "{source}"
            );
            assert_eq!(trash_calls(&world), 0, "{source}");
            assert_eq!(world.runner.count("workspace close"), 0, "{source}");
        }
    }

    #[test]
    fn final_record_completion_survives_its_move_and_record_writers_serialize() {
        let mut world = World::new();
        world.runner = crate::runner::fake::FakeRunner::new();
        world.runner.on("workspace close", ok(r#"{"result":{}}"#));
        let project = world.project("demo", "a.sock");
        let record = project.dir();
        let locks = [
            project.record_file("lock"),
            project.record_file("coordinator.lock"),
        ];
        world.runner.on_fn(
            |cmd| cmd.program == system_trash(),
            move |_| {
                for path in &locks {
                    let file = std::fs::File::options().write(true).open(path)?;
                    assert!(file.try_lock().is_err());
                }
                std::fs::remove_dir_all(&record)?; // exclusively this temp fixture
                Ok(ok(""))
            },
        );
        mock_trash(&world);
        delete(&world.ctx(), "demo", false, false).unwrap();
        assert!(!project.dir().exists());
        let mut plan = persisted(&world, "demo");
        let last = (plan.steps.as_ref().unwrap().len() - 1).to_string();
        assert!(plan.completed.contains(&last));
        plan.completed.remove(&last); // simulate crash before final receipt
        save_plan(&deletion_path(&world.root, "demo"), &plan).unwrap();
        delete(&world.ctx(), "demo", false, false).unwrap();
        assert!(persisted(&world, "demo").completed.contains(&last));
        assert_eq!(trash_calls(&world), 1);
        assert_eq!(world.runner.count("workspace close"), 1);
        assert!(project.lock().is_err());
    }

    #[test]
    fn github_ownership_is_rechecked_and_unreadable_origins_fail_closed() {
        for source in ["publish", "origin", "unreadable"] {
            let world = World::new();
            let repo = world.home.path().join("repo");
            std::fs::create_dir_all(&repo).unwrap();
            crate::project::create(
                &world.root,
                "demo",
                "",
                vec![Repo {
                    path: repo.display().to_string(),
                    publish_url: Some("https://github.com/acme/demo.git".into()),
                    ..Repo::default()
                }],
            )
            .unwrap();
            mock_trash(&world);
            delete(&world.ctx(), "demo", true, true).unwrap();
            let other = world.home.path().join("other");
            std::fs::create_dir_all(&other).unwrap();
            crate::project::create(
                &world.root,
                "second",
                "",
                vec![Repo {
                    path: other.display().to_string(),
                    publish_url: Some(
                        if source == "publish" {
                            "https://github.com/acme/demo.git"
                        } else {
                            "https://github.com/acme/different.git"
                        }
                        .into(),
                    ),
                    ..Repo::default()
                }],
            )
            .unwrap();
            world.runner.on(
                "remote get-url origin",
                if source == "unreadable" {
                    crate::runner::fake::fail(1, "unreadable")
                } else {
                    ok("https://github.com/acme/demo.git")
                },
            );
            assert!(
                delete(&world.ctx(), "demo", true, false).is_err(),
                "{source}"
            );
            assert_eq!(world.runner.count("gh repo delete"), 0, "{source}");
            assert_eq!(trash_calls(&world), 0, "{source}");
        }
    }

    #[test]
    fn completed_journal_does_not_bind_a_new_project_with_the_same_slug() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        mock_trash(&world);
        delete(&world.ctx(), "demo", false, false).unwrap();
        let original = persisted(&world, "demo");
        // A Trash move retains the old inode; the new record must earn its own
        // preview and GitHub choice rather than inheriting old completions.
        std::fs::rename(project.dir(), world.home.path().join("trashed-record")).unwrap();
        crate::project::create(&world.root, "demo", "", vec![]).unwrap();
        delete(&world.ctx(), "demo", true, true).unwrap();
        let new = persisted(&world, "demo");
        assert!(new.github);
        assert!(!new.started);
        assert!(new.completed.is_empty());
        assert_ne!(
            serde_json::to_value(original.steps.unwrap().last()).unwrap(),
            serde_json::to_value(new.steps.unwrap().last()).unwrap()
        );
    }

    #[test]
    fn another_preview_can_refresh_unstarted_bindings_but_not_started_steps() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        delete(&world.ctx(), "demo", false, true).unwrap();
        let before = persisted(&world, "demo");
        world.thread(&project, world.home.path(), |t| {
            t.worktree_path.clear();
        });
        delete(&world.ctx(), "demo", false, true).unwrap();
        let mut refreshed = persisted(&world, "demo");
        assert_ne!(before.topology, refreshed.topology);
        assert!(refreshed.completed.is_empty());
        refreshed.started = true;
        save_plan(&deletion_path(&world.root, "demo"), &refreshed).unwrap();
        let later = world.home.path().join("later-repo");
        std::fs::create_dir_all(&later).unwrap();
        world.add_repo(&project, later.to_str().unwrap());
        delete(&world.ctx(), "demo", false, true).unwrap();
        assert_eq!(
            serde_json::to_value(persisted(&world, "demo").steps).unwrap(),
            serde_json::to_value(refreshed.steps).unwrap()
        );
    }

    #[test]
    fn corrupted_completion_or_final_target_refuses_all_effects() {
        for source in ["completion", "target"] {
            let world = World::new();
            world.project("demo", "a.sock");
            delete(&world.ctx(), "demo", false, true).unwrap();
            let mut plan = persisted(&world, "demo");
            if source == "completion" {
                plan.completed.insert("999".into());
            } else if let DeleteStep::Trash { path, .. } =
                plan.steps.as_mut().unwrap().last_mut().unwrap()
            {
                *path = world.home.path().display().to_string();
            }
            save_plan(&deletion_path(&world.root, "demo"), &plan).unwrap();
            mock_trash(&world);
            assert!(
                delete(&world.ctx(), "demo", false, false)
                    .unwrap_err()
                    .to_string()
                    .contains("invalid deletion plan")
            );
            assert_eq!(trash_calls(&world), 0);
            assert_eq!(world.runner.count("workspace close"), 0);
        }
    }

    #[test]
    fn replaced_kept_repo_is_not_pruned_after_worktree_removal() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let repo = world.home.path().join("repo");
        let lane = world.home.path().join("lane");
        for path in [&repo, &lane] {
            std::fs::create_dir_all(path).unwrap();
        }
        world.add_repo(&project, repo.to_str().unwrap());
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
        world.thread(&project, &lane, |t| {
            t.repo = repo.display().to_string();
        });
        let target = lane.display().to_string();
        let original = world.home.path().join("original-repo");
        world.runner.on_fn(
            move |cmd| cmd.program == system_trash() && cmd.args.last() == Some(&target),
            move |_| {
                std::fs::rename(&repo, &original)?;
                std::fs::create_dir_all(&repo)?;
                Ok(ok(""))
            },
        );
        mock_trash(&world);
        assert!(
            delete(&world.ctx(), "demo", false, false)
                .unwrap_err()
                .to_string()
                .contains("kept repo identity changed")
        );
        assert_eq!(trashed(&world, &lane), 1);
        assert_eq!(world.runner.count("worktree prune"), 0);
    }

    #[test]
    fn shared_session_name_collisions_are_kept_and_rechecked() {
        for added_after_preview in [false, true] {
            let world = World::new();
            let project = world.project("demo", "a.sock");
            let own_cwd = world.home.path().join("lane-a-b");
            let other_cwd = world.home.path().join("lane-a/b");
            for path in [&own_cwd, &other_cwd] {
                std::fs::create_dir_all(path).unwrap();
            }
            world.thread(&project, &own_cwd, |_| {});
            let logs = world
                .root
                .join("pi/agent/sessions")
                .join(pi_session_name(&own_cwd));
            assert_eq!(pi_session_name(&own_cwd), pi_session_name(&other_cwd));
            std::fs::create_dir_all(&logs).unwrap();
            let add_other = || {
                let other = world.project("second", "b.sock");
                world.thread(&other, &other_cwd, |_| {});
            };
            if !added_after_preview {
                add_other();
            }
            delete(&world.ctx(), "demo", false, true).unwrap();
            if added_after_preview {
                add_other();
            }
            mock_trash(&world);
            let result = delete(&world.ctx(), "demo", false, false);
            if added_after_preview {
                assert!(result.unwrap_err().to_string().contains("now owned"));
                assert_eq!(trash_calls(&world), 0);
            } else {
                result.unwrap();
            }
            assert_eq!(trashed(&world, &logs), 0);
        }
    }

    #[test]
    fn repo_used_only_as_another_projects_cwd_is_kept_in_preview() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let repo = world.home.path().join("repo");
        let other_cwd = repo.join("other-lane");
        std::fs::create_dir_all(&other_cwd).unwrap();
        world.add_repo(&project, repo.to_str().unwrap());
        let other = world.project("second", "b.sock");
        world.thread(&other, &other_cwd, |t| {
            t.repo.clear();
            t.worktree_path.clear();
        });
        delete(&world.ctx(), "demo", false, true).unwrap();
        assert!(!persisted(&world, "demo").steps.unwrap().iter().any(
            |s| matches!(s, DeleteStep::Trash { path, .. } if path == repo.to_str().unwrap())
        ));
        mock_trash(&world);
        delete(&world.ctx(), "demo", false, false).unwrap();
        assert_eq!(trashed(&world, &repo), 0);
    }

    #[test]
    fn journal_cannot_be_inside_a_planned_trash_target() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let repo = world.root.join(".deletions");
        std::fs::create_dir_all(&repo).unwrap();
        world.add_repo(&project, repo.to_str().unwrap());
        mock_trash(&world);
        assert!(
            delete(&world.ctx(), "demo", false, true)
                .unwrap_err()
                .to_string()
                .contains("remove the deletion journal")
        );
        assert_eq!(trash_calls(&world), 0);
        assert_eq!(world.runner.count("workspace close"), 0);
    }

    #[test]
    fn noncanonical_parent_target_cannot_remove_record_or_journal() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        world.add_repo(&project, world.root.join("demo/..").to_str().unwrap());
        mock_trash(&world);
        assert!(delete(&world.ctx(), "demo", false, true).is_err());
        assert_eq!(trash_calls(&world), 0);
        assert_eq!(world.runner.count("workspace close"), 0);
    }

    #[test]
    fn historical_intent_loads_but_cannot_rebuild_a_stale_plan() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let text = "slug = 'demo'\ngithub = false\ncreated = 'then'\nrepos = []\nworktrees = []\n";
        let historical: DeleteIntent = toml::from_str(text).unwrap();
        assert!(historical.completed.is_empty());
        assert!(historical.steps.is_none());
        assert!(!historical.started);
        std::fs::write(project.record_file("delete.toml"), text).unwrap();
        mock_trash(&world);
        assert!(
            delete(&world.ctx(), "demo", false, false)
                .unwrap_err()
                .to_string()
                .contains("historical deletion intent")
        );
        assert_eq!(trash_calls(&world), 0);
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
        mock_trash(&world);

        delete(&world.ctx(), "demo", true, false).unwrap();

        assert_eq!(world.runner.count("gh repo delete acme/demo --yes"), 1);
        let calls = world.runner.calls.borrow();
        for path in [repo, pi_session, claude_session] {
            assert!(calls.iter().any(|call| {
                call.program == system_trash()
                    && call
                        .args
                        .iter()
                        .any(|arg| arg == &path.display().to_string())
            }));
        }
    }
}
