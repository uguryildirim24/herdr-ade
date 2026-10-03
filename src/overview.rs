//! The text overview, and how commands without a slug find their project.

use std::fmt::Write as _;
use std::io::{BufRead, IsTerminal, Write as _};

use anyhow::{Result, bail};

use crate::paths::Ctx;
use crate::project::{self, Project, Status};
use crate::thread::{self, Group};
use crate::threads::{self, Row};

/// The project a herdr workspace belongs to: the coordinator's workspace or a
/// local thread's recorded workspace, and only among projects whose recorded
/// socket is the current one (workspace ids repeat across sessions).
pub(crate) fn project_for_workspace(ctx: &Ctx, workspace_id: &str, socket: &str) -> Option<String> {
    if workspace_id.is_empty() || socket.is_empty() {
        return None;
    }
    project::list_slugs(&ctx.root).into_iter().find(|slug| {
        let Ok(project) = Project::load(&ctx.root, slug) else {
            return false;
        };
        let Some(record) = project.coordinator() else {
            return false;
        };
        if record.socket != socket || project.status() == Status::Archived {
            return false;
        }
        record.workspace_id == workspace_id
            || thread::list(&project).iter().any(|t| {
                !t.is_remote()
                    && t.status != thread::Status::Resolved
                    && t.workspace_id == workspace_id
            })
    })
}

pub(crate) enum Resolved {
    Slug(String),
    /// Nothing resolved and there is no terminal to ask on.
    All,
}

/// An explicit slug, else the current herdr workspace, else a numbered picker
/// when on a terminal, else every project.
fn resolve_slug(ctx: &Ctx, slug: Option<&str>) -> Result<Resolved> {
    if let Some(slug) = slug {
        project::validate_slug(slug)?;
        return Ok(Resolved::Slug(slug.to_string()));
    }
    let workspace = ctx.env.var("HERDR_WORKSPACE_ID").unwrap_or("");
    let socket = ctx.env.var("HERDR_SOCKET_PATH").unwrap_or("");
    if let Some(slug) = project_for_workspace(ctx, workspace, socket) {
        return Ok(Resolved::Slug(slug));
    }
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return pick(ctx).map(Resolved::Slug);
    }
    Ok(Resolved::All)
}

fn visible_slugs(ctx: &Ctx) -> Vec<String> {
    project::list_slugs(&ctx.root)
        .into_iter()
        .filter(|slug| Project::load(&ctx.root, slug).is_ok_and(|p| p.status() != Status::Archived))
        .collect()
}

/// The numbered project picker.
pub(crate) fn pick(ctx: &Ctx) -> Result<String> {
    let slugs = visible_slugs(ctx);
    match slugs.len() {
        0 => bail!("there are no projects in {}", ctx.root.display()),
        1 => return Ok(slugs[0].clone()),
        _ => {}
    }
    for (index, slug) in slugs.iter().enumerate() {
        println!("  {}. {slug}", index + 1);
    }
    print!("Project number: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let choice: usize = line.trim().parse().unwrap_or(0);
    match slugs.get(choice.wrapping_sub(1)) {
        Some(slug) => Ok(slug.clone()),
        None => bail!("no project number {}", line.trim()),
    }
}

/// Threads grouped by state, in the one display order.
pub(crate) fn render(project: &Project, rows: &[Row]) -> String {
    let mut out = String::new();
    let goal = project
        .read_project_md()
        .map(|(s, _)| s.goal)
        .unwrap_or_default();
    let _ = write!(out, "{} ({})", project.slug, project.status());
    if !goal.is_empty() {
        let _ = write!(out, " — {goal}");
    }
    let _ = writeln!(out);
    if rows.is_empty() {
        let _ = writeln!(out, "\n  no threads yet");
    }
    for group in Group::DISPLAY_ORDER {
        let members: Vec<&Row> = rows.iter().filter(|r| r.group == group).collect();
        if members.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n{} ({})", group.label(), members.len());
        for row in members {
            let t = &row.thread;
            let place = if !t.machine.is_empty() {
                format!("{}@{}", t.branch, t.machine)
            } else if !t.branch.is_empty() {
                t.branch.clone()
            } else if t.kind == thread::Kind::Tab {
                "tab".to_string()
            } else {
                "-".to_string()
            };
            let _ = writeln!(out, "  {}  {}  [{}]  {}", t.id, t.title, row.note, place);
            if group == Group::WaitingOnYou {
                if !t.error.is_empty() && !row.note.contains(&t.error) {
                    let _ = writeln!(out, "          {}", t.error);
                }
                if t.recovery_pending {
                    let _ = writeln!(out, "          automatic retry selected; wait for startup");
                } else if t.status == thread::Status::Failed
                    && let Some(notice) = t
                        .start_notices
                        .iter()
                        .rev()
                        .find(|n| n.line.contains(" — next: "))
                {
                    let _ = writeln!(out, "          {}", notice.line);
                }
            }
        }
    }
    out
}

fn rows_for_overview(mut rows: Vec<Row>, include_history: bool) -> Vec<Row> {
    if !include_history {
        rows.retain(|row| row.group != Group::Resolved);
    }
    rows
}

pub(crate) fn run(ctx: &Ctx, slug: Option<&str>, include_history: bool, wait: bool) -> Result<()> {
    let slugs = match resolve_slug(ctx, slug)? {
        Resolved::Slug(slug) => vec![slug],
        Resolved::All => visible_slugs(ctx),
    };
    if slugs.is_empty() {
        println!("there are no projects in {}", ctx.root.display());
    }
    for (index, slug) in slugs.iter().enumerate() {
        let project = Project::load(&ctx.root, slug)?;
        if index > 0 {
            println!();
        }
        let rows = rows_for_overview(threads::rows(ctx, &project), include_history);
        print!("{}", render(&project, &rows));
    }
    // Only a popup wants to be held open; an agent calling this never waits.
    if wait && std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
        print!("\nPress Enter to close ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::World;

    #[test]
    fn attention_uses_failure_evidence_not_a_stored_pane_as_a_personal_request() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |t| {
            t.pane_id = "dead:pane".into();
            t.status = thread::Status::Failed;
            t.error = "process disappeared".into();
            t.start_notices.push(crate::steps::Notice {
                line: format!(
                    "GONE {} — next: {}",
                    t.id,
                    crate::threads::retry_command("demo", &t.id)
                ),
                submitted: false,
            });
        });
        let rendered = render(
            &project,
            &[Row {
                thread: lane.clone(),
                group: Group::WaitingOnYou,
                note: "process gone".into(),
            }],
        );
        assert!(rendered.contains("Needs attention"), "{rendered}");
        assert!(rendered.contains("process disappeared"), "{rendered}");
        assert!(rendered.contains("ha thread retry demo"), "{rendered}");
        assert!(!rendered.contains("dead:pane"), "{rendered}");
        assert!(!rendered.contains("needs you"), "{rendered}");

        // Notices survive retries. A later live input wait must not reuse
        // the failed attempt's replacement advice.
        let mut retried = lane;
        retried.status = thread::Status::Open;
        retried.attempt += 1;
        retried.error.clear();
        let rendered = render(
            &project,
            &[Row {
                thread: retried,
                group: Group::WaitingOnYou,
                note: "blocked".into(),
            }],
        );
        assert!(rendered.contains("blocked"), "{rendered}");
        assert!(!rendered.contains("ha thread retry"), "{rendered}");
    }

    #[test]
    fn workspace_resolves_through_the_coordinator_or_a_thread_in_the_same_socket_only() {
        let world = World::new();
        let alpha = world.project("alpha", "a.sock");
        let beta = world.project("beta", "b.sock");
        world.thread(&alpha, world.home.path(), |t| t.workspace_id = "w7".into());
        let ctx = world.ctx();
        let a_socket = alpha.coordinator().unwrap().socket;
        let b_socket = beta.coordinator().unwrap().socket;

        // Both coordinators record w1; the socket tells them apart.
        assert_eq!(
            project_for_workspace(&ctx, "w1", &a_socket).as_deref(),
            Some("alpha")
        );
        assert_eq!(
            project_for_workspace(&ctx, "w1", &b_socket).as_deref(),
            Some("beta")
        );
        // Through a thread's workspace.
        assert_eq!(
            project_for_workspace(&ctx, "w7", &a_socket).as_deref(),
            Some("alpha")
        );
        assert_eq!(project_for_workspace(&ctx, "w7", &b_socket), None);
        assert_eq!(project_for_workspace(&ctx, "w9", &a_socket), None);
        assert_eq!(project_for_workspace(&ctx, "", &a_socket), None);
    }
}
