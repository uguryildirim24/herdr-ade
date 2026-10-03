//! The text overview, and how commands without a slug find their project.

use std::io::{BufRead, Write as _};

use anyhow::{Result, bail};

use crate::paths::Ctx;
use crate::project::{self, Project, Status};
use crate::thread;

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

/// Technical read of the same current actions that feed Rundown and context.
pub(crate) fn run(ctx: &Ctx, slug: &str, include_history: bool) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let view =
        crate::project_view::View::load(ctx, &project, include_history.then_some(usize::MAX))?;
    let text = format!(
        "{} ({})\n{}",
        project.slug,
        project.status(),
        view.render(&[
            "Goal and what Rolf gets",
            "Current work",
            "Seals",
            "Pile reviews",
            "Open tasks"
        ])
    );
    crate::output::success(
        None,
        &serde_json::json!({"result":view.rundown()}),
        &text,
        "",
    )
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
        let view = crate::project_view::View::capture(
            &project,
            &project::Settings::default(),
            Some(vec![crate::threads::Row {
                thread: lane.clone(),
                group: crate::thread::Group::WaitingOnYou,
                note: "process gone".into(),
            }]),
            None,
        );
        let rendered = view.render(&[]);
        assert!(rendered.contains("Needs attention"), "{rendered}");
        assert!(rendered.contains("process disappeared"), "{rendered}");
        assert!(rendered.contains("ha thread retry demo"), "{rendered}");
        assert!(!rendered.contains("needs you"), "{rendered}");

        // Notices survive retries. A later live input wait must not reuse
        // the failed attempt's replacement advice.
        let mut retried = lane;
        retried.status = thread::Status::Open;
        retried.attempt += 1;
        retried.error.clear();
        let view = crate::project_view::View::capture(
            &project,
            &project::Settings::default(),
            Some(vec![crate::threads::Row {
                thread: retried,
                group: crate::thread::Group::WaitingOnYou,
                note: "blocked".into(),
            }]),
            None,
        );
        let rendered = view.render(&[]);
        assert!(rendered.contains("blocked"), "{rendered}");
        assert!(!rendered.contains("ha thread retry"), "{rendered}");
    }

    #[test]
    fn sealed_wait_survives_a_gone_process_and_rundown_includes_unlinked_work() {
        let fx = crate::testkit::fixture();
        let waiting = fx.thread("Browser access");
        fx.seal_waiting(&waiting, 1, 1, "Rolf, please log in to continue.");
        let working = fx.thread("Unlinked running work");
        let rows = vec![
            crate::threads::Row {
                thread: crate::thread::load(&fx.project, &waiting).unwrap(),
                group: crate::thread::Group::WaitingOnYou,
                note: "process gone: pane is absent".into(),
            },
            crate::threads::Row {
                thread: crate::thread::load(&fx.project, &working).unwrap(),
                group: crate::thread::Group::Working,
                note: "working".into(),
            },
        ];
        let view = crate::project_view::View::capture(
            &fx.project,
            &project::Settings::default(),
            Some(rows.clone()),
            None,
        );
        let text = view.render(&["Current work"]);
        assert!(text.contains("waiting seal retained"));
        assert!(text.contains("process absent"));
        assert!(text.contains("retry with a continuation reason"));
        assert!(!text.contains("needs you in pane"));
        let card = view.rundown();
        assert!(
            card["work"]
                .as_str()
                .unwrap()
                .starts_with("1 running · 1 waiting")
        );
        assert!(
            card["needs_you"]
                .as_str()
                .unwrap()
                .contains("please log in")
        );
        assert!(
            card["actions"]
                .to_string()
                .contains("Unlinked running work")
        );
        assert_eq!(crate::plan::counts(&fx.project).unwrap(), (0, 0));

        // Connection failure is not proof of death; neither retry nor a pane
        // direction is fabricated. Answering removes the personal request.
        let mut unknown = rows;
        unknown[0].note = "session unreachable".into();
        let view = crate::project_view::View::capture(
            &fx.project,
            &project::Settings::default(),
            Some(unknown.clone()),
            None,
        );
        assert!(view.render(&["Current work"]).contains("process unknown"));
        assert!(!view.render(&["Current work"]).contains("ha thread retry"));
        unknown[0].group = crate::thread::Group::Unknown;
        unknown[0].note = "agent state unknown; pane still exists".into();
        let view = crate::project_view::View::capture(
            &fx.project,
            &project::Settings::default(),
            Some(unknown.clone()),
            None,
        );
        let text = view.render(&["Current work"]);
        assert!(text.contains("process unknown"), "{text}");
        assert!(!text.contains("ha thread prompt"), "{text}");
        assert!(!text.contains("ha thread retry"), "{text}");
        assert!(view.work_summary().contains("1 unknown"));
        unknown[0].thread.machine = "oci".into();
        unknown[0].thread.last_observed = project::now();
        unknown[0].thread.observation_error = "connection lost".into();
        unknown[0].note = "no agent; last checked earlier; latest check failed".into();
        let view = crate::project_view::View::capture(
            &fx.project,
            &project::Settings::default(),
            Some(unknown.clone()),
            None,
        );
        assert!(view.render(&["Current work"]).contains("process unknown"));
        assert!(!view.render(&["Current work"]).contains("ha thread retry"));
        let event = crate::events::list(&fx.project).pop().unwrap();
        crate::thread::update(&fx.project, &waiting, |t| {
            t.answered_waiting_event = event.id.clone()
        })
        .unwrap();
        unknown[0].thread = crate::thread::load(&fx.project, &waiting).unwrap();
        let view = crate::project_view::View::capture(
            &fx.project,
            &project::Settings::default(),
            Some(unknown),
            None,
        );
        assert!(view.needs_you.is_empty());

        // A seal wins over stale process activity, but not over landing facts.
        fx.seal_done(&working, 1, 1, "change-sha", "Finished.");
        crate::thread::update(&fx.project, &working, |t| {
            t.merged_sha = "change-sha".into();
            t.historical_install_required = true;
        })
        .unwrap();
        let view = crate::project_view::View::capture(
            &fx.project,
            &project::Settings::default(),
            Some(vec![crate::threads::Row {
                thread: crate::thread::load(&fx.project, &working).unwrap(),
                group: crate::thread::Group::Working,
                note: "working".into(),
            }]),
            None,
        );
        let caption = view.rundown()["work"].as_str().unwrap().to_string();
        assert!(caption.starts_with("0 running · 0 waiting"));
        assert!(caption.contains("1 awaiting installation"));
        assert!(!caption.contains("awaiting review"));
        assert!(
            view.render(&["Current work"])
                .contains("merged; awaiting installation")
        );
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
