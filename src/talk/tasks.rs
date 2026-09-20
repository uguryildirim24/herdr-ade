//! Rolf's task list from `TASKS.md` (LEAN U5). Read-only: adding, finishing
//! and cancelling stay his words in the chat. The screen shows each list, one
//! line per open task with its owner, and for a delegated task the state of
//! the thread doing it.
use std::io::ErrorKind;

use crate::project::Project;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Task {
    pub title: String,
    pub owner: String,
    /// The thread id after `→`, when this task is delegated.
    pub thread: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct List {
    pub heading: String,
    pub tasks: Vec<Task>,
}

/// Parse the `## <list>` headings and `- [ ] <title> (<owner>)` lines.
/// Anything else is not a task; a task before its first heading is skipped.
pub fn parse(text: &str) -> Vec<List> {
    let mut lists: Vec<List> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(heading) = line.strip_prefix("## ") {
            let heading = heading.trim();
            if !heading.is_empty() {
                lists.push(List {
                    heading: heading.to_string(),
                    tasks: Vec::new(),
                });
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("- [ ] ") else {
            continue;
        };
        let Some(list) = lists.last_mut() else {
            continue;
        };
        let (title, owner) = split_owner(rest.trim());
        if title.is_empty() {
            continue;
        }
        let (owner, thread) = delegate(owner);
        list.tasks.push(Task {
            title: title.to_string(),
            owner,
            thread,
        });
    }
    lists
}

fn split_owner(text: &str) -> (&str, &str) {
    if text.ends_with(')')
        && let Some(open) = text.rfind(" (")
    {
        return (text[..open].trim(), text[open + 2..text.len() - 1].trim());
    }
    (text, "agent")
}

fn delegate(owner: &str) -> (String, Option<String>) {
    if let Some((who, thread)) = owner.split_once('→') {
        let owner = who.trim();
        let owner = if owner.is_empty() { "agent" } else { owner };
        let thread = thread.trim();
        let thread = (!thread.is_empty()).then(|| thread.to_string());
        return (owner.to_string(), thread);
    }
    (owner.to_string(), None)
}

/// Read the same file the coordinator's digest reads. `true` when the file is
/// present but unreadable: unknown is not empty.
pub fn load(project: &Project) -> (Vec<List>, bool) {
    match std::fs::read_to_string(project.dir().join("TASKS.md")) {
        Ok(text) => (parse(&text), false),
        Err(error) if error.kind() == ErrorKind::NotFound => (Vec::new(), false),
        Err(_) => (Vec::new(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lists_owners_and_delegated_threads() {
        let text = "\
# Tasks

## Backlog
- [ ] Ask Rolf about the cost (me)
- [ ] Write the screen (agent)
- [ ] Add the task list (agent → t-0007)
- [x] already done (me)

## Lean harness
- [ ] Cut the memory (Rolf)
not a task
";
        let lists = parse(text);
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].heading, "Backlog");
        assert_eq!(lists[0].tasks.len(), 3);
        assert_eq!(lists[0].tasks[0].owner, "me");
        assert_eq!(lists[0].tasks[0].thread, None);
        assert_eq!(lists[0].tasks[1].owner, "agent");
        assert_eq!(lists[0].tasks[2].owner, "agent");
        assert_eq!(lists[0].tasks[2].thread.as_deref(), Some("t-0007"));
        assert_eq!(lists[1].heading, "Lean harness");
        assert_eq!(lists[1].tasks[0].owner, "Rolf");
    }
}
