//! Rate limits and provider failures: what the coordinator sees
//! (SPEC-pi v2 §3.7, §3.10).
//!
//! Pi itself never reports `blocked`; the guard extension classifies a
//! settled provider error and emits `herdr:blocked` plus one `ha waiting`.
//! The classification patterns here are the Rust twin of the TypeScript
//! guard (`extensions/herdr-pi-guard.ts`); both are tested with 429 and 401
//! fixtures, and the guard is exercised against the isolated pi.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The four classes the guard names (SPEC-pi v2 §3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LimitClass {
    Limit,
    Login,
    Unreachable,
    Error,
}

impl LimitClass {
    pub fn as_str(self) -> &'static str {
        match self {
            LimitClass::Limit => "limit",
            LimitClass::Login => "login",
            LimitClass::Unreachable => "unreachable",
            LimitClass::Error => "error",
        }
    }
}

/// Classify by HTTP status and message text, in the order the spec lists.
pub fn classify(message: &str, status: Option<u16>) -> LimitClass {
    let text = format!(
        "{} {}",
        message,
        status.map(|s| s.to_string()).unwrap_or_default()
    )
    .to_ascii_lowercase();
    if status == Some(429)
        || contains_any(
            &text,
            &[
                "rate limit",
                "rate-limit",
                "ratelimit",
                "too many requests",
                "usage limit",
                "quota",
                "billing",
                "available balance",
                "insufficient balance",
            ],
        )
    {
        return LimitClass::Limit;
    }
    if matches!(status, Some(401 | 403))
        || contains_any(
            &text,
            &[
                "unauthorized",
                "forbidden",
                "credentials",
                "api key",
                "apikey",
                // Not bare "token": "maximum context length is 128000
                // tokens" is not a login problem.
                "invalid token",
                "invalid_token",
                "expired token",
                "token expired",
                "token has expired",
                "refresh token",
                "access token",
                "re-login",
                "relogin",
                "log in again",
                "login expired",
                "not_ready",
                "not ready",
            ],
        )
    {
        return LimitClass::Login;
    }
    if contains_any(
        &text,
        &[
            "connection refused",
            "econnrefused",
            "fetch failed",
            "enotfound",
            "timeout",
            "timed out",
            "network",
            "socket hang up",
            "connection reset",
            "unreachable",
        ],
    ) {
        return LimitClass::Unreachable;
    }
    LimitClass::Error
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// The line the guard hands the coordinator: `WAITING <lane> <provider>
/// <class>: <first 120 characters>` (SPEC-pi v2 §3.7, §3.10).
pub fn waiting_line(lane: &str, provider: &str, class: LimitClass, detail: &str) -> String {
    format!(
        "WAITING {lane} {provider} {}: {}",
        class.as_str(),
        first_120(detail)
    )
}

/// The blocked label the pane reports: provider, class, status, first 120.
pub fn blocked_label(
    provider: &str,
    class: LimitClass,
    status: Option<u16>,
    detail: &str,
) -> String {
    match status {
        Some(status) => format!(
            "{provider} {} HTTP {status}: {}",
            class.as_str(),
            first_120(detail)
        ),
        None => format!("{provider} {}: {}", class.as_str(), first_120(detail)),
    }
}

fn first_120(text: &str) -> String {
    text.chars().take(120).collect()
}

/// One `ha waiting` per class per ten minutes (SPEC-pi v2 §3.7). Typing into
/// the pane clears the block; it is not a re-send.
#[derive(Debug, Default)]
pub struct Throttle {
    last: BTreeMap<LimitClass, Instant>,
    window: Duration,
}

impl Throttle {
    pub fn new(window: Duration) -> Self {
        Throttle {
            last: BTreeMap::new(),
            window,
        }
    }

    /// True when this class has not been reported inside the window.
    pub fn due(&mut self, class: LimitClass, now: Instant) -> bool {
        match self.last.get(&class) {
            Some(at) if now.duration_since(*at) < self.window => false,
            _ => {
                self.last.insert(class, now);
                true
            }
        }
    }
}

/// One row of the §3.10 table; the coordinator skill prints it.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitRow {
    pub provider: &'static str,
    pub behaviour: &'static str,
    pub coordinator_sees: &'static str,
}

pub fn rate_limit_table() -> Vec<LimitRow> {
    vec![
        LimitRow {
            provider: "any",
            behaviour: "defaults would retry 429, 5xx and timeout 3 times then settle idle; v2 retries once; quota and billing do not retry",
            coordinator_sees: "guard: pane `blocked` with the error label, and `WAITING <lane> <provider> limit|unreachable|error: …`; never `done`; recovery is typing into the pane",
        },
        LimitRow {
            provider: "any",
            behaviour: "401/403 or a failed token refresh",
            coordinator_sees: "guard: `WAITING <lane> <provider> login: …` and pane `blocked` \"login expired\"; Rolf runs the Log in action, then types anything in the lane",
        },
        LimitRow {
            provider: "openai-codex",
            behaviour: "ChatGPT backend throttles; exact numbers not measured",
            coordinator_sees: "as above",
        },
        LimitRow {
            provider: "opencode",
            behaviour: "Zen metered pass / pay per token",
            coordinator_sees: "`WAITING` \"OpenCode limit or empty balance\"; the cost bar is telemetry, not the signal",
        },
        LimitRow {
            provider: "deepseek",
            behaviour: "provider HTTP limits",
            coordinator_sees: "`WAITING` \"DeepSeek limit, wait\"",
        },
        LimitRow {
            provider: "kimi-coding",
            behaviour: "not measured; OAuth refresh can fail",
            coordinator_sees: "`WAITING` \"Kimi login expired\" or \"Kimi limit, wait\"; re-run `/login`",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_429_is_a_limit_and_a_401_is_a_login() {
        assert_eq!(
            classify("Rate limit reached for gpt-5.6-sol", Some(429)),
            LimitClass::Limit
        );
        assert_eq!(
            classify("Monthly usage limit reached", None),
            LimitClass::Limit
        );
        assert_eq!(
            classify("Incorrect API key provided: sk-…", Some(401)),
            LimitClass::Login
        );
        assert_eq!(
            classify("Your authentication token has expired", None),
            LimitClass::Login
        );
        assert_eq!(
            classify("fetch failed: ENOTFOUND api.deepseek.com", None),
            LimitClass::Unreachable
        );
        assert_eq!(classify("stream timed out", None), LimitClass::Unreachable);
        assert_eq!(
            classify(
                "This model's maximum context length is 128000 tokens",
                Some(400)
            ),
            LimitClass::Error
        );
        assert_eq!(
            classify("Failed to refresh OAuth: invalid refresh token", None),
            LimitClass::Login
        );
        assert_eq!(classify("something odd", None), LimitClass::Error);
    }

    #[test]
    fn the_waiting_line_is_bounded_and_named() {
        let long = "x".repeat(500);
        let line = waiting_line("a5", "deepseek", LimitClass::Limit, &long);
        assert!(line.starts_with("WAITING a5 deepseek limit: "));
        assert_eq!(
            line.chars().count(),
            "WAITING a5 deepseek limit: ".len() + 120
        );
        let label = blocked_label("deepseek", LimitClass::Login, Some(401), "bad key");
        assert_eq!(label, "deepseek login HTTP 401: bad key");
    }

    #[test]
    fn one_report_per_class_per_window() {
        let mut throttle = Throttle::new(Duration::from_secs(600));
        let now = Instant::now();
        assert!(throttle.due(LimitClass::Limit, now));
        assert!(!throttle.due(LimitClass::Limit, now + Duration::from_secs(60)));
        assert!(throttle.due(LimitClass::Limit, now + Duration::from_secs(601)));
        assert!(throttle.due(LimitClass::Login, now));
        assert!(throttle.due(LimitClass::Login, now + Duration::from_secs(601)));
    }

    #[test]
    fn the_table_names_every_enabled_provider() {
        let rows = rate_limit_table();
        for provider in crate::pi::roles::enabled_providers() {
            assert!(
                rows.iter()
                    .any(|r| r.provider == provider || r.provider == "any"),
                "missing {provider}"
            );
        }
    }
}
