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
/// The authenticated endpoint accepted ordinary requests but refused the
/// measured 750 KiB review request. Keep a wide margin below that boundary.
pub const REQUEST_BYTE_CAP: usize = 256 * 1024;
const STATUS_MARKER: &str = "\nHERDR_HTTP_STATUS:";
const ERROR_TEXT_CAP: usize = 4096;

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
    let body = body.to_string();
    if body.len() > REQUEST_BYTE_CAP {
        bail!(
            "jev_request_too_large: serialized request is {} bytes; cap is {REQUEST_BYTE_CAP}",
            body.len()
        );
    }
    let config = format!(
        "url = {}\nrequest = \"POST\"\nheader = {}\nheader = \"Content-Type: application/json\"\ndata = {}\nwrite-out = {}\n",
        quote(URL),
        quote(&format!("Authorization: Bearer {key}")),
        quote(&body),
        quote(&format!("{STATUS_MARKER}%{{http_code}}"))
    );
    let output = ctx
        .runner
        .run(
            &Cmd::new("/usr/bin/curl", Duration::from_secs(35))
                .args([
                    "--disable",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "30",
                    "--config",
                    "-",
                ])
                .env_remove("TYPESAFE_API_KEY")
                .stdin(config),
        )
        .context("jev_transport: could not execute curl")?;
    if output.timed_out || output.code == Some(28) {
        bail!("jev_timeout: request timed out after 30 seconds");
    }
    if !output.success() {
        bail!(
            "jev_transport: curl failed (exit {:?}): {}",
            output.code,
            safe_error_text(&output.error_text(), key)
        );
    }
    let (response, status) = output
        .stdout
        .rsplit_once(STATUS_MARKER)
        .context("jev_transport: curl response did not include an HTTP status")?;
    let status: u16 = status
        .trim()
        .parse()
        .context("jev_transport: curl returned an invalid HTTP status")?;
    if !(200..300).contains(&status) {
        bail!(
            "jev_server_refused: HTTP {status}: {}",
            safe_error_text(response, key)
        );
    }
    parse(response, questions)
}

fn safe_error_text(text: &str, key: &str) -> String {
    let redacted = text.replace(key, "[redacted]");
    let mut safe: String = redacted
        .chars()
        .map(|c| {
            if c.is_control() && !matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    if safe.len() > ERROR_TEXT_CAP {
        let mut end = ERROR_TEXT_CAP - '…'.len_utf8();
        while !safe.is_char_boundary(end) {
            end -= 1;
        }
        safe.truncate(end);
        safe.push('…');
    }
    let safe = safe.trim();
    if safe.is_empty() {
        "(empty response)".to_string()
    } else {
        safe.to_string()
    }
}

#[cfg(test)]
pub fn http_response(body: &str, status: u16) -> String {
    format!("{body}{STATUS_MARKER}{status}")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text_is_redacted_control_cleaned_and_byte_bounded() {
        let text = format!("fake-key\u{1b}[31m{}", "界".repeat(ERROR_TEXT_CAP));
        let safe = safe_error_text(&text, "fake-key");
        assert!(safe.starts_with("[redacted] [31m"));
        assert!(!safe.contains("fake-key"));
        assert!(!safe.contains('\u{1b}'));
        assert!(safe.ends_with('…'));
        assert!(safe.len() <= ERROR_TEXT_CAP);
        assert_eq!(safe_error_text("\0 ", "fake-key"), "(empty response)");
    }
}
