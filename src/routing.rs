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
    pub capability: Option<String>,
    pub recipe: String,
    pub retries: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Routing {
    pub default: String,
    pub retries: u32,
    pub pins: BTreeMap<String, String>,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkContract {
    pub workflow: String,
    pub product: String,
    pub capability: Option<String>,
    pub once: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub recipe: String,
    pub rule: String,
    pub pinned: bool,
}

impl Routing {
    pub fn recipe_ids(&self) -> std::collections::BTreeSet<&str> {
        let mut ids = std::collections::BTreeSet::new();
        ids.insert(self.default.as_str());
        ids.extend(self.pins.values().map(String::as_str));
        for rule in &self.rules {
            ids.insert(rule.recipe.as_str());
        }
        ids
    }

    pub fn validate(&self, recipes: &BTreeMap<String, Recipe>) -> Result<()> {
        if self.default.trim().is_empty() {
            bail!(
                "routing_default_missing: add [routing] with default = \"<recipe>\" to config.toml"
            );
        }
        self.validate_recipe(recipes, &self.default)?;
        for (hash, recipe) in &self.pins {
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("routing_pin_invalid: pins use SHA-256 brief hashes");
            }
            self.validate_recipe(recipes, recipe)?;
        }
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.workflow.is_none() && rule.product.is_none() && rule.capability.is_none() {
                bail!("routing_rule_invalid: rule[{index}] has no matcher");
            }
            if rule.recipe.trim().is_empty() {
                bail!("routing_rule_invalid: rule[{index}] has no recipe");
            }
            self.validate_recipe(recipes, &rule.recipe)?;
            if let Some(capability) = &rule.capability
                && !recipes[&rule.recipe].capabilities.contains(capability)
            {
                bail!(
                    "routing_capability_missing: rule[{index}] selects `{}` which does not declare `{capability}`",
                    rule.recipe
                );
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

    pub fn retry_limit(&self, work: &WorkContract) -> u32 {
        self.rules
            .iter()
            .find(|rule| {
                rule.workflow
                    .as_ref()
                    .is_none_or(|value| value == &work.workflow)
                    && rule
                        .product
                        .as_ref()
                        .is_none_or(|value| value == &work.product)
                    && rule
                        .capability
                        .as_ref()
                        .is_none_or(|value| work.capability.as_ref() == Some(value))
            })
            .and_then(|rule| rule.retries)
            .unwrap_or(self.retries)
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
                    .capability
                    .as_ref()
                    .is_none_or(|value| work.capability.as_ref() == Some(value))
        });
        let (base, retries, mut label) = if let Some((index, rule)) = matched {
            (
                rule.recipe.as_str(),
                rule.retries.unwrap_or(self.retries),
                format!("rule[{index}]"),
            )
        } else {
            (self.default.as_str(), self.retries, "default".to_string())
        };
        let pin = self.pins.get(brief_hash);
        if pin.is_some() {
            label = "pin".into();
        }
        let first = pin.map_or(base, String::as_str);
        if recovery > retries {
            bail!(
                "recovery_exhausted: {label} allowed {retries} retries; waiting for the coordinator"
            );
        }
        let recipe = first;
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
        ["default", "rule", "pinned"]
            .into_iter()
            .map(|id| (id.to_string(), recipe()))
            .collect()
    }

    fn routing() -> Routing {
        Routing {
            default: "default".into(),
            retries: 1,
            pins: BTreeMap::new(),
            rules: vec![Rule {
                workflow: Some("reviewer".into()),
                recipe: "rule".into(),
                ..Rule::default()
            }],
        }
    }

    #[test]
    fn missing_table_names_the_config_fix() {
        let error = Routing::default()
            .validate(&recipes())
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("routing_default_missing:"), "{error}");
    }

    #[test]
    fn validation_rejects_an_unknown_rule_recipe() {
        let mut table = routing();
        table.rules[0].recipe = "missing".into();
        let error = table.validate(&recipes()).unwrap_err().to_string();
        assert_eq!(error, "routing_recipe_unknown: missing");
    }
}
