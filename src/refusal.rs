//! Structural marker for a designed command refusal.
//!
//! A refusal is an expected outcome chosen by the harness: a safety/authority
//! guard, or a completed health report refusing to declare the system healthy.
//! It still exits unsuccessfully and keeps its message, but it is not evidence
//! that the harness failed. Unexpected I/O, subprocess and invariant errors use
//! ordinary `anyhow::Error` values and remain eligible for the failure ledger.

use std::fmt;

#[derive(Debug)]
pub(crate) struct DesignedRefusal {
    message: String,
}

impl fmt::Display for DesignedRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DesignedRefusal {}

pub(crate) fn error(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(DesignedRefusal {
        message: message.into(),
    })
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
        let refusal = Err::<(), _>(super::error("same text"))
            .context("outer context")
            .unwrap_err();
        let ordinary = anyhow::anyhow!("same text").context("outer context");
        assert!(super::is(&refusal));
        assert!(!super::is(&ordinary));
    }
}
