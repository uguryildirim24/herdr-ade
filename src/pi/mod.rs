//! Pinned pi integration for `herdr-ade` (SPEC-pi v2 §3).
//!
//! The library owns pi itself: the pinned npm prefix, the wrapper the login
//! shell finds, one shared pi folder for every lane, the D2 rows and Jev
//! recipe ids, the priming row for the adapter table, resume helpers, doctor
//! rows, and the guard extension that turns a provider failure into
//! `blocked` / `WAITING` instead of a silent `done`.
//!
//! The module compiles into two targets: the `herdr-ade` binary and the thin
//! `herdr-pi` binary (setup, login instructions, doctor, check). It therefore
//! uses no `crate::` paths; [`sh`] is the external-command seam.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};

pub mod doctor;
pub mod folder;
pub mod install;
pub mod launch;
pub mod limits;
pub mod priming;
pub mod resume;
pub mod roles;
pub mod sh;

#[cfg(test)]
mod scenarios;

pub use doctor::CheckReport;

/// The pinned pi package. Never a caret range, never `npm install -g`
/// (SPEC-pi v2 §1, §3.2).
pub const PI_PACKAGE: &str = "@earendil-works/pi-coding-agent";
/// The exact pin. Doctor refuses anything else (SPEC-pi v2 §3.2, §3.9).
pub const PI_VERSION: &str = "0.85.1";
/// The package's `engines` floor (SPEC-pi v2 §1).
pub const MIN_NODE: (u32, u32, u32) = (22, 19, 0);
/// The guard extension's file name and marker (SPEC-pi v2 §3.9).
pub const GUARD_FILE: &str = "herdr-pi-guard.ts";
/// Version 2: the parent fallback runs whenever `ha waiting` did not.
pub const GUARD_MARKER: &str = "herdr-pi-guard:version=2";
/// The herdr state hook the running herdr writes (SPEC-pi v2 §3.3).
pub const HERDR_EXTENSION_FILE: &str = "herdr-agent-state.ts";

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
    pub fn for_test(home: &std::path::Path, vars: &[(&str, &str)]) -> Self {
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
}

/// Every path the pi library owns, all under one root
/// (SPEC-pi v2 §3.1, §3.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// `<ADE root>/pi`: `<HERDR_ADE_ROOT>/pi`, or `<root from
    /// config.toml>/pi`, or `~/.herdr-ade/pi`.
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

    /// The pinned npm prefix (`npm install --prefix <this>`).
    pub fn npm(&self) -> PathBuf {
        self.root.join("npm")
    }

    /// The wrapper a cold restore reaches through `~/.local/bin/pi`.
    pub fn wrapper(&self) -> PathBuf {
        self.root.join("bin").join("pi")
    }

    /// `PI_CODING_AGENT_DIR` for every lane.
    pub fn agent(&self) -> PathBuf {
        self.root.join("agent")
    }

    pub fn settings(&self) -> PathBuf {
        self.agent().join("settings.json")
    }

    pub fn models(&self) -> PathBuf {
        self.agent().join("models.json")
    }

    pub fn auth(&self) -> PathBuf {
        self.agent().join("auth.json")
    }

    pub fn trust(&self) -> PathBuf {
        self.agent().join("trust.json")
    }

    pub fn extensions(&self) -> PathBuf {
        self.agent().join("extensions")
    }

    pub fn guard(&self) -> PathBuf {
        self.extensions().join(GUARD_FILE)
    }

    pub fn herdr_extension(&self) -> PathBuf {
        self.extensions().join(HERDR_EXTENSION_FILE)
    }

    pub fn sessions(&self) -> PathBuf {
        self.agent().join("sessions")
    }

    /// Per-thread session dirs for the two-lanes-one-cwd case (§3.3).
    pub fn lanes(&self) -> PathBuf {
        self.root.join("lanes")
    }

    /// The pinned package folder inside the npm prefix.
    pub fn package(&self) -> PathBuf {
        self.npm().join("node_modules").join(PI_PACKAGE)
    }

    pub fn package_json(&self) -> PathBuf {
        self.package().join("package.json")
    }

    pub fn cli_js(&self) -> PathBuf {
        self.package().join("dist").join("bundle").join("cli.js")
    }
}

/// ADE's root, then one `pi/` below it: `HERDR_ADE_ROOT`, else `root` in
/// `~/.config/herdr-ade/config.toml`, else `~/.herdr-ade` (the order
/// `crate::paths::resolve_root` uses without a `--root` flag).
///
/// Not `HERDR_PLUGIN_STATE_DIR`: herdr sets it only for plugin actions, so
/// `herdr-pi setup` run as an action would install into one folder while
/// `ha thread start` in a coordinator shell, the ticker and `herdr-pi` from a
/// terminal check another, and every pi start would be refused.
pub fn resolve_root(env: &Env) -> Result<PathBuf> {
    let root = if let Some(dir) = env.var("HERDR_ADE_ROOT") {
        env.expand_tilde(dir)
    } else if let Some(root) = config_root(env)? {
        env.expand_tilde(&root)
    } else {
        env.home.join(".herdr-ade")
    };
    let root = std::path::absolute(&root)
        .with_context(|| format!("bad path {}", root.display()))?;
    Ok(root.join("pi"))
}

/// `root` from ADE's `config.toml`, when the file sets one.
fn config_root(env: &Env) -> Result<Option<String>> {
    let path = env.home.join(".config/herdr-ade/config.toml");
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

/// A read-only check before `herdr agent start`, from the process
/// environment. This is the entry A1 calls: `crate::pi::check(provider)`.
/// `Err` carries every failing row's text.
pub fn check(provider: &str) -> Result<CheckReport> {
    let env = Env::from_process()?;
    let layout = Layout::from_env(&env)?;
    let report = doctor::check_report(&env, &layout, &sh::RealRunner, provider);
    if report.ok {
        Ok(report)
    } else {
        anyhow::bail!("{}", report.error_text())
    }
}

/// The one-time login per provider (SPEC-pi v2 §2). The plugin prints these
/// steps; Rolf types `/login` inside pi, in the shared folder.
pub fn login_instructions() -> [(&'static str, &'static str, &'static str); 6] {
    [
        (
            "openai-codex",
            "ChatGPT Plus/Pro coding models (Astra, Sol)",
            "`/login`, pick \"ChatGPT Plus/Pro (Codex)\", finish in the browser or with the device code. This is the subscription, not an OpenAI API key.",
        ),
        (
            "opencode",
            "OpenCode Zen",
            "`/login`, pick \"Use an API key\", OpenCode, and paste the key. Zen serves grok-4.6, muse-spark-1.3 and more.",
        ),
        (
            "opencode-go",
            "OpenCode Go (the Muse row)",
            "`/login`, pick \"Use an API key\", OpenCode Go, and paste the Go plan's key. Go serves muse-spark-1.3-contributor.",
        ),
        (
            "deepseek",
            "DeepSeek",
            "`/login`, pick \"Use an API key\", DeepSeek, and paste the key. The direct key, not through Zen.",
        ),
        (
            "kimi-coding",
            "Kimi coding",
            "`/login`. Use the \"Kimi Code (subscription)\" device flow when the screen shows it; otherwise paste the key. Not moonshot.",
        ),
        (
            "xai",
            "Grok by SuperGrok or X Premium (optional)",
            "`/login xai`, pick \"Use a subscription\", and sign in. Workers skip Grok until Rolf enables this row.",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_the_ade_root_whoever_runs_it() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        // An action has HERDR_PLUGIN_STATE_DIR; a coordinator shell does not.
        // Both must land on the same folder.
        let action = Env::for_test(
            home,
            &[
                ("HERDR_PLUGIN_STATE_DIR", "/state/ade"),
                ("HERDR_ADE_ROOT", "/root"),
            ],
        );
        let shell = Env::for_test(home, &[("HERDR_ADE_ROOT", "/root")]);
        assert_eq!(resolve_root(&action).unwrap(), PathBuf::from("/root/pi"));
        assert_eq!(resolve_root(&shell).unwrap(), PathBuf::from("/root/pi"));
        let env = Env::for_test(home, &[("HERDR_ADE_ROOT", "~/r")]);
        assert_eq!(resolve_root(&env).unwrap(), home.join("r/pi"));
        let env = Env::for_test(home, &[("HERDR_PLUGIN_STATE_DIR", "/state/ade")]);
        assert_eq!(resolve_root(&env).unwrap(), home.join(".herdr-ade/pi"));
        std::fs::create_dir_all(home.join(".config/herdr-ade")).unwrap();
        std::fs::write(
            home.join(".config/herdr-ade/config.toml"),
            "root = \"~/from-config\"\n",
        )
        .unwrap();
        let env = Env::for_test(home, &[]);
        assert_eq!(resolve_root(&env).unwrap(), home.join("from-config/pi"));
    }

    #[test]
    fn layout_paths_are_one_shared_folder() {
        let layout = Layout::for_test("/p/pi");
        assert_eq!(
            layout.settings(),
            PathBuf::from("/p/pi/agent/settings.json")
        );
        assert_eq!(
            layout.guard(),
            PathBuf::from("/p/pi/agent/extensions/herdr-pi-guard.ts")
        );
        assert_eq!(layout.wrapper(), PathBuf::from("/p/pi/bin/pi"));
        assert_eq!(
            layout.cli_js(),
            PathBuf::from(
                "/p/pi/npm/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js"
            )
        );
    }
}
