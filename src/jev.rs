//! TypeSafe System One HTTP client. Credentials only travel on curl's stdin.
//! Wire contract: https://docs.typesafe.ai/api.md and /primitives/score.md.
use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths::Ctx;
use crate::runner::Cmd;

pub const URL: &str = "https://api.typesafe.ai/v1/systemone";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Score {
    pub score: f64,
    pub confidence: f64,
    pub probabilities: BTreeMap<String, f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assessment {
    pub scores: BTreeMap<String, Score>,
    pub model: String,
    pub usage: Value,
    pub response: Value,
}

pub fn call(ctx: &Ctx, body: &Value, questions: &BTreeMap<String, Value>) -> Result<Assessment> {
    let key = ctx
        .env
        .var("TYPESAFE_API_KEY")
        .filter(|k| !k.trim().is_empty())
        .context("jev_key_missing: set TYPESAFE_API_KEY in the dispatch process environment")?;
    if key.chars().any(char::is_control) {
        bail!("jev_key_invalid: TYPESAFE_API_KEY contains a control character");
    }
    let quote = |s: &str| serde_json::to_string(s).expect("string serialization");
    let config = format!(
        "url = {}\nrequest = \"POST\"\nheader = {}\nheader = \"Content-Type: application/json\"\ndata = {}\n",
        quote(URL),
        quote(&format!("Authorization: Bearer {key}")),
        quote(&body.to_string())
    );
    let output = ctx
        .runner
        .run(
            &Cmd::new("/usr/bin/curl", Duration::from_secs(35))
                .args([
                    "--disable",
                    "--silent",
                    "--show-error",
                    "--fail",
                    "--max-time",
                    "30",
                    "--config",
                    "-",
                ])
                .env_remove("TYPESAFE_API_KEY")
                .stdin(config),
        )
        .context("jev_transport: could not execute curl")?;
    if !output.success() {
        bail!(
            "jev_transport: request failed (exit {:?}, timeout {})",
            output.code,
            output.timed_out
        );
    }
    parse(&output.stdout, questions)
}

fn probability(v: f64) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}

pub fn parse(text: &str, questions: &BTreeMap<String, Value>) -> Result<Assessment> {
    let value: Value = serde_json::from_str(text).context("jev_response: invalid JSON")?;
    let mut scores = BTreeMap::new();
    for (id, question) in questions {
        let answer = &value["answers"][id];
        let score: Score = serde_json::from_value(answer.clone())
            .with_context(|| format!("jev_response: invalid score {id}"))?;
        let n = question["criteria"]
            .as_array()
            .context("jev_question: criteria must be an array")?
            .len();
        if n < 2
            || answer["type"] != "score"
            || !score.score.is_finite()
            || !(0.0..=(n - 1) as f64).contains(&score.score)
            || !probability(score.confidence)
            || score.probabilities.len() != n
            || !score.probabilities.values().all(|p| probability(*p))
            || (0..n).any(|i| !score.probabilities.contains_key(&i.to_string()))
            || (score.probabilities.values().sum::<f64>() - 1.0).abs() > 0.01
        {
            bail!("jev_response: invalid score distribution for {id}");
        }
        // Keep the full distribution so a future policy can replay decisions.
        scores.insert(id.clone(), score);
    }
    Ok(Assessment {
        scores,
        model: value["model"]
            .as_str()
            .context("jev_response: missing model")?
            .into(),
        usage: value["usage"].clone(),
        response: value,
    })
}
