//! Per-kind capability labels shown by the project UI.

/// Capability labels are evidence-based. Installation alone never promotes a
/// row; acceptance writes the project-local qualification marker after its
/// installed-CLI test passes.
pub(crate) fn capability_label(project: &crate::project::Project, kind: &str) -> &'static str {
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
