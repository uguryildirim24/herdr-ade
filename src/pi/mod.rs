//! Pinned pi integration for `herdr-ade` (SPEC-pi v2 §3).
//!
//! The library owns pi itself: the pinned npm prefix, the wrapper the login
//! shell finds, one shared pi folder for every lane, the D2 rows,
//! doctor rows, and the guard extension that turns a provider failure
//! into `blocked` / `WAITING` instead of a silent `done`.
//!
//! Both command binaries link this module once through the shared library.

use std::path::PathBuf;

use anyhow::Result;

pub(crate) use crate::paths::Env;

pub(crate) mod cli;
pub(crate) mod doctor;
pub(crate) mod folder;
pub(crate) mod install;
pub(crate) mod launch;
pub(crate) mod provider;

/// The pinned pi package. Never a caret range, never `npm install -g`
/// (SPEC-pi v2 §1, §3.2).
pub(crate) const PI_PACKAGE: &str = "@earendil-works/pi-coding-agent";
/// The exact pin. Doctor refuses anything else (SPEC-pi v2 §3.2, §3.9).
pub(crate) const PI_VERSION: &str = "0.99.1";
/// The package's `engines` floor (SPEC-pi v2 §1).
pub(crate) const MIN_NODE: (u32, u32, u32) = (22, 19, 0);
/// The guard extension's file name (SPEC-pi v2 §3.9).
const GUARD_FILE: &str = "herdr-pi-guard.ts";

/// Every path the pi library owns, all under one root
/// (SPEC-pi v2 §3.1, §3.3).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layout {
    /// `<ADE root>/pi`: `<HERDR_ADE_ROOT>/pi`, or `<root from
    /// config.toml>/pi`, or `~/.herdr-ade/pi`.
    pub(crate) root: PathBuf,
}

impl Layout {
    pub(crate) fn from_env(env: &Env) -> Result<Layout> {
        Ok(Layout {
            root: resolve_root(env)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(root: impl Into<PathBuf>) -> Layout {
        Layout { root: root.into() }
    }

    /// The pinned npm prefix (`npm install --prefix <this>`).
    pub(crate) fn npm(&self) -> PathBuf {
        self.root.join("npm")
    }

    /// The wrapper a cold restore reaches through `~/.local/bin/pi`.
    pub(crate) fn wrapper(&self) -> PathBuf {
        self.root.join("bin").join("pi")
    }

    /// `PI_CODING_AGENT_DIR` for every lane.
    pub(crate) fn agent(&self) -> PathBuf {
        self.root.join("agent")
    }

    pub(crate) fn settings(&self) -> PathBuf {
        self.agent().join("settings.json")
    }

    pub(crate) fn models(&self) -> PathBuf {
        self.agent().join("models.json")
    }

    pub(crate) fn trust(&self) -> PathBuf {
        self.agent().join("trust.json")
    }

    pub(crate) fn extensions(&self) -> PathBuf {
        self.agent().join("extensions")
    }

    pub(crate) fn guard(&self) -> PathBuf {
        self.extensions().join(GUARD_FILE)
    }

    pub(crate) fn sessions(&self) -> PathBuf {
        self.agent().join("sessions")
    }

    /// Per-thread session dirs for the two-lanes-one-cwd case (§3.3).
    pub(crate) fn lanes(&self) -> PathBuf {
        self.root.join("lanes")
    }

    /// The pinned package folder inside the npm prefix.
    pub(crate) fn package(&self) -> PathBuf {
        self.npm().join("node_modules").join(PI_PACKAGE)
    }

    pub(crate) fn package_json(&self) -> PathBuf {
        self.package().join("package.json")
    }

    pub(crate) fn cli_js(&self) -> PathBuf {
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
pub(crate) fn resolve_root(env: &Env) -> Result<PathBuf> {
    Ok(crate::paths::resolve_root(None, env, &env.config_dir())?.join("pi"))
}

/// The one-time login per provider (SPEC-pi v2 §2). The plugin prints these
/// steps; Rolf types `/login` inside pi, in the shared folder.
pub(crate) fn login_instructions() -> [(&'static str, &'static str, &'static str); 3] {
    [
        (
            "openai-codex",
            "ChatGPT Plus/Pro coding models (Astra, Sol)",
            "`/login`, pick \"ChatGPT Plus/Pro (Codex)\", finish in the browser or with the device code. This is the subscription, not an OpenAI API key.",
        ),
        (
            "opencode-go",
            "OpenCode Go (the DeepSeek and Muse rows)",
            "`/login`, pick \"Use an API key\", OpenCode Go, and paste the Go plan's key. Go serves deepseek-v4.1-flash and muse-spark-1.3-contributor.",
        ),
        (
            "kimi-coding",
            "Kimi coding",
            "`/login`. Use the \"Kimi Code (subscription)\" device flow when the screen shows it; otherwise paste the key. Not moonshot.",
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
}
