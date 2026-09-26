//! Structural marker for a designed command refusal.
//!
//! A refusal is an expected outcome chosen by the harness: a safety/authority
//! guard, or a completed health report refusing to declare the system healthy.
//! It still exits unsuccessfully and keeps its message, but it is not evidence
//! that the harness failed. Unexpected I/O, subprocess and invariant errors use
//! ordinary `anyhow::Error` values.

use std::fmt;

#[derive(Debug)]
pub(crate) struct DesignedRefusal {
    message: String,
    next: String,
}

impl fmt::Display for DesignedRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DesignedRefusal {}

/// `next` is required at every designed refusal site. Use the exact command
/// that clears the guard, or describe the event the harness is waiting for.
pub(crate) fn error(message: impl Into<String>, next: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(DesignedRefusal {
        message: message.into(),
        next: next.into(),
    })
}

pub(crate) fn next(error: &anyhow::Error) -> Option<&str> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<DesignedRefusal>()
            .map(|refusal| refusal.next.as_str())
    })
}

pub(crate) fn next_line(next: &str) -> String {
    format!("next: {next}")
}

/// Context may wrap a refusal on its way to the CLI, so inspect the full chain.
pub(crate) fn is(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<DesignedRefusal>().is_some())
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    #[test]
    fn the_marker_survives_context_without_matching_message_text() {
        let refusal = Err::<(), _>(super::error("same text", "ha task list"))
            .context("outer context")
            .unwrap_err();
        let ordinary = anyhow::anyhow!("same text").context("outer context");
        assert!(super::is(&refusal));
        assert!(!super::is(&ordinary));
        assert_eq!(super::next(&refusal), Some("ha task list"));
    }

    #[test]
    fn five_common_refusals_render_their_next_line() {
        let cases = [
            (
                "task_title: a title is required",
                "ha task add demo --title \"work\" --request request:q-1 --acceptance \"done\"",
            ),
            (
                "task_unknown: no task `job-1` in `demo`",
                "ha task list demo",
            ),
            (
                "repo_ambiguous: choose /one or /two",
                "ha thread start demo --repo /one --job job-1 --task-file brief.md",
            ),
            (
                "recipe_authority: quote Rolf",
                "ha thread start demo --request q-1 --acceptance \"done\" --task-file brief.md",
            ),
            (
                "report_missing: /work/report.md",
                "ha done --report /work/report.md --sha abc123",
            ),
        ];
        for (message, command) in cases {
            let error = super::error(message, command);
            assert_eq!(
                super::next_line(super::next(&error).unwrap()),
                format!("next: {command}")
            );
        }
    }
}
