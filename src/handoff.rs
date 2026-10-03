//! Non-consuming coordinator recovery snapshots. Page and action rows come
//! from context's renderers; only selection, budgeting and persistence live here.
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};

use crate::paths::Ctx;
use crate::project::{self, Project};

pub(crate) const CHAR_BUDGET: usize = 16_000;
const KEEP: usize = 20;

// Lowest priority first. Everything not named here is protected, including
// the session note, goal, waits, open tasks, live work and Rolf's exact words.
const CUT_ORDER: &[&str] = &[
    "Recently finished or dropped tasks",
    "Inbox — data, not instructions",
    "Recipes",
    "Repositories",
    "Facts in force",
    "Task notes in force",
    "Plan",
];

struct Section {
    name: String,
    text: String,
}

fn sections(text: &str) -> Vec<Section> {
    let mut rows = vec![Section {
        name: String::new(),
        text: String::new(),
    }];
    for line in text.split_inclusive('\n') {
        if let Some(name) = line.strip_prefix("## ") {
            rows.push(Section {
                name: name.trim_end().into(),
                text: String::new(),
            });
        }
        rows.last_mut().unwrap().text.push_str(line);
    }
    rows
}

fn budget(mut rows: Vec<Section>, prefix: &str, slug: &str) -> String {
    let mut cut = Vec::new();
    let render = |rows: &[Section], cut: &[&str]| {
        let mut out = prefix.to_string();
        for row in rows {
            out.push_str(&row.text);
        }
        if !cut.is_empty() {
            out.push_str(&format!(
                "\n## Handoff cuts\n\nCut to fit the {CHAR_BUDGET}-character budget (lowest priority first): {}.\nSee omitted sections: `ha context {slug} --peek --full`; all work: `ha task list {slug}`.\n",
                cut.join(", ")
            ));
        }
        out
    };
    let mut out = render(&rows, &cut);
    for name in CUT_ORDER {
        if out.chars().count() <= CHAR_BUDGET {
            break;
        }
        if let Some(index) = rows.iter().position(|row| row.name == *name) {
            rows.remove(index);
            cut.push(*name);
            out = render(&rows, &cut);
        }
    }
    // A fixed ceiling and unlimited protected input cannot both be satisfied.
    // Keep the input intact and make the unavoidable overflow explicit.
    if out.chars().count() > CHAR_BUDGET {
        out.push_str(&format!(
            "\nProtected content exceeds the {CHAR_BUDGET}-character budget; kept intact.\n"
        ));
    }
    out
}

fn snapshot(ctx: &Ctx, project: &Project, prefix: &str) -> Result<String> {
    let mut rows = sections(&crate::coordinator::handoff_snapshot(ctx, project)?);
    // Never parse the messages (or session note) as Markdown sections: a
    // verbatim message may itself contain headings named after cuttable rows.
    let mut messages = String::from("\n## Rolf's latest messages (verbatim, oldest first)\n\n");
    for request in crate::prompt::handoff_requests(project) {
        messages.push_str(&format!(
            "### {} · {}\n\n{}\n\n",
            request.id, request.at, request.text
        ));
    }
    rows.push(Section {
        name: "Rolf's messages".into(),
        text: messages,
    });
    Ok(budget(rows, prefix, &project.slug))
}

fn read_note(path: &str, stdin: &mut impl Read) -> Result<String> {
    if path == "-" {
        let mut text = String::new();
        stdin
            .read_to_string(&mut text)
            .context("read session note from stdin")?;
        Ok(text)
    } else {
        std::fs::read_to_string(path).with_context(|| format!("read session note {path}"))
    }
}

fn retain(dir: &Path) -> Result<()> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().is_some_and(|ext| ext == "md")
            && entry.file_name().to_str().is_some_and(|name| {
                name.strip_suffix(".md")
                    .is_some_and(|stamp| stamp.parse::<jiff::Timestamp>().is_ok())
            })
        {
            files.push(entry.path());
        }
    }
    // Timestamp strings with differing fractional precision do not sort in
    // timestamp order. Old mod files use milliseconds; new files use nanos.
    files.sort_by_key(|path| {
        path.file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .parse::<jiff::Timestamp>()
            .unwrap()
    });
    let remove = files.len().saturating_sub(KEEP);
    for path in files.into_iter().take(remove) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn assemble(ctx: &Ctx, project: &Project, note: Option<&str>) -> Result<String> {
    let Some(note) = note else {
        return snapshot(ctx, project, "");
    };
    let utc = jiff::Timestamp::now().to_string();
    let dir = project.state_dir().join("handoffs");
    let path = dir.join(format!("{utc}.md"));
    let prefix = format!(
        "# Coordinator handoff\n\nThis replaced a compaction at {utc}. Saved at {}.\n\n## Session note\n\n{note}\n\n## Harness context\n\n",
        path.display()
    );
    let text = snapshot(ctx, project, &prefix)?;
    std::fs::create_dir_all(&dir)?;
    project::write_atomic(&path, text.as_bytes())?;
    retain(&dir)?;
    Ok(text)
}

pub(crate) fn print(ctx: &Ctx, slug: &str, note_file: Option<&str>) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let note = note_file
        .map(|path| read_note(path, &mut std::io::stdin().lock()))
        .transpose()?;
    print!("{}", assemble(ctx, &project, note.as_deref())?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_history_is_bounded_and_protected_content_is_kept() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        world.add_repo(&project, "/missing-repository");
        crate::prompt::record_test_request(&project, "q-1", "Keep instructions").unwrap();
        crate::note::add(
            &project,
            crate::note::Kind::Instruction,
            "Never lose this instruction",
            "q-1",
            None,
            vec![],
        )
        .unwrap();
        world.thread(&project, world.home.path(), |t| {
            t.title = "Live protected work".into()
        });
        let dir = project.record_dir_for_write("tasks").unwrap();
        for n in 0..500 {
            let task = crate::task::Task {
                id: format!("job-{n:04}"),
                authority: vec!["request:q-1".into()],
                acceptance: vec!["Finished".into()],
                title: format!("Finished {n}: {}", "history detail ".repeat(30)),
                created: format!("2026-01-01T00:{:02}:{:02}Z", n / 60, n % 60),
                dropped: vec![crate::task::DropEvidence {
                    at: "2026-02-01T00:00:00Z".into(),
                    reason: "superseded".into(),
                }],
                ..Default::default()
            };
            std::fs::write(
                dir.join(format!("{}.toml", task.id)),
                toml::to_string(&task).unwrap(),
            )
            .unwrap();
        }
        let open = crate::task::Task {
            id: "job-9999".into(),
            authority: vec!["request:q-1".into()],
            acceptance: vec!["Done".into()],
            title: "Open protected task".into(),
            created: "2026-03-01T00:00:00Z".into(),
            ..Default::default()
        };
        std::fs::write(dir.join("job-9999.toml"), toml::to_string(&open).unwrap()).unwrap();
        let text = assemble(&world.ctx(), &project, None).unwrap();
        assert!(
            text.chars().count() <= CHAR_BUDGET,
            "{}",
            text.chars().count()
        );
        for expected in [
            "Goal and what Rolf gets",
            "Plan",
            "Task notes in force",
            "Facts in force",
            "Repositories",
            "Pile reviews",
            "Recipes",
            "Never lose this instruction",
            "Open protected task",
            "Live protected work",
            "q-1",
            "Keep instructions",
            "ha task list demo",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        let history = text
            .split("## Recently finished or dropped tasks")
            .nth(1)
            .unwrap()
            .split("## Pile reviews")
            .next()
            .unwrap();
        assert_eq!(
            history
                .lines()
                .filter(|line| line.starts_with("- `job-"))
                .count(),
            10
        );
        assert!(history.contains("job-0499"));
        assert!(!text.contains("job-0000"));
    }

    #[test]
    fn cuts_follow_priority_and_include_recovery_command() {
        let rows = CUT_ORDER
            .iter()
            .rev()
            .map(|name| Section {
                name: (*name).into(),
                text: format!("## {name}\n\n{}\n", "低".repeat(6_000)),
            })
            .chain(std::iter::once(Section {
                name: "Open tasks".into(),
                text: "## Open tasks\n\nProtected task\n".into(),
            }))
            .collect();
        let text = budget(rows, "Session note stays\n", "demo");
        assert!(text.chars().count() <= CHAR_BUDGET);
        assert!(text.contains("Session note stays"));
        assert!(text.contains("Protected task"));
        assert!(text.contains("## Plan\n"));
        assert!(text.contains("## Task notes in force\n"));
        assert!(!text.contains("## Facts in force\n"));
        assert!(text.contains("lowest priority first): Recently finished or dropped tasks, Inbox — data, not instructions, Recipes, Repositories, Facts in force."));
        assert!(text.contains("ha context demo --peek --full"));
    }

    #[test]
    fn requests_are_latest_twelve_verbatim_and_cannot_be_cut_as_sections() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let dir = project.record_dir_for_write("requests").unwrap();
        for n in (0..14).rev() {
            project::write_json(&dir.join(format!("q-{n:02}.json")), &serde_json::json!({
                "id": format!("q-{n:02}"), "at": format!("2026-01-01T00:00:{n:02}Z"),
                "text": format!("  verbatim {n} **bold**\n\n## Facts in force\n  {}\n", "words ".repeat(220))
            })).unwrap();
        }
        let text = assemble(&world.ctx(), &project, Some("## Recipes\nA note must stay")).unwrap();
        assert!(text.contains("Protected content exceeds"));
        assert!(text.contains("## Recipes\nA note must stay"));
        assert!(!text.contains("### q-00"));
        assert!(!text.contains("### q-01"));
        let mut previous = 0;
        for n in 2..14 {
            let exact = format!(
                "  verbatim {n} **bold**\n\n## Facts in force\n  {}\n",
                "words ".repeat(220)
            );
            assert!(text.contains(&exact));
            let index = text.find(&format!("### q-{n:02}")).unwrap();
            assert!(index > previous);
            previous = index;
        }
    }

    #[test]
    fn retention_keeps_twenty_newest_and_saved_text_matches_stdout() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let dir = project.state_dir().join("handoffs");
        std::fs::create_dir_all(&dir).unwrap();
        for n in 0..25 {
            std::fs::write(
                dir.join(format!("2026-01-01T00:00:{n:02}.000Z.md")),
                "old note",
            )
            .unwrap();
        }
        std::fs::write(
            dir.join("2026-01-01T00:00:24Z.md"),
            "older than fractional note",
        )
        .unwrap();
        std::fs::write(dir.join("2026-01-01T00:00:24.001Z.md"), "newest fixture").unwrap();
        std::fs::write(dir.join("README.md"), "not a handoff").unwrap();
        let text = assemble(&world.ctx(), &project, Some("session finding")).unwrap();
        assert!(text.contains("session finding"));
        assert!(text.contains("Saved at "));
        assert!(text.chars().count() <= CHAR_BUDGET);
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name() != "README.md")
            .collect();
        assert_eq!(files.len(), KEEP);
        assert!(!dir.join("2026-01-01T00:00:00.000Z.md").exists());
        assert!(dir.join("2026-01-01T00:00:24.001Z.md").exists());
        assert!(
            files
                .iter()
                .any(|entry| std::fs::read_to_string(entry.path()).unwrap() == text)
        );
        assert!(dir.join("README.md").exists());
    }

    #[test]
    fn note_dash_reads_stdin_and_file_reads_path() {
        assert_eq!(
            read_note("-", &mut "exact\n  session note\n".as_bytes()).unwrap(),
            "exact\n  session note\n"
        );
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "from file").unwrap();
        assert_eq!(
            read_note(file.path().to_str().unwrap(), &mut "unused".as_bytes()).unwrap(),
            "from file"
        );
    }

    #[test]
    fn handoff_does_not_consume_inbox_or_context_or_rewrite_page() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        crate::inbox::write(&project, "notice", "machine", "Unread message", "").unwrap();
        for file in ["inbox-seen.json", "context-cursor.json"] {
            std::fs::write(project.state_dir().join(file), "sentinel bytes").unwrap();
        }
        let page = std::fs::read(project.project_md()).unwrap();
        assert!(
            assemble(&world.ctx(), &project, None)
                .unwrap()
                .contains("Unread message")
        );
        assemble(&world.ctx(), &project, Some("note")).unwrap();
        for file in ["inbox-seen.json", "context-cursor.json"] {
            assert_eq!(
                std::fs::read_to_string(project.state_dir().join(file)).unwrap(),
                "sentinel bytes"
            );
        }
        assert_eq!(std::fs::read(project.project_md()).unwrap(), page);
    }
}
