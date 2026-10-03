//! Token accounting from the sealing lane's local transcript, never another machine.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths::Ctx;
use crate::thread::Thread;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Usage {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write: u64,
    pub(crate) reasoning: u64,
    pub(crate) total: u64,
}

impl Usage {
    fn add(&mut self, other: &Self) -> Result<()> {
        for (sum, value) in [
            (&mut self.input, other.input),
            (&mut self.output, other.output),
            (&mut self.cache_read, other.cache_read),
            (&mut self.cache_write, other.cache_write),
            (&mut self.reasoning, other.reasoning),
            (&mut self.total, other.total),
        ] {
            *sum = sum.checked_add(value).context("token count overflow")?;
        }
        Ok(())
    }
}

pub(crate) fn summary(usage: Option<&Usage>) -> String {
    let Some(usage) = usage.filter(|usage| usage.total > 0) else {
        return "usage unknown".into();
    };
    format!(
        "{} tokens ({} cached)",
        short(usage.total),
        short(usage.cache_read.saturating_add(usage.cache_write))
    )
}

fn short(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.1}K", tokens as f64 / 1_000.0),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

/// Missing, ambiguous, malformed and empty evidence all mean unknown, not zero.
pub(crate) fn collect(ctx: &Ctx, lane: &Thread) -> Option<Usage> {
    let kind = lane.launch.kind.as_str();
    let cwd = Path::new(&lane.cwd);
    let path = match kind {
        "pi" => {
            if let Some(file) = ctx.env.var("PI_SESSION_FILE") {
                PathBuf::from(file)
            } else {
                let agent = ctx
                    .env
                    .var("PI_CODING_AGENT_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        crate::pi::Layout {
                            root: ctx.root.join("pi"),
                        }
                        .agent()
                    });
                let name = format!(
                    "--{}--",
                    cwd.to_string_lossy().trim_matches('/').replace('/', "-")
                );
                latest_session(&agent.join("sessions").join(name), cwd, kind).ok()?
            }
        }
        "claude" => {
            let name: String = cwd
                .to_string_lossy()
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            latest_session(&ctx.env.home.join(".claude/projects").join(name), cwd, kind).ok()?
        }
        _ => return None,
    };
    read_usage(&path, cwd, kind).ok().flatten()
}

fn rows(path: &Path) -> Result<impl Iterator<Item = Result<Value>>> {
    Ok(BufReader::new(std::fs::File::open(path)?)
        .lines()
        .map(|line| {
            let line = line?;
            Ok(serde_json::from_str(&line)?)
        }))
}

fn session_start(path: &Path, cwd: &Path, kind: &str) -> Result<jiff::Timestamp> {
    for row in rows(path)? {
        let row = row?;
        if kind == "pi" && row["type"] != "session" {
            continue;
        }
        if let (Some(recorded), Some(timestamp)) = (row["cwd"].as_str(), row["timestamp"].as_str())
        {
            anyhow::ensure!(
                Path::new(recorded) == cwd,
                "transcript cwd differs from lane"
            );
            return Ok(timestamp.parse()?);
        }
    }
    anyhow::bail!("transcript has no session identity")
}

fn latest_session(dir: &Path, cwd: &Path, kind: &str) -> Result<PathBuf> {
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            sessions.push((session_start(&path, cwd, kind)?, path));
        }
    }
    sessions.sort();
    let (start, path) = sessions.pop().context("session file not found")?;
    anyhow::ensure!(
        sessions.last().is_none_or(|other| other.0 != start),
        "ambiguous session"
    );
    Ok(path)
}

fn number(value: &Value, key: &str) -> Result<u64> {
    match value.get(key) {
        None => Ok(0),
        Some(value) => value.as_u64().context("invalid token count"),
    }
}

fn read_usage(path: &Path, cwd: &Path, kind: &str) -> Result<Option<Usage>> {
    session_start(path, cwd, kind)?;
    let mut total = Usage::default();
    let mut claude = BTreeMap::<String, Usage>::new();
    for row in rows(path)? {
        let row = row?;
        let message = &row["message"];
        if message["role"] != "assistant" {
            continue;
        }
        let Some(value) = message.get("usage") else {
            continue;
        };
        anyhow::ensure!(value.is_object(), "invalid usage object");
        if kind == "pi" {
            total.add(&Usage {
                input: number(value, "input")?,
                output: number(value, "output")?,
                cache_read: number(value, "cacheRead")?,
                cache_write: number(value, "cacheWrite")?,
                reasoning: number(value, "reasoning")?,
                total: number(value, "totalTokens")?,
            })?;
        } else {
            let id = message["id"]
                .as_str()
                .context("assistant message has no id")?;
            let usage = claude.entry(id.to_string()).or_default();
            // Streaming blocks can repeat an id with later, fuller counters.
            usage.input = usage.input.max(number(value, "input_tokens")?);
            usage.output = usage.output.max(number(value, "output_tokens")?);
            usage.cache_read = usage
                .cache_read
                .max(number(value, "cache_read_input_tokens")?);
            usage.cache_write = usage
                .cache_write
                .max(number(value, "cache_creation_input_tokens")?);
            usage.reasoning = usage.reasoning.max(number(value, "reasoning_tokens")?);
        }
    }
    for mut usage in claude.into_values() {
        usage.total = [
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write,
            usage.reasoning,
        ]
        .into_iter()
        .try_fold(0_u64, |sum, value| sum.checked_add(value))
        .context("token count overflow")?;
        total.add(&usage)?;
    }
    Ok((total.total > 0).then_some(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(kind: &str, messages: &[Value]) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session.jsonl");
        let header = serde_json::json!({"type": if kind == "pi" { "session" } else { "user" }, "cwd": "/lane", "timestamp": "2026-10-03T00:00:00Z"});
        let mut text = format!("{header}\n");
        for message in messages {
            text.push_str(&format!(
                "{}\n",
                serde_json::json!({"type":"message", "message":message})
            ));
        }
        std::fs::write(&path, text).unwrap();
        (root, path)
    }

    #[test]
    fn pi_sums_assistant_usage_including_cache_and_reasoning() {
        let (_root, path) = transcript(
            "pi",
            &[
                serde_json::json!({"role":"assistant","usage":{"input":1171,"output":24,"cacheRead":4900000,"cacheWrite":0,"reasoning":5,"totalTokens":4901195}}),
                serde_json::json!({"role":"toolResult","usage":{"totalTokens":9999999}}),
                serde_json::json!({"role":"assistant","usage":{"input":1,"output":2,"cacheRead":3,"cacheWrite":4,"reasoning":1,"totalTokens":10}}),
            ],
        );
        let usage = read_usage(&path, Path::new("/lane"), "pi")
            .unwrap()
            .unwrap();
        assert_eq!(
            usage,
            Usage {
                input: 1172,
                output: 26,
                cache_read: 4900003,
                cache_write: 4,
                reasoning: 6,
                total: 4901205
            }
        );
        assert_eq!(summary(Some(&usage)), "4.9M tokens (4.9M cached)");
    }

    #[test]
    fn claude_deduplicates_message_ids_and_keeps_final_counters() {
        let message = serde_json::json!({"id":"a","role":"assistant","usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":30,"cache_creation_input_tokens":40}});
        let mut final_block = message.clone();
        final_block["usage"]["output_tokens"] = 8.into();
        let (_root, path) = transcript(
            "claude",
            &[
                message.clone(),
                message,
                final_block,
                serde_json::json!({"id":"b","role":"assistant","usage":{"input_tokens":1,"output_tokens":2}}),
            ],
        );
        assert_eq!(
            read_usage(&path, Path::new("/lane"), "claude").unwrap(),
            Some(Usage {
                input: 11,
                output: 10,
                cache_read: 30,
                cache_write: 40,
                reasoning: 0,
                total: 91
            })
        );
    }

    #[test]
    fn missing_malformed_wrong_lane_and_zero_usage_are_unknown() {
        let (_root, path) = transcript("pi", &[]);
        assert_eq!(read_usage(&path, Path::new("/lane"), "pi").unwrap(), None);
        assert!(read_usage(&path, Path::new("/other"), "pi").is_err());
        assert!(read_usage(&path.with_extension("missing"), Path::new("/lane"), "pi").is_err());
        std::fs::write(&path, "not json\n").unwrap();
        assert!(read_usage(&path, Path::new("/lane"), "pi").is_err());
        let (_root, path) = transcript(
            "pi",
            &[serde_json::json!({"role":"assistant","usage":{"totalTokens":0}})],
        );
        assert_eq!(read_usage(&path, Path::new("/lane"), "pi").unwrap(), None);
        assert_eq!(summary(None), "usage unknown");
    }

    #[test]
    fn collector_uses_exact_pi_session_and_claudes_latest_cwd_session() {
        let world = crate::scenarios::World::new();
        let (_root, path) = transcript(
            "pi",
            &[serde_json::json!({"role":"assistant","usage":{"input":5,"totalTokens":5}})],
        );
        let env = crate::paths::Env::for_test(
            world.home.path(),
            &[("PI_SESSION_FILE", path.to_str().unwrap())],
        );
        let mut ctx = world.ctx();
        ctx.env = &env;
        let mut lane = Thread {
            cwd: "/lane".into(),
            launch: crate::contracts::Launch {
                kind: "pi".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(collect(&ctx, &lane).unwrap().total, 5);
        lane.cwd = "/other".into();
        assert!(collect(&ctx, &lane).is_none());
        lane.cwd = "/lane".into();
        lane.launch.kind = "claude".into();
        let dir = world.home.path().join(".claude/projects/-lane");
        std::fs::create_dir_all(&dir).unwrap();
        let (_root, path) = transcript(
            "claude",
            &[serde_json::json!({"id":"a","role":"assistant","usage":{"input_tokens":9}})],
        );
        let text = std::fs::read_to_string(path).unwrap();
        std::fs::write(
            dir.join("old.jsonl"),
            text.replace("2026-10-03", "2026-10-02"),
        )
        .unwrap();
        std::fs::write(dir.join("new.jsonl"), &text).unwrap();
        assert_eq!(collect(&ctx, &lane).unwrap().total, 9);
        std::fs::write(dir.join("ambiguous.jsonl"), &text).unwrap();
        assert!(collect(&ctx, &lane).is_none());
    }
}
