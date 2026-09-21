//! Editable real-unit task rubric, price-aware routing and observed-outcome replay.
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
    /// Capability order used only by floors and failure escalation.
    pub tier: u32,
    /// Published Artificial Analysis Coding Index.
    pub coding_index: f64,
    /// Blended dollars per million tokens, used to choose among capable models.
    pub price_per_million: f64,
}
/// A veto on the raw, zero-based Score answer (not its interpolated index).
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
    /// Real Coding Index units corresponding to the Score criteria.
    pub index_values: Vec<f64>,
    pub models: BTreeMap<String, ModelCard>,
    pub confidence_floor: f64,
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
        if self.version != 2
            || self.model.is_empty()
            || self.note.is_empty()
            || !(0.0..=1.0).contains(&self.confidence_floor)
            || self.models.is_empty()
            || self.models.values().any(|m| {
                m.description.trim().is_empty()
                    || m.tier == 0
                    || !m.coding_index.is_finite()
                    || m.coding_index <= 0.0
                    || !m.price_per_million.is_finite()
                    || m.price_per_million < 0.0
            })
            || self.questions.len() != 1
        {
            bail!(
                "routing_policy_invalid: version, one index question, model measurements or confidence floor"
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
        let criteria_len = self.questions.values().next().expect("one question")["criteria"]
            .as_array()
            .expect("validated criteria")
            .len();
        if self.index_values.len() != criteria_len
            || self.index_values.iter().any(|v| !v.is_finite() || *v < 0.0)
            || !self.index_values.windows(2).all(|w| w[0] < w[1])
        {
            bail!(
                "routing_policy_invalid: index_values must increase and match the Score criteria"
            );
        }
        let mut tiers = BTreeSet::new();
        if self.models.values().any(|m| !tiers.insert(m.tier)) {
            bail!("routing_policy_invalid: model capability tiers must be unique");
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
        for id in self.models.keys().chain(self.floor_recipes()) {
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
        let (question, _) = self.questions.iter().next().expect("validated question");
        let answer = assessment
            .scores
            .get(question)
            .context("routing_answer_missing")?;
        let score = interpolate(&self.index_values, answer.score)?;
        let confidence = answer.confidence;
        let base = self.cheapest_capable(score, 0)?;
        let mut recipe = base.clone();
        let mut reasons = Vec::new();
        if confidence < self.confidence_floor
            && let Ok(stronger) = self.cheapest_capable(score, self.models[&recipe].tier + 1)
        {
            recipe = stronger;
            reasons.push("low-confidence");
        }
        if let Some(tier) = previous_tier {
            recipe = self
                .cheapest_capable(score, tier + 1)
                .context("escalation_exhausted: no stronger model remains in the ladder")?;
            reasons.push("failure");
        }
        let mut decision = Decision {
            upgraded: recipe != base,
            recipe,
            score,
            confidence,
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

    fn cheapest_capable(&self, required_index: f64, min_tier: u32) -> Result<String> {
        self.models
            .iter()
            .filter(|(_, card)| card.coding_index >= required_index && card.tier >= min_tier)
            .min_by(|(a_id, a), (b_id, b)| {
                a.price_per_million
                    .total_cmp(&b.price_per_million)
                    .then_with(|| a.coding_index.total_cmp(&b.coding_index))
                    .then_with(|| a_id.cmp(b_id))
            })
            .map(|(id, _)| id.clone())
            .or_else(|| {
                self.models
                    .iter()
                    .filter(|(_, card)| card.tier >= min_tier)
                    .max_by(|a, b| a.1.coding_index.total_cmp(&b.1.coding_index))
                    .map(|(id, _)| id.clone())
            })
            .context("routing_model_missing: no model satisfies the required index and tier")
    }

    pub fn strongest(&self) -> &str {
        self.models
            .iter()
            .max_by_key(|(_, card)| card.tier)
            .expect("validated models")
            .0
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

fn interpolate(values: &[f64], score: f64) -> Result<f64> {
    if !score.is_finite() || score < 0.0 || score > (values.len() - 1) as f64 {
        bail!("routing_answer_invalid: Score is outside its criteria");
    }
    let low = score.floor() as usize;
    let high = score.ceil() as usize;
    if low == high {
        return Ok(values[low]);
    }
    Ok(values[low] + (values[high] - values[low]) * score.fract())
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

#[derive(Debug, Default, Serialize)]
pub struct Evaluation {
    pub finished_lanes: usize,
    pub observed_good_enough: usize,
    pub observed_not_good_enough: usize,
    pub no_round_outcome: usize,
    pub saved_assessments: usize,
    pub confidence_clear: usize,
    pub policy_confirmed_good: usize,
    pub policy_confirmed_bad: usize,
    pub policy_untried: usize,
    pub outcomes: Vec<Value>,
}

/// Replay the current policy only against work that actually ran. A different
/// model is an untried counterfactual, never a fabricated right/wrong label.
pub fn evaluate(ctx: &crate::paths::Ctx, project: &crate::project::Project) -> Result<Evaluation> {
    let policy = Policy::read(&ctx.config_dir.join("routing.json"))?;
    let config = crate::launch::parse_launch_config(&ctx.config_dir)?;
    policy.validate_recipes(&config.recipes)?;
    let ledger = read_dispatch_ledger(project)?;
    let events = crate::round::sealed_events(project)?;
    let rounds = crate::round::list(project);
    let mut result = Evaluation::default();

    for thread in crate::thread::list(project) {
        let Some(done) = events
            .iter()
            .filter(|event| event.thread == thread.id && event.payload.done.is_some())
            .max_by(|a, b| (&a.created, &a.id).cmp(&(&b.created, &b.id)))
        else {
            continue;
        };
        result.finished_lanes += 1;
        let carrying_round = done
            .round
            .as_deref()
            .and_then(|id| rounds.iter().find(|r| r.round == id))
            .or_else(|| {
                rounds.iter().find(|r| {
                    r.manifest.members.iter().any(|m| m.thread == thread.id)
                        || r.reviewer.as_deref() == Some(&thread.id)
                })
            });
        let merged_without_reject = carrying_round.and_then(|round| {
            round.rejections.map(|rejections| {
                round.phase == crate::contracts::RoundPhase::Merged && rejections == 0
            })
        });
        let escalated = thread.launch.escalations > 0;
        let observed_good = merged_without_reject == Some(true) && !escalated;
        match merged_without_reject {
            None => result.no_round_outcome += 1,
            Some(_) if observed_good => result.observed_good_enough += 1,
            Some(_) => result.observed_not_good_enough += 1,
        }

        let brief_hash = if thread.launch.brief_hash.is_empty() {
            std::fs::read(crate::thread::task_path(project, &thread.id))
                .map(|bytes| crate::thread::sha256_hex(&bytes))
                .unwrap_or_default()
        } else {
            thread.launch.brief_hash.clone()
        };
        let dispatch = ledger.iter().rev().find(|row| {
            row["brief_hash"] == brief_hash
                && row["recipe"] == thread.launch.recipe_id
                && row["escalations"].as_u64() == Some(thread.launch.escalations as u64)
        });
        let assessment: Option<Assessment> = dispatch
            .and_then(|row| row.get("assessment"))
            .filter(|value| !value.is_null())
            .and_then(|value| serde_json::from_value(value.clone()).ok());
        let decision = assessment
            .as_ref()
            .and_then(|saved| policy.select(saved, None, &thread.role).ok());
        if let Some(saved) = &assessment {
            result.saved_assessments += 1;
            if saved
                .scores
                .values()
                .all(|score| score.confidence >= policy.confidence_floor)
            {
                result.confidence_clear += 1;
            }
        }
        let policy_result = match decision.as_ref() {
            Some(d) if d.recipe == thread.launch.recipe_id && observed_good => {
                result.policy_confirmed_good += 1;
                "confirmed-good"
            }
            Some(d) if d.recipe == thread.launch.recipe_id => {
                result.policy_confirmed_bad += 1;
                "confirmed-bad"
            }
            Some(_) => {
                result.policy_untried += 1;
                "untried"
            }
            None => "not-scored",
        };
        result.outcomes.push(json!({
            "thread": thread.id,
            "model_run": thread.launch.recipe_id,
            "price_per_million": policy.models.get(&thread.launch.recipe_id).map(|m| m.price_per_million),
            "escalated": escalated,
            "failure": dispatch.and_then(|row| row.get("failure")).filter(|v| !v.is_null()),
            "round": carrying_round.map(|round| &round.round),
            "merged_without_reject": merged_without_reject,
            "saved_assessment": assessment,
            "policy_selection": decision.as_ref().map(|d| &d.recipe),
            "required_coding_index": decision.as_ref().map(|d| d.score),
            "policy_result": policy_result
        }));
    }
    Ok(result)
}

fn read_dispatch_ledger(project: &crate::project::Project) -> Result<Vec<Value>> {
    let path = project.state_dir().join("dispatch.jsonl");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(line, text)| {
            serde_json::from_str(text)
                .with_context(|| format!("{} line {} is not JSON", path.display(), line + 1))
        })
        .collect()
}
