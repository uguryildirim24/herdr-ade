//! The D15 adapter row for kind `pi` (SPEC-pi v2 §3.6).
//!
//! Same channel as every other kind: after `agent start` reports ready, ADE
//! types exactly one line, `ha skill <role>`. No hook injection. The guard
//! (SPEC-pi v2 §3.7) is what makes a provider failure `blocked`, not idle.

/// Everything A2's adapter table needs for kind `pi`.
#[derive(Debug, Clone, PartialEq)]
pub struct AdapterRow {
    pub kind: &'static str,
    pub start: &'static str,
    pub receipt: &'static str,
    /// Traps that show before the herdr extension reports (screen detection).
    pub traps_pre_ready: &'static str,
    /// Traps after the extension holds authority.
    pub traps_post_ready: &'static str,
    pub required: &'static str,
    pub status: &'static str,
    /// `ha context` label until the D15 row passes (SPEC-ADE D15).
    pub capability: &'static str,
    pub prime_line: &'static str,
}

/// The exact row from SPEC-pi v2 §3.6.
pub fn adapter_row() -> AdapterRow {
    AdapterRow {
        kind: "pi",
        start: "--kind pi -- <role args>",
        receipt: "`ha skill` / `ha context`",
        traps_pre_ready: concat!(
            "Pre-ready: `pi` not the wrapper (process-info); the trust question ",
            "(herdr reads it as ready/idle; prevented by `defaultProjectTrust: \"never\"`; ",
            "if seen, never type into it; T10 must show no dialog); \"session folder is ",
            "missing, Continue / Cancel\" on resume (restore falls back to `$HOME` when the ",
            "saved cwd is gone); \"No models available\" under a wrong dir. Missing login is ",
            "not pre-ready: refused at start by `pi auth check`. Missing `fd`/`rg` download ",
            "silently into `<agentDir>/bin` (not a prompt). There is no unhandled ",
            "tool-confirm."
        ),
        traps_post_ready: concat!(
            "Post-ready: provider error or rate limit, reported `blocked` / `WAITING` by ",
            "the guard; unsubmitted composer."
        ),
        required: "yes, if the coding lane moves",
        status: "untested",
        capability: "unqualified",
        prime_line: "ha skill <role>",
    }
}

/// One line for the coordinator skill and `ha context` headers.
pub fn prime_line(role: &str) -> String {
    format!("ha skill {role}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_names_the_spec_traps() {
        let row = adapter_row();
        assert_eq!(row.kind, "pi");
        assert_eq!(row.status, "untested");
        assert_eq!(row.capability, "unqualified");
        assert!(row.traps_pre_ready.contains("trust question"));
        assert!(row.traps_pre_ready.contains("defaultProjectTrust"));
        assert!(row.traps_pre_ready.contains("pi auth check"));
        assert!(row.traps_post_ready.contains("guard"));
        assert_eq!(prime_line("lane"), "ha skill lane");
    }
}
