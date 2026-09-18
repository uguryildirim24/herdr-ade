//! The `kind = "pi"` rows for the roles table and the Jev picker
//! (SPEC-pi v2 §3.4, §3.5).
//!
//! One pi row replaces its native twin in exactly the list the twin was in.
//! Cursor is not replaced: `cursor_grok_xhigh` stays native while the port
//! runs, then it is retired; no `pi_cursor_*` row exists. Grok under pi is
//! withheld until Rolf says (decision 2026-09-18 19:25, item 6).

use anyhow::Result;

use super::launch;

/// One ready-made recipe row. Fields line up with Jev v2's `Recipe` plus
/// `model_family` and the D17 `plain` phrase; A4 maps them into its table.
#[derive(Debug, Clone, PartialEq)]
pub struct PiRecipe {
    /// Jev recipe id.
    pub id: &'static str,
    /// D2 kind; always `pi`.
    pub kind: &'static str,
    /// The Jev `provider` column; must equal `--provider` in `args`.
    pub provider: &'static str,
    /// The pair filter compares this, not a CLI-shaped model string.
    pub model_family: &'static str,
    /// The argv after `--`.
    pub args: Vec<String>,
    /// Always empty for pi: the wrapper supplies the folder, no secret.
    pub env: Vec<String>,
    pub ready_timeout_ms: u64,
    /// The Jev v2 recipe switch; disabled rows wait for Rolf.
    pub enabled: bool,
    /// May a start-time `allowed` list carry it?
    pub start_time_allowed: bool,
    /// The D17 `plain` phrase: a noun phrase A4's reason templates slot in
    /// ("<job> runs on <plain>"), so no product name and no full sentence.
    /// A row that replaces a native twin keeps the twin's phrase.
    pub plain: &'static str,
}

impl PiRecipe {
    pub fn thinking(&self) -> String {
        launch::flag_value(&self.args, "--thinking").unwrap_or_default()
    }

    /// `--provider`, `--model`, `--thinking`, `--no-skills` and nothing else.
    pub fn validate(&self) -> Result<()> {
        launch::validate_args(&self.args)?;
        launch::validate_provider_column(self.provider, &self.args)?;
        let model = launch::flag_value(&self.args, "--model")
            .ok_or_else(|| anyhow::anyhow!("pi_args_forbidden: `--model` is required"))?;
        launch::validate_thinking(self.provider, &model, &self.thinking())?;
        if self.args.len() != 7 {
            anyhow::bail!("pi_args_forbidden: a pi row carries exactly four flags");
        }
        Ok(())
    }
}

fn recipe(
    id: &'static str,
    provider: &'static str,
    model: &'static str,
    thinking: &'static str,
    enabled: bool,
    start_time_allowed: bool,
    plain: &'static str,
) -> PiRecipe {
    PiRecipe {
        id,
        kind: "pi",
        provider,
        model_family: model,
        args: launch::start_args(provider, model, thinking),
        env: Vec::new(),
        ready_timeout_ms: 30_000,
        enabled,
        start_time_allowed,
        plain,
    }
}

/// The rows this round ships (SPEC-pi v2 §3.5, §6.2).
pub fn pi_recipes() -> Vec<PiRecipe> {
    vec![
        recipe(
            "pi_deepseek_flash",
            "deepseek",
            "deepseek-v4-flash",
            "low",
            true,
            true,
            "the cheap coding helper",
        ),
        recipe(
            "pi_codex_sol_high",
            "openai-codex",
            "gpt-5.6-sol",
            "high",
            true,
            false,
            "the careful number helper",
        ),
        recipe(
            "pi_codex_astra_xhigh",
            "openai-codex",
            "gpt-6-astra",
            "xhigh",
            false,
            false,
            "the hardest problem helper",
        ),
        recipe(
            "pi_opencode_muse",
            "opencode-go",
            "muse-spark-1.3-contributor",
            "high",
            true,
            true,
            "the second coding helper",
        ),
        recipe(
            "pi_kimi_k3",
            "kimi-coding",
            "k3",
            "high",
            true,
            true,
            "the long task helper",
        ),
    ]
}

/// Rows that deliberately do not ship yet, with the reason.
pub fn withheld_recipes() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "pi_xai_grok_xhigh",
            "Grok under pi waits for Rolf's word (decision 19:25, item 6)",
        ),
        (
            "pi_opencode_grok_xhigh",
            "the OpenCode Grok door is a Grok route; withheld with the xAI row",
        ),
    ]
}

/// Providers a pi row may name. Never `cursor` (SPEC-pi v2 §3.5).
pub fn is_pi_provider(provider: &str) -> bool {
    launch::PROVIDERS.contains(&provider)
}

/// Every provider an enabled recipe uses, in row order, for doctor.
pub fn enabled_providers() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for row in pi_recipes().iter().filter(|r| r.enabled) {
        if !out.contains(&row.provider) {
            out.push(row.provider);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_row_validates_and_carries_four_flags() {
        for row in pi_recipes() {
            row.validate().unwrap_or_else(|e| panic!("{}: {e}", row.id));
            assert_eq!(row.kind, "pi");
            assert!(row.env.is_empty());
            assert_eq!(row.ready_timeout_ms, 30_000);
            assert!(row.args.contains(&"--no-skills".to_string()));
            assert_eq!(
                launch::flag_value(&row.args, "--provider").as_deref(),
                Some(row.provider)
            );
        }
    }

    #[test]
    fn astra_is_disabled_and_sol_is_not_a_start_time_row() {
        let rows = pi_recipes();
        let astra = rows
            .iter()
            .find(|r| r.id == "pi_codex_astra_xhigh")
            .unwrap();
        assert!(!astra.enabled);
        let sol = rows.iter().find(|r| r.id == "pi_codex_sol_high").unwrap();
        assert!(sol.enabled && !sol.start_time_allowed);
    }

    #[test]
    fn there_is_no_cursor_recipe_and_no_grok_recipe_yet() {
        for row in pi_recipes() {
            assert!(!row.id.contains("cursor"));
            assert!(!row.provider.contains("cursor"));
        }
        let witheld = withheld_recipes();
        assert!(witheld.iter().any(|(id, _)| *id == "pi_xai_grok_xhigh"));
        assert!(!pi_recipes().iter().any(|r| r.id.contains("grok")));
    }

    #[test]
    fn enabled_providers_are_deduped_and_keep_fable_out() {
        let providers = enabled_providers();
        assert_eq!(
            providers,
            vec!["deepseek", "openai-codex", "opencode-go", "kimi-coding"]
        );
        assert!(!providers.contains(&"cursor"));
    }

    #[test]
    fn a_row_with_a_cursor_provider_is_refused_by_the_row_check() {
        let row = PiRecipe {
            id: "pi_cursor",
            kind: "pi",
            provider: "cursor",
            model_family: "x",
            args: launch::start_args("cursor", "x", "high"),
            env: Vec::new(),
            ready_timeout_ms: 30_000,
            enabled: true,
            start_time_allowed: false,
            plain: "x",
        };
        assert!(
            row.validate()
                .unwrap_err()
                .to_string()
                .contains("pi_cursor_forbidden")
        );
    }
}
