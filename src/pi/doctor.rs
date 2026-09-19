//! `herdr-pi doctor` and `check <provider>` (SPEC-pi v2 §3.4, §3.9).
//!
//! One page, fail closed. Never opens a browser, never prints a secret, never
//! contacts a provider: `pi auth check --no-refresh` with stdin closed and a
//! short timeout. `npm root -g` runs inside `$SHELL -lic` like every other
//! login-shell probe.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::{Env, Layout, PI_VERSION, folder, install, launch, provider, roles, sh};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub level: Level,
    pub label: String,
    pub detail: String,
}

impl Row {
    pub fn ok(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            level: Level::Ok,
            label: label.into(),
            detail: detail.into(),
        }
    }

    pub fn warn(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            level: Level::Warn,
            label: label.into(),
            detail: detail.into(),
        }
    }

    pub fn fail(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            level: Level::Fail,
            label: label.into(),
            detail: detail.into(),
        }
    }

    /// `[ok  ] label: detail`, the plugin's doctor line shape.
    pub fn line(&self) -> String {
        let mark = match self.level {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        format!("[{mark}] {}: {}", self.label, self.detail)
    }
}

pub fn healthy(rows: &[Row]) -> bool {
    !rows.iter().any(|r| r.level == Level::Fail)
}

/// Run every row from the process environment (A1 wires this into
/// `herdr-ade doctor`). `Err` only when the environment cannot be read.
pub fn doctor_rows() -> Result<(Vec<Row>, bool)> {
    let env = Env::from_process()?;
    let layout = Layout::from_env(&env)?;
    let providers = roles::enabled_providers();
    let rows = doctor_rows_with(&env, &layout, &sh::RealRunner, &providers);
    let ok = healthy(&rows);
    Ok((rows, ok))
}

pub fn doctor_rows_with(
    env: &Env,
    layout: &Layout,
    runner: &dyn sh::Runner,
    providers: &[&str],
) -> Vec<Row> {
    let mut rows = Vec::new();

    // node
    match sh::login_shell(runner, "node --version") {
        Ok(output) if output.success() => {
            let version = sh::first_line(&output);
            if parse_node(&version).map(|v| v >= super::MIN_NODE) == Some(true) {
                rows.push(Row::ok("node", version));
            } else {
                rows.push(Row::fail(
                    "node",
                    format!(
                        "{version}; {} or later is required",
                        launch::min_node_string()
                    ),
                ));
            }
        }
        Ok(output) => rows.push(Row::fail("node", output.error_text())),
        Err(error) => rows.push(Row::fail("node", format!("{error:#}"))),
    }

    // npm
    match sh::login_shell(runner, "command -v npm") {
        Ok(output) if output.success() => {
            rows.push(Row::ok("npm", sh::first_line(&output)));
        }
        Ok(output) => rows.push(Row::fail("npm", output.error_text())),
        Err(error) => rows.push(Row::fail("npm", format!("{error:#}"))),
    }

    // pi version: the wrapper and the prefix pin must agree, exactly.
    let wrapper_version = if layout.wrapper().is_file() {
        match runner
            .run(&sh::Cmd::new(layout.wrapper().display().to_string(), sh::SHORT).arg("--version"))
        {
            Ok(output) if output.success() => Some(sh::first_line(&output)),
            Ok(output) => {
                rows.push(Row::fail(
                    "pi version",
                    format!("{}: {}", layout.wrapper().display(), output.error_text()),
                ));
                None
            }
            Err(error) => {
                rows.push(Row::fail(
                    "pi version",
                    format!("{}: {error:#}", layout.wrapper().display()),
                ));
                None
            }
        }
    } else {
        rows.push(Row::fail(
            "pi version",
            format!(
                "no wrapper at {}; run `herdr-pi setup`",
                layout.wrapper().display()
            ),
        ));
        None
    };
    if let Some(version) = &wrapper_version {
        let token = version.split_whitespace().last().unwrap_or(version);
        if token == PI_VERSION {
            rows.push(Row::ok("pi version", format!("wrapper {token}")));
        } else {
            rows.push(Row::fail(
                "pi version",
                format!("wrapper reports {token}, pinned {PI_VERSION}"),
            ));
        }
    }

    // pin: the prefix's package.json must hold the exact range, not a caret.
    match pinned_range(layout) {
        Some(range) if range == PI_VERSION => rows.push(Row::ok(
            "pin",
            format!("{range} in {}", layout.package_json().display()),
        )),
        Some(range) => rows.push(Row::fail(
            "pin",
            format!(
                "{range} in {}; the pin is exact 0.85.1",
                layout.package_json().display()
            ),
        )),
        None => {
            let installed =
                install::installed_version(layout).unwrap_or_else(|| "not installed".into());
            rows.push(Row::fail(
                "pin",
                format!(
                    "no exact pin in {} (installed: {installed})",
                    layout.npm().display()
                ),
            ));
        }
    }

    // wrapper on the login PATH: ~/.local/bin/pi, resolving to the wrapper,
    // and no earlier pi.
    rows.push(wrapper_path_row(runner, env, layout));

    // prefix isolation: the global npm root must not carry the package.
    // `npm root -g` prints the folder (for example
    // `/opt/homebrew/lib/node_modules`); the package is a folder inside it.
    match sh::login_shell(runner, "npm root -g") {
        Ok(output) if output.success() => {
            let global_root = sh::first_line(&output);
            if Path::new(&global_root).join(super::PI_PACKAGE).exists() {
                rows.push(Row::fail(
                    "prefix",
                    format!(
                        "a global npm root has {} ({}); the pin lives in the plugin prefix",
                        super::PI_PACKAGE,
                        global_root
                    ),
                ));
            } else if layout.package().is_dir() {
                rows.push(Row::ok(
                    "prefix",
                    format!(
                        "{} (global npm root: {global_root})",
                        layout.npm().display()
                    ),
                ));
            } else {
                rows.push(Row::fail(
                    "prefix",
                    format!(
                        "{} is missing; run `herdr-pi setup`",
                        layout.package().display()
                    ),
                ));
            }
        }
        Ok(output) => rows.push(Row::fail("prefix", output.error_text())),
        Err(error) => rows.push(Row::fail("prefix", format!("{error:#}"))),
    }

    // the shared pi folder
    if folder::exists(layout) {
        rows.push(Row::ok("pi folder", layout.agent().display().to_string()));
    } else {
        rows.push(Row::fail(
            "pi folder",
            format!(
                "{} is missing; run `herdr-pi setup`",
                layout.agent().display()
            ),
        ));
    }
    match folder::read_settings(layout) {
        Ok(state) => {
            if state.trust_never {
                rows.push(Row::ok("settings trust", "defaultProjectTrust: never"));
            } else {
                rows.push(Row::fail(
                    "settings trust",
                    format!(
                        "{} must set defaultProjectTrust: \"never\"",
                        layout.settings().display()
                    ),
                ));
            }
            if state.skills_disabled {
                rows.push(Row::ok("settings skills", "skills: [\"!**\"]"));
            } else {
                rows.push(Row::fail(
                    "settings skills",
                    format!(
                        "{} must set \"skills\": [\"!**\"] (pi 0.85.1 has no skills.enabled)",
                        layout.settings().display()
                    ),
                ));
            }
            if !state.retries_capped {
                rows.push(Row::warn(
                    "settings retry",
                    "retry.maxRetries 1 / provider.maxRetries 0 is the v2 brake",
                ));
            }
        }
        Err(error) => rows.push(Row::fail("settings", format!("{error:#}"))),
    }
    match folder::trust_has_true(layout) {
        Ok(true) => rows.push(Row::fail(
            "trust.json",
            "a true entry means repository pi code may run; remove it",
        )),
        Ok(false) => rows.push(Row::ok("trust.json", "no true entry")),
        Err(error) => rows.push(Row::fail("trust.json", format!("{error:#}"))),
    }

    // deepseek compaction: the shared models.json must lower the opencode-go
    // DeepSeek window so pi compacts near 372k tokens.
    match provider::missing_overrides(&layout.models()) {
        Ok(missing) if missing.is_empty() => rows.push(Row::ok(
            "deepseek compaction",
            format!("contextWindow {}", provider::DEEPSEEK_CONTEXT_WINDOW),
        )),
        Ok(missing) => rows.push(Row::fail(
            "deepseek compaction",
            format!(
                "{} misses contextWindow {}; run `herdr-pi setup`",
                missing.join(", "),
                provider::DEEPSEEK_CONTEXT_WINDOW
            ),
        )),
        Err(error) => rows.push(Row::fail("deepseek compaction", format!("{error:#}"))),
    }

    // the herdr state hook: the running herdr must call it current.
    let herdr_status = runner.run(
        &sh::Cmd::new(env.herdr_bin(), sh::SHORT)
            .args(["integration", "status"])
            .env("PI_CODING_AGENT_DIR", layout.agent().display().to_string()),
    );
    match herdr_status {
        Ok(output) if output.success() && output.stdout.contains("pi: current") => {
            rows.push(Row::ok("herdr extension", "pi: current"));
        }
        Ok(output) => rows.push(Row::fail(
            "herdr extension",
            format!(
                "{} integration status did not print `pi: current`: {}",
                env.herdr_bin(),
                output.error_text()
            ),
        )),
        Err(error) => rows.push(Row::fail("herdr extension", format!("{error:#}"))),
    }

    // the guard
    if install::guard_ok(layout) {
        rows.push(Row::ok(
            "guard",
            format!("{} with marker", layout.guard().display()),
        ));
    } else {
        rows.push(Row::fail(
            "guard",
            format!(
                "{} is missing or stale; run `herdr-pi setup`",
                layout.guard().display()
            ),
        ));
    }

    // Cursor: informational unless something Cursor-shaped is installed.
    let artifacts = cursor_artifacts(layout);
    if artifacts.is_empty() {
        rows.push(Row::ok(
            "Cursor",
            "outside pi; native cursor lanes only, then retired",
        ));
    } else {
        rows.push(Row::fail(
            "Cursor",
            format!("outside pi, but found {}", artifacts.join(", ")),
        ));
    }

    // each enabled provider: a login or a named missing-login failure.
    if layout.wrapper().is_file() {
        for provider in providers {
            // The `pro` relay only exists on the machine running the bridge,
            // so a missing provider row is informational. A `pro` provider
            // that is present but not logged in stays a failure.
            if *provider == "pro" && !provider::has_provider(&layout.models(), "pro") {
                rows.push(Row::ok("provider pro", "not on this machine (no relay)"));
                continue;
            }
            match auth_check(runner, layout, provider) {
                Ok(()) => rows.push(Row::ok(format!("provider {provider}"), "login ready")),
                Err(error) => rows.push(Row::fail(
                    format!("provider {provider}"),
                    format!("{error:#}"),
                )),
            }
        }
        if providers.contains(&"cursor") {
            rows.push(Row::fail("provider cursor", "no Cursor route under pi"));
        }
    } else {
        rows.push(Row::fail("providers", "no wrapper; run `herdr-pi setup`"));
    }

    // ~/.codex: the bridge base url must not leak into Codex (pro-bridge risk 2).
    rows.push(codex_row(env));

    // ~/.pi: existence only, never read further.
    if env.home.join(".pi").exists() {
        rows.push(Row::ok(
            "~/.pi",
            "present; informational, the harness never writes it",
        ));
    } else {
        rows.push(Row::ok("~/.pi", "not present"));
    }

    rows
}

/// The `--provider` a `check <provider>` call must refuse before any start.
pub fn check_provider_allowed(provider: &str) -> Result<()> {
    if provider.eq_ignore_ascii_case("cursor") {
        anyhow::bail!("pi_cursor_forbidden: Cursor stays outside pi (decision 18:30)");
    }
    if !launch::PROVIDERS.contains(&provider) {
        anyhow::bail!("pi_args_forbidden: `{provider}` is not a pi provider");
    }
    Ok(())
}

/// Read-only readiness for one provider (SPEC-pi v2 §3, §3.4): wrapper
/// identity, pin, trust setting, both extensions, `pi auth check`.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckReport {
    pub ok: bool,
    pub provider: String,
    pub rows: Vec<Row>,
}

impl CheckReport {
    pub fn failures(&self) -> Vec<&Row> {
        self.rows
            .iter()
            .filter(|r| r.level == Level::Fail)
            .collect()
    }

    pub fn error_text(&self) -> String {
        let mut text = format!("pi is not ready for `{}`", self.provider);
        for row in self.failures() {
            text.push_str(&format!("; {}: {}", row.label, row.detail));
        }
        text
    }

    /// The shape `herdr-pi check <provider>` prints and A1 records.
    pub fn json(&self) -> Value {
        let rows: Vec<Value> = self
            .rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "check": row.label,
                    "ok": row.level != Level::Fail,
                    "level": match row.level {
                        Level::Ok => "ok",
                        Level::Warn => "warn",
                        Level::Fail => "fail",
                    },
                    "detail": row.detail,
                })
            })
            .collect();
        serde_json::json!({
            "ok": self.ok,
            "provider": self.provider,
            "checks": rows,
        })
    }
}

/// The full `check <provider>`: everything A1 refuses a start on.
pub fn check_report(
    env: &Env,
    layout: &Layout,
    runner: &dyn sh::Runner,
    provider: &str,
) -> CheckReport {
    let mut rows = Vec::new();

    if let Err(error) = check_provider_allowed(provider) {
        rows.push(Row::fail("provider", format!("{error:#}")));
    }

    if layout.agent().is_dir() {
        rows.push(Row::ok("pi folder", layout.agent().display().to_string()));
    } else {
        rows.push(Row::fail(
            "pi folder",
            format!("{} is missing", layout.agent().display()),
        ));
    }

    match folder::read_settings(layout) {
        Ok(state) if state.trust_never => {
            rows.push(Row::ok("settings trust", "defaultProjectTrust: never"));
        }
        Ok(_) => rows.push(Row::fail(
            "settings trust",
            format!(
                "{} must set defaultProjectTrust: \"never\"",
                layout.settings().display()
            ),
        )),
        Err(error) => rows.push(Row::fail("settings", format!("{error:#}"))),
    }

    match folder::trust_has_true(layout) {
        Ok(false) => rows.push(Row::ok("trust.json", "no true entry")),
        Ok(true) => rows.push(Row::fail("trust.json", "a true entry is present")),
        Err(error) => rows.push(Row::fail("trust.json", format!("{error:#}"))),
    }

    if install::is_installed_exactly(layout) {
        rows.push(Row::ok(
            "pin",
            format!("{PI_VERSION} in {}", layout.npm().display()),
        ));
    } else {
        let found = install::installed_version(layout).unwrap_or_else(|| "missing".into());
        rows.push(Row::fail(
            "pin",
            format!("installed pi is {found}, pinned {PI_VERSION}"),
        ));
    }

    rows.push(wrapper_path_row(runner, env, layout));

    if install::guard_ok(layout) {
        rows.push(Row::ok("guard", "present with marker"));
    } else {
        rows.push(Row::fail(
            "guard",
            "extensions/herdr-pi-guard.ts is missing or stale",
        ));
    }

    match runner.run(
        &sh::Cmd::new(env.herdr_bin(), sh::SHORT)
            .args(["integration", "status"])
            .env("PI_CODING_AGENT_DIR", layout.agent().display().to_string()),
    ) {
        Ok(output) if output.success() && output.stdout.contains("pi: current") => {
            rows.push(Row::ok("herdr extension", "pi: current"));
        }
        Ok(output) => rows.push(Row::fail(
            "herdr extension",
            format!("herdr integration status: {}", output.error_text()),
        )),
        Err(error) => rows.push(Row::fail("herdr extension", format!("{error:#}"))),
    }

    if check_provider_allowed(provider).is_ok() {
        if layout.wrapper().is_file() {
            match auth_check(runner, layout, provider) {
                Ok(()) => rows.push(Row::ok("login", format!("{provider} ready"))),
                Err(error) => rows.push(Row::fail("login", format!("{error:#}"))),
            }
        } else {
            rows.push(Row::fail(
                "login",
                format!("no wrapper at {}", layout.wrapper().display()),
            ));
        }
    }

    let ok = healthy(&rows);
    CheckReport {
        ok,
        provider: provider.to_string(),
        rows,
    }
}

/// Do not start a lane A1 will have to kill: refuse with the failing rows.
pub fn check_with(
    layout: &Layout,
    env: &Env,
    runner: &dyn sh::Runner,
    provider: &str,
) -> Result<CheckReport> {
    let report = check_report(env, layout, runner, provider);
    if report.ok {
        Ok(report)
    } else {
        anyhow::bail!("{}", report.error_text())
    }
}

fn auth_check(runner: &dyn sh::Runner, layout: &Layout, provider: &str) -> Result<()> {
    let output = runner.run(
        &sh::Cmd::new(layout.wrapper().display().to_string(), sh::SHORT)
            .args([
                "auth",
                "check",
                "--provider",
                provider,
                "--json",
                "--no-refresh",
            ])
            .env("PI_CODING_AGENT_DIR", layout.agent().display().to_string()),
    )?;
    let parsed: Value = serde_json::from_str(output.stdout.trim()).map_err(|_| {
        anyhow::anyhow!(
            "auth check did not answer JSON (exit {}): {}",
            output
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".into()),
            output.error_text()
        )
    })?;
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("");
    if status == "ready" && output.success() {
        return Ok(());
    }
    let reason = parsed
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or(status);
    anyhow::bail!("missing login: {reason} (run `herdr-pi login {provider}`)")
}

fn wrapper_path_row(runner: &dyn sh::Runner, env: &Env, layout: &Layout) -> Row {
    wrapper_path_row_with(runner, env, layout, &sh::shell())
}

/// The selected probe and parse, with the login shell named so the tests can
/// drive `/bin/bash` and `/bin/zsh` on any host.
fn wrapper_path_row_with(runner: &dyn sh::Runner, env: &Env, layout: &Layout, shell: &str) -> Row {
    let link = env.home.join(".local/bin/pi");
    // `whence` is zsh and `type -a` is bash, so the machine's own login shell
    // picks its own word; anything else gets POSIX `command -v`.
    let script = path_probe(shell);
    let output = match runner.run(&sh::Cmd::new(shell, sh::SHORT).args(["-lic", script])) {
        Ok(output) => output,
        Err(error) => return Row::fail("wrapper on PATH", format!("{error:#}")),
    };
    let first = first_pi_path(&output);
    if first != link.display().to_string() {
        return Row::fail(
            "wrapper on PATH",
            format!(
                "`$SHELL -lic '{script}'` finds `{first}` first; expected {}",
                link.display()
            ),
        );
    }
    if !link.exists() {
        return Row::fail(
            "wrapper on PATH",
            format!(
                "{} does not exist; run `{}`",
                link.display(),
                install::link_line(layout, &env.home)
            ),
        );
    }
    match std::fs::canonicalize(&link) {
        Ok(target) if target == canonical_or_self(&layout.wrapper()) => Row::ok(
            "wrapper on PATH",
            format!("{} -> {}", link.display(), target.display()),
        ),
        Ok(target) => Row::fail(
            "wrapper on PATH",
            format!(
                "{} points at {}, not {}",
                link.display(),
                target.display(),
                layout.wrapper().display()
            ),
        ),
        Err(error) => Row::fail(
            "wrapper on PATH",
            format!("{} does not resolve: {error}", link.display()),
        ),
    }
}

/// The login shell's own "where is this word" builtin.
fn path_probe(shell: &str) -> &'static str {
    match Path::new(shell).file_name().and_then(|name| name.to_str()) {
        Some("bash") => "type -a pi",
        Some("zsh") => "whence -va pi",
        _ => "command -v pi",
    }
}

/// The first path a probe names, from stdout or stderr. `type -a` and
/// `whence -va` print `pi is /path`; `command -v` prints the bare path. A
/// login rc file may print its own lines, so a `pi is ...` line must name an
/// absolute path and a bare line must itself be the `pi` path.
fn first_pi_path(output: &sh::Output) -> String {
    for line in output.stdout.lines().chain(output.stderr.lines()) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pi is ") {
            let path = rest.trim();
            if path.starts_with('/') {
                return path.to_string();
            }
        } else if Path::new(line).file_name().and_then(|name| name.to_str()) == Some("pi") {
            return line.to_string();
        }
    }
    String::new()
}

fn pinned_range(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.npm().join("package.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value
        .get("dependencies")
        .and_then(|d| d.get(super::PI_PACKAGE))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn cursor_artifacts(layout: &Layout) -> Vec<String> {
    let mut found = Vec::new();
    if layout.npm().join("node_modules/@cursor/sdk").exists() {
        found.push("@cursor/sdk".to_string());
    }
    let modules = layout.npm().join("node_modules");
    if let Ok(entries) = std::fs::read_dir(&modules) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("pi-cursor") {
                found.push(name.clone());
            }
            if name.starts_with('@')
                && let Ok(inner) = std::fs::read_dir(entry.path())
            {
                for item in inner.flatten() {
                    let scoped = format!("{name}/{}", item.file_name().to_string_lossy());
                    if scoped.to_ascii_lowercase().contains("pi-cursor") || scoped == "@cursor/sdk"
                    {
                        found.push(scoped);
                    }
                }
            }
        }
    }
    for ext in [
        layout.extensions().join("herdr-pi-cursor.ts"),
        layout.extensions().join("pi-cursor.ts"),
    ] {
        if ext.exists() {
            found.push(ext.display().to_string());
        }
    }
    found
}

fn codex_row(env: &Env) -> Row {
    let path = env.home.join(".codex/config.toml");
    if !path.exists() {
        return Row::ok("~/.codex", "no config.toml");
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Row::warn("~/.codex", format!("{} is unreadable", path.display()));
    };
    let Ok(value) = text.parse::<toml::Table>() else {
        return Row::warn("~/.codex", format!("{} does not parse", path.display()));
    };
    let base = value.get("openai_base_url").and_then(|v| v.as_str());
    match base {
        Some(url) if url.contains("127.0.0.1:17841") => Row::fail(
            "~/.codex",
            format!("openai_base_url points at the bridge ({url}); remove it (pro-bridge risk 2)"),
        ),
        Some(url) => Row::ok("~/.codex", format!("openai_base_url {url}")),
        None => Row::ok("~/.codex", "no openai_base_url override"),
    }
}

fn parse_node(version: &str) -> Option<(u32, u32, u32)> {
    let v = version.trim().trim_start_matches('v');
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi::sh::fake::{FakeRunner, fail, ok};

    fn scripted(env: &Env) -> FakeRunner {
        let link = env.home.join(".local/bin/pi");
        let runner = FakeRunner::new();
        runner.on("zsh -lic node --version", ok("v22.19.0\n"));
        runner.on("zsh -lic command -v npm", ok("/opt/homebrew/bin/npm\n"));
        runner.on(
            "zsh -lic npm root -g",
            ok(&format!(
                "{}\n",
                env.home.join("global/node_modules").display()
            )),
        );
        runner.on(
            "zsh -lic whence -va pi",
            ok(&format!("pi is {}\n", link.display())),
        );
        runner.on("herdr integration status", ok("pi: current\n"));
        runner.on("--version", ok("0.85.1\n"));
        runner
    }

    fn installed_layout(dir: &Path) -> Layout {
        let layout = Layout::for_test(dir.join("pi"));
        folder::ensure(&layout).unwrap();
        crate::pi::provider::write_overrides(&layout.models()).unwrap();
        install::write_guard(&layout).unwrap();
        crate::pi::launch::write_wrapper(&layout).unwrap();
        std::fs::create_dir_all(layout.package().join("dist/bundle")).unwrap();
        std::fs::write(layout.package_json(), r#"{"version":"0.85.1"}"#).unwrap();
        std::fs::write(layout.cli_js(), "// cli").unwrap();
        std::fs::write(
            layout.npm().join("package.json"),
            r#"{"dependencies":{"@earendil-works/pi-coding-agent":"0.85.1"}}"#,
        )
        .unwrap();
        layout
    }

    /// The link `~/.local/bin/pi` a real setup asks Rolf to make.
    fn link_into(env: &Env, layout: &Layout) {
        std::fs::create_dir_all(env.home.join(".local/bin")).unwrap();
        std::os::unix::fs::symlink(layout.wrapper(), env.home.join(".local/bin/pi")).unwrap();
    }

    /// Merge the `pro` relay provider into the shared `models.json`.
    fn add_pro(layout: &Layout) {
        let text = std::fs::read_to_string(layout.models()).unwrap();
        let mut value: Value = serde_json::from_str(&text).unwrap();
        value["providers"]["pro"] = serde_json::json!({"apiKey": "tok"});
        std::fs::write(layout.models(), serde_json::to_string(&value).unwrap()).unwrap();
    }

    #[test]
    fn a_good_install_passes_every_scripted_row() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        link_into(&env, &layout);
        let runner = scripted(&env);
        let rows = doctor_rows_with(&env, &layout, &runner, &[]);
        let text: Vec<String> = rows.iter().map(Row::line).collect();
        assert!(
            text.iter().any(|l| l.contains("[ok  ] node: v22.19.0")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("[ok  ] pin: 0.85.1")),
            "{text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("[ok  ] wrapper on PATH")),
            "{text:?}"
        );
        assert!(text.iter().any(|l| l.contains("[ok  ] guard")), "{text:?}");
        assert!(text.iter().any(|l| l.contains("[ok  ] Cursor")), "{text:?}");
        assert!(!text.iter().any(|l| l.contains("[FAIL]")), "{text:?}");
        assert!(healthy(&rows));
    }

    #[test]
    fn login_shell_chatter_is_not_a_pi_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[]);
        link_into(&env, &layout);
        let link = env.home.join(".local/bin/pi");
        let runner = FakeRunner::new();
        runner.on(
            "zsh -lic whence -va pi",
            ok(&format!(
                "fnm: this shell is ready\npi is {}\n",
                link.display()
            )),
        );
        assert_eq!(wrapper_path_row(&runner, &env, &layout).level, Level::Ok);
        let runner = FakeRunner::new();
        runner.on(
            "zsh -lic whence -va pi",
            ok(&format!(
                "pi is /opt/homebrew/bin/pi\npi is {}\n",
                link.display()
            )),
        );
        assert_eq!(wrapper_path_row(&runner, &env, &layout).level, Level::Fail);
    }

    #[test]
    fn the_probe_uses_the_login_shells_own_word() {
        assert_eq!(path_probe("/bin/bash"), "type -a pi");
        assert_eq!(path_probe("/bin/zsh"), "whence -va pi");
        assert_eq!(path_probe("zsh"), "whence -va pi");
        assert_eq!(path_probe("/usr/bin/fish"), "command -v pi");
    }

    #[test]
    fn the_probe_parse_takes_the_first_path() {
        let out = |stdout: &str, stderr: &str| sh::Output {
            code: Some(0),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            timed_out: false,
        };
        // bash `type -a pi`
        assert_eq!(
            first_pi_path(&out("pi is /one/pi\npi is /two/pi\n", "")),
            "/one/pi"
        );
        // zsh `whence -va pi`
        assert_eq!(
            first_pi_path(&out("pi is /one/pi\npi is /two/pi\n", "")),
            "/one/pi"
        );
        // POSIX `command -v pi` prints the bare path
        assert_eq!(first_pi_path(&out("/three/pi\n", "")), "/three/pi");
        // rc chatter and a function are not a resolution
        assert_eq!(
            first_pi_path(&out(
                "fnm: this shell is ready\n/etc/profile.d/x.sh: ready\npi is a function\npi is /four/pi\n",
                ""
            )),
            "/four/pi"
        );
        // stderr counts too
        assert_eq!(first_pi_path(&out("", "pi is /five/pi\n")), "/five/pi");
        assert_eq!(first_pi_path(&out("", "")), "");
    }

    #[test]
    fn the_wrapper_row_probes_bash_on_a_bash_host() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[]);
        link_into(&env, &layout);
        let link = env.home.join(".local/bin/pi");
        let runner = FakeRunner::new();
        runner.on(
            "bash -lic type -a pi",
            ok(&format!("pi is {}\n", link.display())),
        );
        let row = wrapper_path_row_with(&runner, &env, &layout, "/bin/bash");
        assert_eq!(row.level, Level::Ok, "{row:?}");
        assert_eq!(runner.count("type -a pi"), 1);
        assert_eq!(runner.count("whence"), 0);
    }

    #[test]
    fn the_pro_row_is_informational_when_the_relay_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = scripted(&env);
        let rows = doctor_rows_with(&env, &layout, &runner, &["pro"]);
        let row = rows.iter().find(|r| r.label == "provider pro").unwrap();
        assert_eq!(row.level, Level::Ok, "{row:?}");
        assert_eq!(row.detail, "not on this machine (no relay)");
        assert_eq!(runner.count("auth check"), 0);
    }

    #[test]
    fn a_present_pro_provider_still_needs_a_login() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        add_pro(&layout);

        let runner = scripted(&env);
        runner.on("auth check --provider pro", ok(r#"{"status":"ready"}"#));
        let rows = doctor_rows_with(&env, &layout, &runner, &["pro"]);
        let row = rows.iter().find(|r| r.label == "provider pro").unwrap();
        assert_eq!(row.level, Level::Ok, "{row:?}");
        assert_eq!(row.detail, "login ready");

        let runner = scripted(&env);
        runner.on(
            "auth check --provider pro",
            fail(
                1,
                r#"{"status":"not_ready","reason":"credentials_not_configured"}"#,
            ),
        );
        let rows = doctor_rows_with(&env, &layout, &runner, &["pro"]);
        let row = rows.iter().find(|r| r.label == "provider pro").unwrap();
        assert_eq!(row.level, Level::Fail, "{row:?}");
        assert!(row.detail.contains("credentials_not_configured"), "{row:?}");
    }

    #[test]
    fn a_caret_pin_and_a_true_trust_entry_fail() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        std::fs::write(
            layout.npm().join("package.json"),
            r#"{"dependencies":{"@earendil-works/pi-coding-agent":"^0.85.1"}}"#,
        )
        .unwrap();
        std::fs::write(layout.trust(), "{\"/repo\":true}\n").unwrap();
        let rows = doctor_rows_with(&env, &layout, &scripted(&env), &[]);
        let text: Vec<String> = rows.iter().map(Row::line).collect();
        assert!(text.iter().any(|l| l.contains("[FAIL] pin")), "{text:?}");
        assert!(
            text.iter().any(|l| l.contains("[FAIL] trust.json")),
            "{text:?}"
        );
    }

    #[test]
    fn a_missing_login_is_a_named_failure() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = scripted(&env);
        runner.on(
            "auth check --provider kimi-coding",
            ok(r#"{"status":"not_ready","reason":"credentials_not_configured"}"#),
        );
        runner.on(
            "auth check --provider opencode-go",
            ok(r#"{"status":"ready"}"#),
        );
        let rows = doctor_rows_with(&env, &layout, &runner, &["kimi-coding", "opencode-go"]);
        let text: Vec<String> = rows.iter().map(Row::line).collect();
        assert!(
            text.iter()
                .any(|l| l.contains("[FAIL] provider kimi-coding")
                    && l.contains("credentials_not_configured")),
            "{text:?}"
        );
        assert!(
            text.iter()
                .any(|l| l.contains("[ok  ] provider opencode-go")),
            "{text:?}"
        );
    }

    /// T9: a global install of the package is a failure; the root's path
    /// string never contains the package name, so the folder is what counts.
    #[test]
    fn a_global_install_of_the_package_fails_the_prefix_row() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path(), &[]);
        let layout = installed_layout(dir.path());
        link_into(&env, &layout);
        let runner = scripted(&env);
        let prefix = |runner: &FakeRunner| {
            doctor_rows_with(&env, &layout, runner, &[])
                .into_iter()
                .find(|r| r.label == "prefix")
                .unwrap()
        };
        assert_eq!(prefix(&runner).level, Level::Ok);
        std::fs::create_dir_all(
            env.home
                .join("global/node_modules")
                .join(crate::pi::PI_PACKAGE),
        )
        .unwrap();
        let row = prefix(&runner);
        assert_eq!(row.level, Level::Fail, "{row:?}");
        assert!(row.detail.contains("global npm root"));
    }

    #[test]
    fn a_cursor_package_fails_the_cursor_row() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        std::fs::create_dir_all(layout.npm().join("node_modules/@cursor/sdk")).unwrap();
        std::fs::create_dir_all(layout.npm().join("node_modules/pi-cursor-sdk")).unwrap();
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let rows = doctor_rows_with(&env, &layout, &scripted(&env), &[]);
        let text: Vec<String> = rows.iter().map(Row::line).collect();
        let cursor = text.iter().find(|l| l.contains("Cursor")).unwrap();
        assert!(
            cursor.contains("[FAIL]") && cursor.contains("@cursor/sdk"),
            "{cursor}"
        );
    }

    #[test]
    fn the_codex_bridge_url_is_a_failure_and_an_absent_file_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path(), &[]);
        assert_eq!(codex_row(&env).level, Level::Ok);
        std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
        std::fs::write(
            dir.path().join(".codex/config.toml"),
            "openai_base_url = \"http://127.0.0.1:17841/v1\"\n",
        )
        .unwrap();
        assert_eq!(codex_row(&env).level, Level::Fail);
    }

    #[test]
    fn the_deepseek_compaction_row_is_ok_after_setup_and_names_a_missing_model() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        link_into(&env, &layout);
        let row = |layout: &Layout| {
            doctor_rows_with(&env, layout, &scripted(&env), &[])
                .into_iter()
                .find(|r| r.label == "deepseek compaction")
                .unwrap()
        };
        assert_eq!(row(&layout).level, Level::Ok);
        std::fs::write(layout.models(), "{\"providers\":{}}\n").unwrap();
        let missing = row(&layout);
        assert_eq!(missing.level, Level::Fail, "{missing:?}");
        assert!(
            missing.detail.contains("deepseek-v4.1-flash"),
            "{missing:?}"
        );
    }

    #[test]
    fn check_refuses_cursor_and_unknown_providers() {
        assert!(check_provider_allowed("cursor").is_err());
        assert!(check_provider_allowed("moonshot").is_err());
        assert!(check_provider_allowed("kimi-coding").is_ok());
    }

    #[test]
    fn check_report_is_json_with_a_failure_list() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = scripted(&env);
        runner.on(
            "auth check --provider kimi-coding",
            fail(
                1,
                r#"{"status":"not_ready","reason":"credentials_not_configured"}"#,
            ),
        );
        let report = check_report(&env, &layout, &runner, "kimi-coding");
        assert!(!report.ok);
        assert!(report.error_text().contains("not ready"));
        let json = report.json();
        assert_eq!(json["ok"], Value::Bool(false));
        assert_eq!(json["provider"], "kimi-coding");
        assert!(report.failures().iter().any(|r| r.label == "login"));
    }

    #[test]
    fn node_versions_compare_against_the_floor() {
        assert_eq!(parse_node("v22.19.0"), Some((22, 19, 0)));
        assert_eq!(parse_node("22.19"), Some((22, 19, 0)));
        assert!(parse_node("v22.18.9").unwrap() < super::super::MIN_NODE);
        assert!(parse_node("v23.0.0").unwrap() >= super::super::MIN_NODE);
        assert_eq!(parse_node("garbage"), None);
    }
}
