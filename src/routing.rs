//! Small, editable recipe routing and bounded recovery policy.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::Recipe;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rule {
    pub workflow: Option<String>,
    pub product: Option<String>,
    pub requires_claude: Option<bool>,
    pub recipe: String,
    pub retries: Option<u32>,
    pub fallback: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Routing {
    pub default: String,
    pub retries: u32,
    pub fallback: Vec<String>,
    pub pins: BTreeMap<String, String>,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkContract {
    pub workflow: String,
    pub product: String,
    pub requires_claude: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub recipe: String,
    pub rule: String,
    pub pinned: bool,
}

impl Routing {
    pub fn validate(&self, recipes: &BTreeMap<String, Recipe>) -> Result<()> {
        if self.default.trim().is_empty() {
            bail!("routing_default_empty: [routing].default must name a recipe");
        }
        self.validate_recipe(recipes, &self.default)?;
        for recipe in &self.fallback {
            self.validate_recipe(recipes, recipe)?;
        }
        for (hash, recipe) in &self.pins {
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("routing_pin_invalid: pins use SHA-256 brief hashes");
            }
            self.validate_recipe(recipes, recipe)?;
        }
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.workflow.is_none() && rule.product.is_none() && rule.requires_claude.is_none() {
                bail!("routing_rule_invalid: rule[{index}] has no matcher");
            }
            if rule.recipe.trim().is_empty() {
                bail!("routing_rule_invalid: rule[{index}] has no recipe");
            }
            self.validate_recipe(recipes, &rule.recipe)?;
            if let Some(fallback) = &rule.fallback {
                for recipe in fallback {
                    self.validate_recipe(recipes, recipe)?;
                }
            }
        }
        Ok(())
    }

    fn validate_recipe(&self, recipes: &BTreeMap<String, Recipe>, id: &str) -> Result<()> {
        let Some(recipe) = recipes.get(id) else {
            bail!("routing_recipe_unknown: {id}");
        };
        if !recipe.enabled {
            bail!("routing_recipe_disabled: {id}");
        }
        Ok(())
    }

    pub fn select(
        &self,
        brief_hash: &str,
        work: &WorkContract,
        recovery: u32,
    ) -> Result<Selection> {
        let matched = self.rules.iter().enumerate().find(|(_, rule)| {
            rule.workflow
                .as_ref()
                .is_none_or(|value| value == &work.workflow)
                && rule
                    .product
                    .as_ref()
                    .is_none_or(|value| value == &work.product)
                && rule
                    .requires_claude
                    .is_none_or(|value| value == work.requires_claude)
        });
        let (base, retries, fallback, mut label) = if let Some((index, rule)) = matched {
            (
                rule.recipe.as_str(),
                rule.retries.unwrap_or(self.retries),
                rule.fallback.as_ref().unwrap_or(&self.fallback),
                format!("rule[{index}]"),
            )
        } else {
            (
                self.default.as_str(),
                self.retries,
                &self.fallback,
                "default".to_string(),
            )
        };
        let pin = self.pins.get(brief_hash);
        if pin.is_some() {
            label = "pin".into();
        }
        let first = pin.map_or(base, String::as_str);
        let recipe = if recovery <= retries {
            first
        } else {
            let fallback_index = recovery.saturating_sub(retries).saturating_sub(1) as usize;
            fallback.get(fallback_index).map(String::as_str).ok_or_else(|| {
                anyhow::anyhow!(
                    "recovery_exhausted: {label} allowed {retries} retries and {} fallbacks; waiting for the coordinator",
                    fallback.len()
                )
            })?
        };
        Ok(Selection {
            recipe: recipe.to_string(),
            rule: label,
            pinned: pin.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> Recipe {
        Recipe {
            kind: "pi".into(),
            plain: "the quick helper".into(),
            ..Recipe::default()
        }
    }

    fn recipes() -> BTreeMap<String, Recipe> {
        ["default", "rule", "pinned", "fallback"]
            .into_iter()
            .map(|id| (id.to_string(), recipe()))
            .collect()
    }

    fn routing() -> Routing {
        Routing {
            default: "default".into(),
            retries: 1,
            fallback: vec!["fallback".into()],
            pins: BTreeMap::new(),
            rules: vec![Rule {
                workflow: Some("reviewer".into()),
                recipe: "rule".into(),
                ..Rule::default()
            }],
        }
    }

    fn work(workflow: &str) -> WorkContract {
        WorkContract {
            workflow: workflow.into(),
            product: String::new(),
            requires_claude: false,
        }
    }

    #[test]
    fn ordered_rule_then_default_then_pin() {
        let mut table = routing();
        assert_eq!(
            table.select("hash", &work("reviewer"), 0).unwrap().recipe,
            "rule"
        );
        assert_eq!(
            table.select("hash", &work("lane"), 0).unwrap().recipe,
            "default"
        );
        let hash = "a".repeat(64);
        table.pins.insert(hash.clone(), "pinned".into());
        let selected = table.select(&hash, &work("reviewer"), 0).unwrap();
        assert_eq!(selected.recipe, "pinned");
        assert_eq!(selected.rule, "pin");
    }

    #[test]
    fn recovery_retries_then_falls_back_then_stops() {
        let table = routing();
        assert_eq!(
            table.select("hash", &work("lane"), 1).unwrap().recipe,
            "default"
        );
        assert_eq!(
            table.select("hash", &work("lane"), 2).unwrap().recipe,
            "fallback"
        );
        let error = table
            .select("hash", &work("lane"), 3)
            .unwrap_err()
            .to_string();
        assert!(error.contains("recovery_exhausted"), "{error}");
        assert!(error.contains("waiting for the coordinator"), "{error}");
    }

    #[test]
    fn validation_rejects_an_unknown_rule_recipe() {
        let mut table = routing();
        table.rules[0].recipe = "missing".into();
        let error = table.validate(&recipes()).unwrap_err().to_string();
        assert_eq!(error, "routing_recipe_unknown: missing");
    }
}
