//! Editable task rubric, deterministic routing arithmetic and labelled-case replay.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::contracts::Recipe;
use crate::jev::Assessment;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCard {
    pub description: String,
    pub tier: u32,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub up_to: f64,
    pub recipe: String,
}
/// A veto on the raw, zero-based Score answer (not its weighted contribution).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerFloor {
    pub question: String,
    pub min_score: f64,
    pub recipe: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FloorCause {
    Role {
        role: String,
    },
    Answer {
        question: String,
        min_score: f64,
        score: f64,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct AppliedFloor {
    pub recipe: String,
    pub tier: u32,
    pub cause: FloorCause,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub note: String,
    pub model: String,
    pub questions: BTreeMap<String, Value>,
    pub weights: BTreeMap<String, f64>,
    pub models: BTreeMap<String, ModelCard>,
    pub routes: Vec<Route>,
    pub confidence_floor: f64,
    pub borderline_margin: f64,
    /// Hand-written by the user, never accepted by `thread start`.
    pub pins: BTreeMap<String, String>,
    #[serde(default)]
    pub role_floors: BTreeMap<String, String>,
    #[serde(default)]
    pub answer_floors: Vec<AnswerFloor>,
}
#[derive(Debug, Serialize)]
pub struct Decision {
    pub recipe: String,
    pub score: f64,
    pub confidence: f64,
    pub upgraded: bool,
    pub upgrade_reason: String,
    pub floors: Vec<AppliedFloor>,
}
impl Policy {
    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "routing_policy_missing: install editable policy at {}",
                path.display()
            )
        })?;
        Self::parse(text.as_bytes())
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let p: Self = serde_json::from_slice(bytes).context("routing_policy_invalid")?;
        p.validate()?;
        Ok(p)
    }
    fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.model.is_empty()
            || self.note.is_empty()
            || !(0.0..=1.0).contains(&self.confidence_floor)
            || !(0.0..=1.0).contains(&self.borderline_margin)
            || self.models.is_empty()
            || self
                .models
                .values()
                .any(|m| m.description.trim().is_empty() || m.tier == 0)
            || self.questions.len() != 3
            || self.questions.keys().ne(self.weights.keys())
            || self.weights.values().any(|w| !w.is_finite() || *w <= 0.0)
            || !self.weights.values().sum::<f64>().is_finite()
        {
            bail!(
                "routing_policy_invalid: version, model cards, three weighted questions or thresholds"
            );
        }
        for (id, q) in &self.questions {
            if q.get("instructions").is_none()
                || q["type"] != "score"
                || !q["criteria"]
                    .as_array()
                    .is_some_and(|a| a.len() >= 2 && a.iter().all(Value::is_string))
            {
                bail!(
                    "routing_policy_invalid: {id} needs Score instructions and an ordered criteria array"
                );
            }
        }
        if self.routes.is_empty() || self.routes.last().is_none_or(|r| r.up_to != 1.0) {
            bail!("routing_policy_invalid: routes must cover scores through 1.0");
        }
        let mut previous = (-1.0, 0);
        for route in &self.routes {
            let card = self
                .models
                .get(&route.recipe)
                .context("routing_policy_invalid: route has no model card")?;
            if !(0.0..=1.0).contains(&route.up_to)
                || route.up_to <= previous.0
                || card.tier <= previous.1
            {
                bail!("routing_policy_invalid: cutoffs and capability tiers must increase");
            }
            previous = (route.up_to, card.tier);
        }
        for (hash, recipe) in &self.pins {
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || recipe.is_empty()
            {
                bail!("routing_policy_invalid: pins use task SHA-256 keys and recipe values");
            }
        }
        for role in self.role_floors.keys() {
            if !matches!(
                role.as_str(),
                "lane" | "reviewer" | "critic" | "drafter" | "research" | "planner" | "coordinator"
            ) {
                bail!("routing_policy_invalid: unknown floor role {role}");
            }
        }
        for floor in &self.answer_floors {
            let question = self
                .questions
                .get(&floor.question)
                .context("routing_policy_invalid: answer floor has unknown question")?;
            let top = question["criteria"]
                .as_array()
                .expect("validated criteria")
                .len()
                - 1;
            if !floor.min_score.is_finite() || !(0.0..=top as f64).contains(&floor.min_score) {
                bail!("routing_policy_invalid: answer floor min_score outside criteria range");
            }
        }
        for id in self.floor_recipes() {
            if !self.models.contains_key(id) {
                bail!("routing_policy_invalid: floor {id} has no model card");
            }
        }
        Ok(())
    }
    fn floor_recipes(&self) -> impl Iterator<Item = &String> {
        self.role_floors
            .values()
            .chain(self.answer_floors.iter().map(|f| &f.recipe))
    }
    pub fn validate_recipes(&self, recipes: &BTreeMap<String, Recipe>) -> Result<()> {
        for id in self.models.keys().chain(self.pins.values()) {
            if !recipes.contains_key(id) {
                bail!("routing_recipe_missing: {id}");
            }
        }
        for id in self
            .routes
            .iter()
            .map(|r| &r.recipe)
            .chain(self.floor_recipes())
        {
            let recipe = recipes
                .get(id)
                .with_context(|| format!("routing_recipe_missing: {id}"))?;
            if !recipe.enabled {
                bail!("routing_recipe_disabled: {id}");
            }
        }
        Ok(())
    }
    /// The exclusions are product/runtime rules, not a web-access classifier.
    pub fn exclusion<'a>(&'a self, brief: &str, contract: &Value) -> Option<&'a str> {
        self.pins
            .get(&crate::thread::sha256_hex(brief.as_bytes()))
            .map(String::as_str)
            .or_else(|| {
                if contract["workflow"] == "coordinator" {
                    Some("claude_coordinator_opus")
                } else if contract["requires_claude"] == true {
                    Some("claude_fable_xhigh")
                } else if contract["product"] == "web-research" {
                    Some("agy_gemini_flash")
                } else if contract["product"] == "spec" {
                    Some("claude_fable_xhigh")
                } else {
                    None
                }
            })
    }
    /// Roster changes never change the inference prompt.
    pub fn request(&self, state: Value) -> Value {
        json!({"model": self.model, "state": state, "questions": self.questions})
    }
    pub fn select(
        &self,
        assessment: &Assessment,
        previous_tier: Option<u32>,
        workflow: &str,
    ) -> Result<Decision> {
        let mut score = 0.0;
        let mut confidence: f64 = 1.0;
        for (id, weight) in &self.weights {
            let answer = assessment
                .scores
                .get(id)
                .context("routing_answer_missing")?;
            let top = self.questions[id]["criteria"]
                .as_array()
                .context("routing_criteria_invalid")?
                .len()
                - 1;
            score += weight * answer.score / top as f64;
            confidence = confidence.min(answer.confidence);
        }
        score /= self.weights.values().sum::<f64>();
        let base = self
            .routes
            .iter()
            .position(|r| score <= r.up_to)
            .unwrap_or(self.routes.len() - 1);
        let mut index = base;
        let mut reasons = Vec::new();
        if confidence < self.confidence_floor {
            index = (index + 1).min(self.routes.len() - 1);
            reasons.push("low-confidence");
        }
        for (i, route) in self.routes.iter().enumerate().take(self.routes.len() - 1) {
            if (score - route.up_to).abs() <= self.borderline_margin {
                index = index.max(i + 1);
                reasons.push("borderline");
            }
        }
        if let Some(tier) = previous_tier {
            let stronger = self
                .routes
                .iter()
                .position(|r| self.models[&r.recipe].tier > tier)
                .context("escalation_exhausted: no stronger model remains in the ladder")?;
            index = index.max(stronger);
            reasons.push("failure");
        }
        let mut decision = Decision {
            recipe: self.routes[index].recipe.clone(),
            score,
            confidence,
            upgraded: index > base,
            upgrade_reason: reasons.join(","),
            floors: Vec::new(),
        };
        decision.floors = self.apply_floors(&mut decision.recipe, workflow, Some(assessment));
        if !decision.floors.is_empty() {
            decision.upgraded = true;
            if !decision.upgrade_reason.is_empty() {
                decision.upgrade_reason.push(',');
            }
            decision.upgrade_reason.push_str("floor");
        }
        Ok(decision)
    }

    /// Record all triggered constraints that raise the original pick; the strongest
    /// wins. Equal tiers keep the scorer's recipe, so floors never act as pins.
    pub fn apply_floors(
        &self,
        recipe: &mut String,
        workflow: &str,
        assessment: Option<&Assessment>,
    ) -> Vec<AppliedFloor> {
        let original_tier = self.models[recipe.as_str()].tier;
        let mut applied = Vec::new();
        let mut consider = |id: &String, cause: FloorCause| {
            let tier = self.models[id].tier;
            if tier > original_tier {
                applied.push(AppliedFloor {
                    recipe: id.clone(),
                    tier,
                    cause,
                });
                if tier > self.models[recipe.as_str()].tier {
                    *recipe = id.clone();
                }
            }
        };
        if let Some(id) = self.role_floors.get(workflow) {
            consider(
                id,
                FloorCause::Role {
                    role: workflow.into(),
                },
            );
        }
        if let Some(assessment) = assessment {
            for floor in &self.answer_floors {
                let score = assessment.scores[&floor.question].score;
                if score >= floor.min_score {
                    consider(
                        &floor.recipe,
                        FloorCause::Answer {
                            question: floor.question.clone(),
                            min_score: floor.min_score,
                            score,
                        },
                    );
                }
            }
        }
        applied
    }
}

/// Remove exact runtime/model ids from state without shortening the brief or
/// scrubbing ordinary words such as "high", "code" or "review".
pub fn scrub(mut state: Value, recipes: &BTreeMap<String, Recipe>) -> Value {
    let mut names: BTreeSet<String> = recipes.keys().map(|s| s.to_lowercase()).collect();
    for r in recipes.values() {
        for pair in r.args.windows(2) {
            if pair[0] == "--model" {
                names.insert(pair[1].to_lowercase());
            }
        }
    }
    fn visit(v: &mut Value, names: &BTreeSet<String>) {
        match v {
            Value::String(text) => {
                let mut out = String::new();
                let mut word = String::new();
                let flush = |word: &mut String, out: &mut String| {
                    if names.contains(&word.to_lowercase()) {
                        out.push_str("[model]");
                    } else {
                        out.push_str(word);
                    }
                    word.clear();
                };
                for c in text.chars() {
                    if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                        word.push(c);
                    } else {
                        flush(&mut word, &mut out);
                        out.push(c);
                    }
                }
                flush(&mut word, &mut out);
                *text = out;
            }
            Value::Array(a) => {
                for x in a {
                    visit(x, names);
                }
            }
            Value::Object(o) => {
                for x in o.values_mut() {
                    visit(x, names);
                }
            }
            _ => {}
        }
    }
    visit(&mut state, &names);
    state
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub brief: String,
    pub state: Value,
    pub expected: String,
    /// Saved TypeSafe response enables tuning with no further inference call.
    pub response: Option<Value>,
    #[serde(default = "default_workflow")]
    pub workflow: String,
}
fn default_workflow() -> String {
    "lane".into()
}
#[derive(Debug, Default, Serialize)]
pub struct Evaluation {
    pub correct: usize,
    pub over_routed: usize,
    pub under_routed: usize,
    pub same_tier_wrong: usize,
    pub errors: usize,
    pub cases: Vec<Value>,
}
pub fn evaluate(ctx: &crate::paths::Ctx, cases: &Path) -> Result<Evaluation> {
    let policy = Policy::read(&ctx.config_dir.join("routing.json"))?;
    let config = crate::launch::parse_launch_config(&ctx.config_dir)?;
    policy.validate_recipes(&config.recipes)?;
    let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(cases)?)?;
    let mut result = Evaluation::default();
    for case in cases {
        let contract = crate::launch::work_contract(&case.brief, &case.workflow)?;
        let assessment = if policy.exclusion(&case.brief, &contract).is_some() {
            None
        } else {
            Some(match &case.response {
                Some(response) => crate::jev::parse(&response.to_string(), &policy.questions),
                None => crate::jev::call(
                    ctx,
                    &policy.request(scrub(
                        json!({"brief": case.brief, "repository": case.state, "failure": null}),
                        &config.recipes,
                    )),
                    &policy.questions,
                ),
            })
        };
        let got = match &assessment {
            None => Ok(policy
                .exclusion(&case.brief, &contract)
                .expect("exclusion checked")
                .to_string()),
            Some(Ok(a)) => policy.select(a, None, &case.workflow).map(|d| d.recipe),
            Some(Err(e)) => Err(anyhow::anyhow!("{e:#}")),
        };
        let (selected, outcome) = match got {
            Ok(id) if id == case.expected => {
                result.correct += 1;
                (id, "correct")
            }
            Ok(id) => {
                let got_tier = policy.models.get(&id).map(|m| m.tier);
                let want_tier = policy.models.get(&case.expected).map(|m| m.tier);
                let outcome = match got_tier.zip(want_tier) {
                    Some((got, want)) if got > want => {
                        result.over_routed += 1;
                        "over-routed"
                    }
                    Some((got, want)) if got < want => {
                        result.under_routed += 1;
                        "under-routed"
                    }
                    Some(_) => {
                        result.same_tier_wrong += 1;
                        "same-tier-wrong"
                    }
                    None => {
                        result.errors += 1;
                        "unranked-model"
                    }
                };
                (id, outcome)
            }
            Err(e) => {
                result.errors += 1;
                (format!("{e:#}"), "error")
            }
        };
        result.cases.push(
            json!({"id": case.id, "brief": case.brief, "state": case.state, "workflow": case.workflow,
            "expected": case.expected, "selected": selected, "outcome": outcome,
            "assessment": assessment.as_ref().and_then(|a| a.as_ref().ok()),
            "response": assessment.as_ref().and_then(|a| a.as_ref().ok()).map(|a| &a.response)}),
        );
    }
    Ok(result)
}
