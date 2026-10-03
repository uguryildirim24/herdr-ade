//! Root, config directory, herdr binary and socket resolution.
//!
//! Nothing here reads the process environment directly: callers pass an `Env`,
//! so resolution order is testable and never depends on plugin variables.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::herdr;
use crate::runner::Runner;

#[derive(Debug, Clone)]
pub(crate) struct Env {
    vars: BTreeMap<String, String>,
    pub(crate) home: PathBuf,
}

impl Env {
    pub(crate) fn from_process() -> Result<Self> {
        let vars: BTreeMap<String, String> = std::env::vars().collect();
        let home = vars
            .get("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .context("HOME is not set")?;
        Ok(Env { vars, home })
    }

    #[cfg(test)]
    pub(crate) fn for_test(home: &Path, vars: &[(&str, &str)]) -> Self {
        Env {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            home: home.to_path_buf(),
        }
    }

    /// A variable's value; an empty value counts as unset.
    pub(crate) fn var(&self, key: &str) -> Option<&str> {
        self.vars
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// Config directory: `$XDG_CONFIG_HOME/herdr-ade` when that variable is set
    /// (SPEC-ADE item 39), else `~/.config/herdr-ade`.
    pub(crate) fn config_dir(&self) -> PathBuf {
        let xdg = self
            .var("XDG_CONFIG_HOME")
            .map(|value| self.expand_tilde(value));
        crate::config::dir(&self.home, xdg.as_deref())
    }

    /// `HERDR_BIN_PATH` when set, else `herdr` on `PATH`.
    pub(crate) fn herdr_bin(&self) -> String {
        self.var("HERDR_BIN_PATH").unwrap_or("herdr").to_string()
    }

    fn expand_tilde(&self, path: &str) -> PathBuf {
        match path.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None if path == "~" => self.home.clone(),
            None => PathBuf::from(path),
        }
    }
}

/// What every subcommand works from: the environment, the resolved root and
/// config directory, and the runner all external commands go through.
pub(crate) struct Ctx<'a> {
    pub(crate) env: &'a Env,
    pub(crate) root: PathBuf,
    pub(crate) config_dir: PathBuf,
    pub(crate) runner: &'a dyn Runner,
    /// False in tests, so commands that ensure a ticker never spawn a process.
    pub(crate) detached_ticker: bool,
}

/// Projects root: `--root`, then `HERDR_ADE_ROOT`, then `root` in
/// `<config_dir>/config.toml`, then `~/.herdr-ade`.
pub(crate) fn resolve_root(flag: Option<&Path>, env: &Env, config_dir: &Path) -> Result<PathBuf> {
    if let Some(flag) = flag {
        return absolute(flag);
    }
    if let Some(var) = env.var("HERDR_ADE_ROOT") {
        return absolute(&env.expand_tilde(var));
    }
    let document = crate::config::Document::read(config_dir)?;
    if let Some(root) = document
        .value("root")
        .and_then(toml::Value::as_str)
        .filter(|root| !root.is_empty())
    {
        return absolute(&env.expand_tilde(root));
    }
    Ok(env.home.join(".herdr-ade"))
}

fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).with_context(|| format!("bad path {}", path.display()))
}

/// Which herdr session a command should talk to, as given on the command line.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SessionFlags {
    pub(crate) session: Option<String>,
    pub(crate) socket: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Session {
    pub(crate) socket: PathBuf,
    /// Known only when the session was chosen by name.
    pub(crate) name: Option<String>,
}

/// `--session`, then `--socket`, then `HERDR_SOCKET_PATH`, then `HERDR_SESSION`,
/// then herdr's default socket. A name is turned into a socket path by asking
/// herdr (`session list --json`), never by guessing herdr's directory layout.
pub(crate) fn resolve_session(
    flags: &SessionFlags,
    env: &Env,
    runner: &dyn Runner,
) -> Result<Session> {
    if flags.session.is_some() && flags.socket.is_some() {
        bail!("pass --session or --socket, not both");
    }
    if let Some(name) = &flags.session {
        return session_by_name(name, env, runner);
    }
    if let Some(socket) = &flags.socket {
        return Ok(Session {
            socket: absolute(socket)?,
            name: None,
        });
    }
    if let Some(socket) = env.var("HERDR_SOCKET_PATH") {
        return Ok(Session {
            socket: PathBuf::from(socket),
            name: None,
        });
    }
    if let Some(name) = env.var("HERDR_SESSION") {
        return session_by_name(name, env, runner);
    }
    let sessions = herdr::session_list(&env.herdr_bin(), runner).unwrap_or_default();
    let socket = sessions
        .into_iter()
        .find(|s| s.default)
        .map(|s| s.socket_path)
        .unwrap_or_else(|| env.home.join(".config/herdr/herdr.sock"));
    Ok(Session { socket, name: None })
}

fn session_by_name(name: &str, env: &Env, runner: &dyn Runner) -> Result<Session> {
    let sessions = herdr::session_list(&env.herdr_bin(), runner)?;
    match sessions.into_iter().find(|s| s.name == name) {
        Some(found) => Ok(Session {
            socket: found.socket_path,
            name: Some(name.to_string()),
        }),
        None => {
            bail!("herdr has no session named `{name}`; start it with `herdr --session {name}`")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, ok};

    const SESSIONS: &str = r#"{"sessions":[
        {"default":true,"name":"default","running":true,"session_dir":"/h/.config/herdr","socket_path":"/h/.config/herdr/herdr.sock"},
        {"default":false,"name":"hp-dev","running":true,"session_dir":"/h/.config/herdr/sessions/hp-dev","socket_path":"/h/.config/herdr/sessions/hp-dev/herdr.sock"}]}"#;

    #[test]
    fn root_config_that_does_not_parse_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), "root = [").unwrap();
        let env = Env::for_test(home.path(), &[]);
        assert!(resolve_root(None, &env, home.path()).is_err());
    }

    #[test]
    fn unknown_session_name_is_refused() {
        let runner = FakeRunner::new();
        runner.on("session list --json", ok(SESSIONS));
        let env = Env::for_test(Path::new("/h"), &[]);
        let flags = SessionFlags {
            session: Some("nope".into()),
            socket: None,
        };
        assert!(resolve_session(&flags, &env, &runner).is_err());
    }

    #[test]
    fn session_and_socket_together_are_refused() {
        let runner = FakeRunner::new();
        let env = Env::for_test(Path::new("/h"), &[]);
        let flags = SessionFlags {
            session: Some("a".into()),
            socket: Some("/b".into()),
        };
        assert!(resolve_session(&flags, &env, &runner).is_err());
    }
}
