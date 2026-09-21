//! Per-kind launch, receipt, trap, and correction capability facts.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CorrectionAdapter {
    ClaudeStop,
    CursorFollowup,
    CodexStop,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Adapter {
    pub(crate) kind: &'static str,
    pub(crate) receipt: &'static str,
    pub(crate) positive_resend_evidence: &'static str,
    pub(crate) required: &'static str,
    pub(crate) correction: CorrectionAdapter,
    /// What happens to the coordinator's native chat (the `talk` header).
    pub(crate) chat: &'static str,
    /// The `talk` header's surface clause for a non-Claude coordinator.
    pub(crate) surface: &'static str,
}

const ADAPTERS: [Adapter; 8] = [
    Adapter {
        kind: "claude",
        receipt: "ha skill or ha context",
        positive_resend_evidence: "explicit rejected submission only",
        required: "coordinator and lane",
        correction: CorrectionAdapter::ClaudeStop,
        chat: "checked after display; native pane shows the first version",
        surface: "surface mediated, native checked after display",
    },
    Adapter {
        kind: "cursor",
        receipt: "ha skill",
        positive_resend_evidence: "observed unsubmitted pasted block for the same attempt",
        required: "lane and reviewer",
        correction: CorrectionAdapter::CursorFollowup,
        chat: "not checked",
        surface: "chat: shown only through say and ask",
    },
    Adapter {
        kind: "codex",
        receipt: "ha skill",
        positive_resend_evidence: "explicit rejected submission only",
        required: "no",
        correction: CorrectionAdapter::CodexStop,
        chat: "not checked",
        surface: "chat: shown only through say and ask",
    },
    Adapter {
        kind: "opencode",
        receipt: "ha skill",
        positive_resend_evidence: "none",
        required: "no",
        correction: CorrectionAdapter::None,
        chat: "not checked",
        surface: "chat: shown only through say and ask",
    },
    Adapter {
        kind: "agy",
        receipt: "ha skill",
        positive_resend_evidence: "none",
        required: "no",
        correction: CorrectionAdapter::None,
        chat: "not checked",
        surface: "chat: shown only through say and ask",
    },
    Adapter {
        kind: "chatgpt",
        receipt: "artifact event from passive adoption",
        positive_resend_evidence: "none",
        required: "Pro attack turns",
        correction: CorrectionAdapter::None,
        chat: "not checked",
        surface: "chat: shown only through say and ask",
    },
    Adapter {
        kind: "dsh",
        receipt: "ha skill",
        positive_resend_evidence: "none",
        required: "no",
        correction: CorrectionAdapter::None,
        chat: "not checked",
        surface: "chat: shown only through say and ask",
    },
    Adapter {
        kind: "pi",
        receipt: "ha skill or ha context",
        positive_resend_evidence: "none",
        required: "yes, if the coding lane moves",
        correction: CorrectionAdapter::None,
        chat: "not checked",
        surface: "chat: shown only through say and ask",
    },
];

pub(crate) fn get(kind: &str) -> Option<&'static Adapter> {
    ADAPTERS.iter().find(|adapter| adapter.kind == kind)
}

/// Capability labels are evidence-based. Installation alone never promotes a
/// row; acceptance writes the project-local qualification marker after its
/// installed-CLI test passes.
pub(crate) fn capability_label(project: &crate::project::Project, kind: &str) -> &'static str {
    let _adapter = get(kind);
    let qualified = project
        .state_dir()
        .join("capabilities")
        .join(format!("{kind}.qualified"))
        .is_file();
    match (kind, qualified) {
        ("claude", true) => {
            "surface mediated; native chat checked after display; native pane shows the first version"
        }
        ("cursor", true) => "surface mediated; native chat gets a follow-up after display",
        ("codex", true) => "surface mediated; native chat checked after display",
        _ => "capability: unqualified; chat: shown only through say and ask",
    }
}
