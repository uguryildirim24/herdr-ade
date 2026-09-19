//! `herdr-pro`, the Pro bridge plugin (SPEC-pro-bridge v2).
//!
//! Pro is an ordinary Codex pane whose own Codex process points at the
//! installed `codex-chatgpt-web` bridge. The plugin never starts, restarts or
//! stops the bridge: it health-checks it, feeds Pro a packet, collects the
//! answer from the Codex rollout and types the D7-shaped DONE line.
//!
//! Like `src/pi/`, this module compiles into two targets (the `herdr-ade`
//! binary and the thin `herdr-pro` binary), so it uses no `crate::` paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

pub mod bridge;
pub mod doctor;
pub mod herdr_cli;
pub mod lane;
pub mod packet;
pub mod state;
pub mod turn;

/// The `Runner` seam is the pi module's, reused by path (SPEC-pi v2 §7).
#[path = "../pi/sh.rs"]
pub mod sh;

/// The launcher's port (spec §4, Q11). The terminal fallback uses 17941.
pub const BRIDGE_PORT: u16 = 17841;
pub const FALLBACK_PORT: u16 = 17941;
/// The routed Pro slug and the effort that fixes it (spec §1).
pub const MODEL: &str = "chatgpt-web/pro";
pub const EFFORT: &str = "ultra";
/// `!cat` output is delivered whole only with this override (spec §5, check a).
pub const TOOL_OUTPUT_TOKEN_LIMIT: u64 = 60_000;
/// Packet caps (spec §4).
pub const PACKET_MAX_BYTES: usize = 200 * 1024;
pub const PACKET_MAX_TOKENS: u64 = 60_000;
/// In flight: default 2, hard max 4 (upstream browser cap is 5; never 5).
pub const INFLIGHT_DEFAULT: usize = 2;
pub const INFLIGHT_MAX: usize = 4;
/// Turn timeout: Pro turns can last an hour (fold notes item 3).
pub const TURN_TIMEOUT: Duration = Duration::from_secs(120 * 60);
/// The packet `!cat` must show in the rollout this fast.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// The breaker (spec §4): 2 h, cleared only by Rolf through `resume-bridge`.
pub const COOLDOWN: Duration = Duration::from_secs(2 * 60 * 60);
/// Two failed turns inside this window trip the breaker.
pub const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Bridge majors the plugin has seen (spec Design, check 6).
pub const SEEN_BRIDGE_MAJORS: &[u64] = &[5];

/// The process environment, read once, so resolution never depends on plugin
/// variables that are not there.
#[derive(Debug, Clone)]
pub struct Env {
    vars: BTreeMap<String, String>,
    pub home: PathBuf,
}

impl Env {
    pub fn from_process() -> Result<Self> {
        let vars: BTreeMap<String, String> = std::env::vars().collect();
        let home = vars
            .get("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .context("HOME is not set")?;
        Ok(Env { vars, home })
    }

    #[cfg(test)]
    pub fn for_test(home: &Path, vars: &[(&str, &str)]) -> Self {
        Env {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            home: home.to_path_buf(),
        }
    }

    /// A variable's value; an empty value counts as unset.
    pub fn var(&self, key: &str) -> Option<&str> {
        self.vars
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    pub fn expand_tilde(&self, path: &str) -> PathBuf {
        sh::expand_tilde(path, &self.home)
    }

    /// The herdr binary: `HERDR_BIN_PATH` when set, else `herdr` on `PATH`.
    pub fn herdr_bin(&self) -> String {
        self.var("HERDR_BIN_PATH").unwrap_or("herdr").to_string()
    }

    /// The Codex home every Pro lane uses: Rolf's `~/.codex` in v1 (spec
    /// Design, "Codex home (v1)"). `-c` overrides only; nothing is written
    /// there. `CODEX_HOME` is never set by the plugin unless a Pro home is
    /// named.
    pub fn codex_home(&self) -> PathBuf {
        self.home.join(".codex")
    }

    /// The v2 shared Pro home, when Rolf names one. Empty for v1.
    pub fn pro_codex_home(&self) -> Option<PathBuf> {
        self.var("HERDR_PRO_CODEX_HOME")
            .map(|dir| self.expand_tilde(dir))
    }

    /// The home a lane's Codex actually uses.
    pub fn lane_codex_home(&self) -> PathBuf {
        self.pro_codex_home().unwrap_or_else(|| self.codex_home())
    }

    /// `CODEX_HOME=...` for a lane tab, only when a Pro home is named. The
    /// fork's launch-env persistence makes this survive a cold restart; until
    /// then `herdr-pro resume` passes it again.
    pub fn codex_home_env(&self) -> Option<String> {
        self.pro_codex_home()
            .map(|home| format!("CODEX_HOME={}", home.display()))
    }

    /// Concurrent Pro turns: two by default, configurable up to the hard cap
    /// of four.
    pub fn inflight_limit(&self) -> Result<usize> {
        let Some(raw) = self.var("HERDR_PRO_MAX_INFLIGHT") else {
            return Ok(INFLIGHT_DEFAULT);
        };
        let limit = raw
            .parse::<usize>()
            .with_context(|| format!("HERDR_PRO_MAX_INFLIGHT is not a number: {raw}"))?;
        if !(1..=INFLIGHT_MAX).contains(&limit) {
            anyhow::bail!(
                "HERDR_PRO_MAX_INFLIGHT must be between 1 and {INFLIGHT_MAX}, got {limit}"
            );
        }
        Ok(limit)
    }

    /// The bridge's own home: `CODEX_CHATGPT_WEB_HOME` else `~/.codex-chatgpt-web`.
    pub fn bridge_home(&self) -> PathBuf {
        match self.var("CODEX_CHATGPT_WEB_HOME") {
            Some(dir) => self.expand_tilde(dir),
            None => self.home.join(".codex-chatgpt-web"),
        }
    }
}

/// Every path the plugin owns, all under one root. `HERDR_PRO_STATE_DIR` is
/// the explicit override (tests and install checks); otherwise
/// `<ADE root>/pro-bridge`, the same root `herdr-pi` uses for `pi/`. An action
/// run and a coordinator shell therefore agree (the reason `pi::resolve_root`
/// ignores `HERDR_PLUGIN_STATE_DIR`).
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn from_env(env: &Env) -> Result<Layout> {
        Ok(Layout {
            root: resolve_root(env)?,
        })
    }

    #[cfg(test)]
    pub fn for_test(root: impl Into<PathBuf>) -> Layout {
        Layout { root: root.into() }
    }

    pub fn lanes(&self) -> PathBuf {
        self.root.join("lanes")
    }

    pub fn packets(&self) -> PathBuf {
        self.root.join("packets")
    }

    pub fn turns(&self) -> PathBuf {
        self.root.join("turns")
    }

    pub fn inflight(&self) -> PathBuf {
        self.root.join("inflight")
    }

    pub fn start_lock(&self) -> PathBuf {
        self.root.join("start.lock")
    }

    pub fn turn_lock(&self) -> PathBuf {
        self.root.join("turn.lock")
    }

    pub fn cooldown(&self) -> PathBuf {
        self.root.join("cooldown-until")
    }

    pub fn usage(&self) -> PathBuf {
        self.root.join("usage.jsonl")
    }

    pub fn bridge_state(&self) -> PathBuf {
        self.root.join("bridge-state.json")
    }

    pub fn lane(&self, name: &str) -> PathBuf {
        self.lanes().join(format!("{name}.toml"))
    }

    pub fn turn(&self, tag: &str) -> PathBuf {
        self.turns().join(format!("{tag}.toml"))
    }

    pub fn packet(&self, tag: &str) -> PathBuf {
        self.packets().join(format!("{tag}.md"))
    }

    pub fn inflight_lock(&self, tag: &str) -> PathBuf {
        self.inflight().join(format!("{tag}.lock"))
    }

    /// Create every directory the plugin writes into.
    pub fn ensure(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.lanes(),
            self.packets(),
            self.turns(),
            self.inflight(),
        ] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("could not create {}", dir.display()))?;
        }
        Ok(())
    }
}

/// The plugin root: `HERDR_PRO_STATE_DIR`, else `<ADE root>/pro-bridge`, where
/// the ADE root is `HERDR_ADE_ROOT`, then `root` in
/// `~/.config/herdr-ade/config.toml`, then `~/.herdr-ade`.
pub fn resolve_root(env: &Env) -> Result<PathBuf> {
    let root = if let Some(dir) = env.var("HERDR_PRO_STATE_DIR") {
        return absolute(&env.expand_tilde(dir));
    } else if let Some(dir) = env.var("HERDR_ADE_ROOT") {
        env.expand_tilde(dir)
    } else if let Some(root) = config_root(env)? {
        env.expand_tilde(&root)
    } else {
        env.home.join(".herdr-ade")
    };
    let root = absolute(&root)?;
    Ok(root.join("pro-bridge"))
}

fn config_root(env: &Env) -> Result<Option<String>> {
    let config = env
        .var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".config"));
    let path = config.join("herdr-ade/config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let table: toml::Table = text
        .parse()
        .with_context(|| format!("{} does not parse", path.display()))?;
    Ok(table
        .get("root")
        .and_then(toml::Value::as_str)
        .filter(|root| !root.is_empty())
        .map(str::to_string))
}

fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).with_context(|| format!("bad path {}", path.display()))
}

/// RFC 3339 now.
pub fn now_rfc3339() -> String {
    jiff::Timestamp::now().to_string()
}

/// Parse an RFC 3339 stamp the plugin wrote.
pub fn parse_rfc3339(text: &str) -> Option<jiff::Timestamp> {
    text.trim().parse().ok()
}

/// Seconds between a stamp and now; negative when the stamp is in the future.
pub fn seconds_since(text: &str, now: jiff::Timestamp) -> Option<i64> {
    let then = parse_rfc3339(text)?;
    Some(now.as_second() - then.as_second())
}

/// The default turn id when the coordinator does not pass `--id`:
/// `<lane>-<NN>`, numbered by the lane's earlier turns.
pub fn next_turn_id(layout: &Layout, lane: &str) -> String {
    let mut n = 0u32;
    if let Ok(entries) = std::fs::read_dir(layout.turns()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!("{lane}-")) && name.ends_with(".toml") {
                n += 1;
            }
        }
    }
    format!("{lane}-{:02}", n + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_shared_between_an_action_and_a_shell() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        // An action has HERDR_PLUGIN_STATE_DIR; a coordinator shell does not.
        let action = Env::for_test(home, &[("HERDR_PLUGIN_STATE_DIR", "/state/ade")]);
        let shell = Env::for_test(home, &[("HERDR_ADE_ROOT", "/root")]);
        assert_eq!(
            resolve_root(&action).unwrap(),
            home.join(".herdr-ade/pro-bridge")
        );
        assert_eq!(
            resolve_root(&shell).unwrap(),
            PathBuf::from("/root/pro-bridge")
        );
    }

    #[test]
    fn a_named_pro_home_changes_the_lane_home_and_the_tab_env() {
        let env = Env::for_test(Path::new("/h"), &[("HERDR_PRO_CODEX_HOME", "~/pro-codex")]);
        assert_eq!(env.lane_codex_home(), PathBuf::from("/h/pro-codex"));
        assert_eq!(
            env.codex_home_env().as_deref(),
            Some("CODEX_HOME=/h/pro-codex")
        );
        let env = Env::for_test(Path::new("/h"), &[]);
        assert_eq!(env.lane_codex_home(), PathBuf::from("/h/.codex"));
        assert_eq!(env.codex_home_env(), None);
    }

    #[test]
    fn explicit_state_dir_wins() {
        let env = Env::for_test(Path::new("/h"), &[("HERDR_PRO_STATE_DIR", "~/pro")]);
        assert_eq!(resolve_root(&env).unwrap(), PathBuf::from("/h/pro"));
    }

    #[test]
    fn turn_ids_count_earlier_turns() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path());
        layout.ensure().unwrap();
        assert_eq!(next_turn_id(&layout, "pro"), "pro-01");
        std::fs::write(layout.turn("pro-01"), "x").unwrap();
        assert_eq!(next_turn_id(&layout, "pro"), "pro-02");
    }
}
