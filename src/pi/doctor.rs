//! `herdr-pi doctor` and `check <provider>` (SPEC-pi v2 §3.4, §3.9).
//!
//! One page, fail closed. Never opens a browser or prints a secret. Provider
//! readiness makes one tiny, tool-free model call because a stored credential
//! does not prove that the subscription still works. Results are cached for
//! one ticker interval. `npm root -g` runs inside `$SHELL -lic` like every
//! other login-shell probe.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::Value;

use super::{Env, Layout, PI_VERSION, folder, install, launch, provider};
use crate::runner as sh;

// A healthy Mac Codex print-mode probe has exceeded 10s; leave room for
// normal provider latency without treating a real refusal as ready.
const LIVE_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const READINESS_CACHE_TTL: Duration = Duration::from_secs(15);

fn pi_model(
    config: &crate::launch::LaunchConfig,
    id: &str,
    recipe: &crate::contracts::Recipe,
) -> Result<Option<String>> {
    let adapter = config
        .adapters
        .get(&recipe.kind)
        .with_context(|| format!("adapter_unknown: recipe `{id}` uses `{}`", recipe.kind))?;
    if adapter.doctor.readiness != "pi" {
        return Ok(None);
    }
    crate::adapters::validate_recipe(adapter, id, recipe)?;
    Ok(launch::flag_value(&recipe.args, "--model"))
}

/// Provider/model pairs used by enabled routes in the canonical recipe file.
pub(crate) fn configured_routed_models(config_dir: &Path) -> Result<Vec<(String, String)>> {
    let config = crate::launch::parse_launch_config(config_dir)?;
    let mut models = BTreeSet::new();
    for id in config.routing.recipe_ids() {
        let recipe = &config.recipes[id];
        if let Some(model) = pi_model(&config, id, recipe)? {
            models.insert((recipe.provider.clone(), model));
        }
    }
    Ok(models.into_iter().collect())
}

pub(crate) fn configured_deepseek_models(config_dir: &Path) -> Result<Vec<String>> {
    let config = crate::launch::recipe_catalog(config_dir)?;
    let mut models = BTreeSet::new();
    for (id, recipe) in &config.recipes {
        if let Some(model) = pi_model(&config, id, recipe)?
            && recipe.provider == provider::PROVIDER_ID
            && model.starts_with("deepseek")
        {
            models.insert(model);
        }
    }
    Ok(models.into_iter().collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum FailureEvidence {
    Unknown,
    Provider,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Row {
    pub(crate) level: Level,
    pub(crate) label: String,
    pub(crate) detail: String,
    pub(crate) evidence: FailureEvidence,
}

impl Row {
    pub(crate) fn ok(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            level: Level::Ok,
            label: label.into(),
            detail: detail.into(),
            evidence: FailureEvidence::Unknown,
        }
    }

    pub(crate) fn warn(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            level: Level::Warn,
            label: label.into(),
            detail: detail.into(),
            evidence: FailureEvidence::Unknown,
        }
    }

    pub(crate) fn fail(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            level: Level::Fail,
            label: label.into(),
            detail: detail.into(),
            evidence: FailureEvidence::Unknown,
        }
    }

    fn provider_fail(label: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            level: Level::Fail,
            label: label.into(),
            detail: detail.into(),
            evidence: FailureEvidence::Provider,
        }
    }

    /// `[ok  ] label: detail`, the plugin's doctor line shape.
    pub(crate) fn line(&self) -> String {
        let mark = match self.level {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        format!("[{mark}] {}: {}", self.label, self.detail)
    }
}

pub(crate) fn healthy(rows: &[Row]) -> bool {
    !rows.iter().any(|r| r.level == Level::Fail)
}

pub(crate) fn failure_evidence<'a>(mut failures: impl Iterator<Item = &'a Row>) -> FailureEvidence {
    match failures.next() {
        Some(first)
            if first.evidence == FailureEvidence::Provider
                && failures.all(|row| row.evidence == FailureEvidence::Provider) =>
        {
            FailureEvidence::Provider
        }
        _ => FailureEvidence::Unknown,
    }
}

/// Run every row from the process environment (A1 wires this into
/// `herdr-ade doctor`). `Err` only when the environment cannot be read.
pub(crate) fn doctor_rows() -> Result<(Vec<Row>, bool)> {
    let env = Env::from_process()?;
    let layout = Layout::from_env(&env)?;
    let rows = match configured_routed_models(&env.config_dir()) {
        Ok(models) => {
            let borrowed: Vec<(&str, &str)> = models
                .iter()
                .map(|(provider, model)| (provider.as_str(), model.as_str()))
                .collect();
            doctor_rows_with_models(&env, &layout, &sh::RealRunner, &borrowed)
        }
        Err(error) => {
            let mut rows = doctor_rows_with(&env, &layout, &sh::RealRunner, &[]);
            rows.push(Row::fail("recipes", format!("{error:#}")));
            rows
        }
    };
    let ok = healthy(&rows);
    Ok((rows, ok))
}

pub(crate) fn doctor_rows_with(
    env: &Env,
    layout: &Layout,
    runner: &dyn sh::Runner,
    deepseek_models: &[String],
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
        match runner.run(
            &sh::Cmd::new(layout.wrapper().display().to_string(), sh::SHORT)
                .arg("--version")
                .own_group(),
        ) {
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
        Some(range) if range == PI_VERSION && install::is_installed_exactly(layout) => {
            rows.push(Row::ok(
                "pin",
                format!("{range} in {}", layout.package_json().display()),
            ))
        }
        Some(range) => rows.push(Row::fail(
            "pin",
            format!(
                "prefix pin {range}, installed {}; expected exactly {PI_VERSION}",
                install::installed_version(layout).unwrap_or_else(|| "missing".into())
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
                        "{} must set \"skills\": [\"!**\"] (pi 0.99.1 uses skill patterns, not skills.enabled)",
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
    match provider::missing_overrides(&layout.models(), deepseek_models) {
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
            .own_group()
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
fn check_provider_allowed(provider: &str) -> Result<()> {
    if !launch::PROVIDERS.contains(&provider) {
        anyhow::bail!("pi_args_forbidden: `{provider}` is not a pi provider");
    }
    Ok(())
}

/// Read-only readiness for one provider (SPEC-pi v2 §3, §3.4): wrapper
/// identity, pin, trust setting, both extensions, `pi auth check`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CheckReport {
    pub(crate) ok: bool,
    pub(crate) provider: String,
    pub(crate) rows: Vec<Row>,
}

impl CheckReport {
    #[cfg(test)]
    pub(crate) fn failures(&self) -> Vec<&Row> {
        self.rows
            .iter()
            .filter(|r| r.level == Level::Fail)
            .collect()
    }

    /// A provider class requires positive provider evidence and no competing
    /// local/setup failure. Missing binaries, malformed local output and
    /// timeouts therefore stay unknown.
    pub(crate) fn failure_evidence(&self) -> FailureEvidence {
        failure_evidence(self.rows.iter().filter(|row| row.level == Level::Fail))
    }

    /// The shape `herdr-pi check <provider>` prints and A1 records.
    pub(crate) fn json(&self) -> Value {
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
                    "failure_class": if row.level == Level::Fail {
                        match row.evidence {
                            FailureEvidence::Provider => "provider",
                            FailureEvidence::Unknown => "unknown",
                        }
                    } else {
                        "none"
                    },
                })
            })
            .collect();
        serde_json::json!({
            "ok": self.ok,
            "provider": self.provider,
            "checks": rows,
            "failure_class": if self.ok { "none" } else { match self.failure_evidence() {
                FailureEvidence::Provider => "provider",
                FailureEvidence::Unknown => "unknown",
            } },
        })
    }
}

pub(crate) fn check_report_model(
    env: &Env,
    layout: &Layout,
    runner: &dyn sh::Runner,
    provider: &str,
    model: Option<&str>,
) -> CheckReport {
    let deepseek: Vec<_> = model
        .filter(|model| provider == super::provider::PROVIDER_ID && model.starts_with("deepseek"))
        .map(str::to_string)
        .into_iter()
        .collect();
    let mut rows = doctor_rows_with(env, layout, runner, &deepseek);
    if let Err(error) = check_provider_allowed(provider) {
        rows.push(Row::fail("provider", format!("{error:#}")));
    } else if layout.wrapper().is_file() {
        let result = auth_check_model(runner, layout, provider, model);
        let label = if result
            .as_ref()
            .err()
            .is_some_and(|error| error.evidence == FailureEvidence::Unknown)
        {
            "readiness"
        } else {
            "login"
        };
        rows.push(auth_row(label.into(), result));
    }

    let ok = healthy(&rows);
    CheckReport {
        ok,
        provider: provider.to_string(),
        rows,
    }
}

/// ADE-facing doctor rows for the exact routed provider/model pairs.
pub(crate) fn doctor_rows_with_models(
    env: &Env,
    layout: &Layout,
    runner: &dyn sh::Runner,
    models: &[(&str, &str)],
) -> Vec<Row> {
    let deepseek: Vec<_> = models
        .iter()
        .filter(|(p, m)| *p == provider::PROVIDER_ID && m.starts_with("deepseek"))
        .map(|(_, m)| m.to_string())
        .collect();
    let mut rows = doctor_rows_with(env, layout, runner, &deepseek);
    if layout.wrapper().is_file() {
        rows.extend(provider_rows(runner, layout, models));
    } else {
        rows.extend(models.iter().map(|(p, m)| {
            Row::fail(
                format!("provider {p}/{m}"),
                "no wrapper; run `herdr-pi setup`",
            )
        }));
    }
    rows
}

fn auth_row(label: String, result: AuthResult) -> Row {
    match result {
        Ok(()) => Row::ok(label, "login ready"),
        Err(error) if error.evidence == FailureEvidence::Provider => {
            Row::provider_fail(label, error.detail)
        }
        Err(error) => Row::fail(label, error.detail),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthFailure {
    evidence: FailureEvidence,
    detail: String,
}

type AuthResult = std::result::Result<(), AuthFailure>;

fn unknown_auth(detail: impl Into<String>) -> AuthFailure {
    AuthFailure {
        evidence: FailureEvidence::Unknown,
        detail: detail.into(),
    }
}

fn provider_auth(detail: impl Into<String>) -> AuthFailure {
    AuthFailure {
        evidence: FailureEvidence::Provider,
        detail: detail.into(),
    }
}

#[cfg(test)]
fn auth_check(runner: &dyn sh::Runner, layout: &Layout, provider: &str) -> AuthResult {
    auth_check_model(runner, layout, provider, None)
}

fn auth_check_model(
    runner: &dyn sh::Runner,
    layout: &Layout,
    provider: &str,
    model: Option<&str>,
) -> AuthResult {
    auth_checks(runner, layout, &[(provider, model)]).remove(0)
}

fn provider_rows(runner: &dyn sh::Runner, layout: &Layout, models: &[(&str, &str)]) -> Vec<Row> {
    let inputs: Vec<_> = models.iter().map(|(p, m)| (*p, Some(*m))).collect();
    models
        .iter()
        .zip(auth_checks(runner, layout, &inputs))
        .map(|((p, m), result)| auth_row(format!("provider {p}/{m}"), result))
        .collect()
}

/// Both production and scripted runners use the batching seam, in two phases:
/// auth refresh, then one live model call. A failure never causes a second call.
fn auth_checks(
    runner: &dyn sh::Runner,
    layout: &Layout,
    inputs: &[(&str, Option<&str>)],
) -> Vec<AuthResult> {
    let keys: Vec<_> = inputs
        .iter()
        .map(|(p, m)| match m {
            Some(m) => format!(
                "model-{}",
                &crate::thread::sha256_hex(format!("{p}\0{m}").as_bytes())[..16]
            ),
            None => p.to_string(),
        })
        .collect();
    let mut results: Vec<_> = keys
        .iter()
        .map(|key| read_cached_probe(layout, key))
        .collect();
    let pending: Vec<_> = results
        .iter()
        .enumerate()
        .filter_map(|(i, result)| result.is_none().then_some(i))
        .collect();
    let auth: Vec<_> = pending
        .iter()
        .map(|&i| {
            sh::Cmd::new(layout.wrapper().display().to_string(), sh::SHORT)
                .own_group()
                .args(["auth", "check", "--provider", inputs[i].0, "--json"])
                .env("PI_CODING_AGENT_DIR", layout.agent().display().to_string())
        })
        .collect();
    let mut live = Vec::new();
    let mut live_ids = Vec::new();
    for (&i, output) in pending.iter().zip(runner.run_parallel(&auth)) {
        match output
            .map_err(|error| {
                unknown_auth(format!("provider readiness check could not run: {error:#}"))
            })
            .and_then(|output| live_command(layout, inputs[i].0, inputs[i].1, &output))
        {
            Ok(command) => {
                live_ids.push(i);
                live.push(command);
            }
            Err(error) => results[i] = Some(Err(error)),
        }
    }
    for (i, output) in live_ids.into_iter().zip(runner.run_parallel(&live)) {
        results[i] = Some(
            output
                .map_err(|error| {
                    unknown_auth(format!("provider readiness probe could not run: {error:#}"))
                })
                .and_then(|output| live_result(inputs[i].0, &output)),
        );
    }
    for i in pending {
        write_cached_probe(
            layout,
            &keys[i],
            inputs[i].0,
            results[i].as_ref().expect("probe result"),
        );
    }
    results
        .into_iter()
        .map(|result| result.expect("probe result"))
        .collect()
}

fn live_command(
    layout: &Layout,
    provider: &str,
    selected_model: Option<&str>,
    output: &sh::Output,
) -> std::result::Result<sh::Cmd, AuthFailure> {
    if output.timed_out || output.code.is_none() {
        return Err(unknown_auth(
            "provider readiness check timed out or was terminated by a signal",
        ));
    }
    let answer = if output.stdout.trim().is_empty() {
        output.stderr.trim()
    } else {
        output.stdout.trim()
    };
    let parsed: Value = serde_json::from_str(answer).map_err(|_| {
        unknown_auth(format!(
            "provider readiness check did not answer JSON (exit {}): {}",
            output
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".into()),
            output.error_text()
        ))
    })?;
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("");
    if status != "ready" || !output.success() {
        let reason = parsed.get("reason").and_then(Value::as_str).unwrap_or("");
        if status == "not_ready" && !reason.is_empty() {
            return Err(provider_auth(format!(
                "missing sign-in: {reason} (run `herdr-pi login {provider}`)"
            )));
        }
        return Err(unknown_auth(format!(
            "provider readiness check returned an unrecognized status `{status}` (exit {:?})",
            output.code
        )));
    }

    let model = selected_model
        .map(Ok)
        .unwrap_or_else(|| probe_model(provider))
        .map_err(|error| unknown_auth(format!("provider readiness setup failed: {error:#}")))?;
    // GPT-6.1 Sol has no Off thinking level (its catalog maps `off` to
    // null). Probe it at a supported level instead of refusing a ready lane.
    let thinking = if model == "gpt-6.1-sol" {
        "high"
    } else {
        "off"
    };
    Ok(
        sh::Cmd::new(layout.wrapper().display().to_string(), LIVE_PROBE_TIMEOUT)
            .own_group()
            .args([
                "--provider",
                provider,
                "--model",
                model,
                "--thinking",
                thinking,
                "--no-tools",
                "--no-skills",
                "--no-extensions",
                "--no-prompt-templates",
                "--no-themes",
                "--no-context-files",
                "--no-session",
                "--system-prompt",
                "Reply only OK.",
                "--print",
                "Reply OK.",
            ])
            .env("PI_CODING_AGENT_DIR", layout.agent().display().to_string()),
    )
}

fn live_result(provider: &str, live: &sh::Output) -> AuthResult {
    if live.success() {
        return Ok(());
    }
    if live.timed_out || live.code.is_none() {
        return Err(unknown_auth(
            "provider readiness probe timed out or was terminated",
        ));
    }
    let error_text = live.error_text();
    let detail = error_text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("provider readiness probe failed without a diagnostic")
        .trim();
    if positive_sign_in_evidence(detail) {
        Err(provider_auth(format!(
            "stored sign-in no longer works for {provider}: {detail} (run `herdr-pi login {provider}`)"
        )))
    } else {
        Err(unknown_auth(format!(
            "provider readiness probe failed without a recognized provider refusal: {detail}"
        )))
    }
}

/// Diagnostics that positively identify a credential/account refusal. A
/// generic non-zero exit is not evidence about sign-in.
pub(crate) fn positive_sign_in_evidence(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    [
        "subscription expired",
        "authentication failed",
        "unauthorized",
        "invalid api key",
        "invalid_api_key",
        "token expired",
        "login required",
        "not logged in",
        "credentials_not_configured",
        "status 401",
        "status 403",
        "http 401",
        "http 403",
    ]
    .iter()
    .any(|marker| detail.contains(marker))
}

fn probe_model(provider: &str) -> Result<&'static str> {
    match provider {
        "openai-codex" => Ok("gpt-5.6-sol"),
        "opencode-go" => Ok("deepseek-v4.1-flash"),
        "kimi-coding" => Ok("k3"),
        _ => anyhow::bail!("pi_args_forbidden: `{provider}` is not a pi provider"),
    }
}

fn probe_cache_path(layout: &Layout, key: &str) -> PathBuf {
    layout.root.join(format!("readiness-{key}.json"))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedObservation {
    checked_unix: u64,
    row: Row,
}

/// Native and pi probes use the same typed cache. Unknown evidence is never
/// persisted or upgraded; labels come from the current selected probe plan.
pub(crate) fn cached_row(path: &Path, label: String) -> Option<Row> {
    let mut cached: CachedObservation = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    if now.saturating_sub(cached.checked_unix) > READINESS_CACHE_TTL.as_secs()
        || (cached.row.level != Level::Ok && cached.row.evidence != FailureEvidence::Provider)
    {
        return None;
    }
    cached.row.label = label;
    Some(cached.row)
}

pub(crate) fn cache_row(path: &Path, row: Row) {
    if row.level != Level::Ok && row.evidence != FailureEvidence::Provider {
        let _ = std::fs::remove_file(path);
        return;
    }
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(checked_unix) = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
    else {
        return;
    };
    let Ok(bytes) = serde_json::to_vec(&CachedObservation { checked_unix, row }) else {
        return;
    };
    let staged = path.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&staged, bytes).is_ok() {
        let _ = std::fs::rename(&staged, path);
    }
    let _ = std::fs::remove_file(staged);
}

fn read_cached_probe(layout: &Layout, key: &str) -> Option<AuthResult> {
    cached_row(&probe_cache_path(layout, key), String::new()).map(|row| {
        if row.level == Level::Ok {
            Ok(())
        } else {
            Err(AuthFailure {
                evidence: row.evidence,
                detail: row.detail,
            })
        }
    })
}

fn write_cached_probe(layout: &Layout, key: &str, provider: &str, result: &AuthResult) {
    if !layout.root.is_dir() {
        return;
    }
    let mut row = auth_row(String::new(), result.clone());
    // Provider output never becomes a credential-adjacent file.
    if row.evidence == FailureEvidence::Provider {
        let reason = if row.detail.starts_with("missing sign-in:") {
            "missing sign-in"
        } else {
            "stored sign-in no longer works"
        };
        row.detail = format!("{reason} for {provider} (run `herdr-pi login {provider}`)");
    }
    cache_row(&probe_cache_path(layout, key), row);
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
    let output = match runner.run(
        &sh::Cmd::new(shell, sh::SHORT)
            .args(["-lic", script])
            .own_group(),
    ) {
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

/// The login shell's own "where is this word" builtin. Visible to the
/// fake-runner scenarios so their scripts use the same shell as the probe.
pub(super) fn path_probe(shell: &str) -> &'static str {
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
    use crate::runner::fake::{FakeRunner, fail, ok};

    #[test]
    fn pi_inventory_uses_the_canonical_catalog_and_route_validation() {
        let dir = tempfile::tempdir().unwrap();
        // Setup can write compaction overrides before routing is configured.
        assert!(!configured_deepseek_models(dir.path()).unwrap().is_empty());
        let path = dir.path().join("config.toml");
        let config = r#"
[routing]
default = "pi_opencode_deepseek"
[routing.pins]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" = "pi_codex_sol_high"
[[routing.rules]]
workflow = "reviewer"
recipe = "pi_opencode_muse"
[recipes.pi_opencode_deepseek]
kind = "pi"
provider = "opencode-go"
args = ["--provider=opencode-go", "--model=deepseek-custom", "--thinking=low", "--no-skills"]
"#;
        std::fs::write(&path, config).unwrap();
        let catalog = crate::launch::parse_launch_config(dir.path()).unwrap();
        let models = configured_routed_models(dir.path()).unwrap();
        assert_eq!(models.len(), 3);
        for id in catalog.routing.recipe_ids() {
            let recipe = &catalog.recipes[id];
            let model =
                launch::validate_recipe(id, &recipe.provider, &recipe.args, &recipe.env).unwrap();
            assert!(models.contains(&(recipe.provider.clone(), model)));
        }
        assert_eq!(
            configured_deepseek_models(dir.path()).unwrap(),
            ["deepseek-custom"]
        );
        // The former shadow schema accepted invalid rules and recipe fields.
        for invalid in [
            config.replace("workflow = \"reviewer\"\n", ""),
            config.replace("kind = \"pi\"", "kind = \"pi\"\nextra = true"),
            config.replace(
                "provider = \"opencode-go\"",
                "provider = \"opencode-go\"\ncapabilities = [\"undeclared\"]",
            ),
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(configured_routed_models(dir.path()).is_err());
        }
    }

    fn scripted(env: &Env) -> FakeRunner {
        let link = env.home.join(".local/bin/pi");
        let shell = sh::shell();
        let probe = path_probe(&shell);
        let runner = FakeRunner::new();
        runner.on(&format!("{shell} -lic node --version"), ok("v22.19.0\n"));
        runner.on(
            &format!("{shell} -lic command -v npm"),
            ok("/opt/homebrew/bin/npm\n"),
        );
        runner.on(
            &format!("{shell} -lic npm root -g"),
            ok(&format!(
                "{}\n",
                env.home.join("global/node_modules").display()
            )),
        );
        let resolution = if probe == "command -v pi" {
            format!("{}\n", link.display())
        } else {
            format!("pi is {}\n", link.display())
        };
        runner.on(&format!("{shell} -lic {probe}"), ok(&resolution));
        runner.on("herdr integration status", ok("pi: current\n"));
        runner.on("--version", ok("0.99.1\n"));
        runner.on("--print Reply OK.", ok("OK\n"));
        runner
    }

    fn installed_layout(dir: &Path) -> Layout {
        let layout = Layout::for_test(dir.join("pi"));
        folder::ensure(&layout).unwrap();
        crate::pi::provider::write_overrides(&layout.models(), &["deepseek-v4.1-flash".into()])
            .unwrap();
        install::write_guard(&layout).unwrap();
        crate::pi::launch::write_wrapper(&layout).unwrap();
        std::fs::create_dir_all(layout.package().join("dist/bundle")).unwrap();
        std::fs::write(layout.package_json(), r#"{"version":"0.99.1"}"#).unwrap();
        std::fs::write(layout.cli_js(), "// cli").unwrap();
        std::fs::write(
            layout.npm().join("package.json"),
            r#"{"dependencies":{"@earendil-works/pi-coding-agent":"0.99.1"}}"#,
        )
        .unwrap();
        layout
    }

    /// The link `~/.local/bin/pi` a real setup asks Rolf to make.
    fn link_into(env: &Env, layout: &Layout) {
        std::fs::create_dir_all(env.home.join(".local/bin")).unwrap();
        std::os::unix::fs::symlink(layout.wrapper(), env.home.join(".local/bin/pi")).unwrap();
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
        assert_eq!(
            wrapper_path_row_with(&runner, &env, &layout, "/bin/zsh").level,
            Level::Ok
        );
        let runner = FakeRunner::new();
        runner.on(
            "zsh -lic whence -va pi",
            ok(&format!(
                "pi is /opt/homebrew/bin/pi\npi is {}\n",
                link.display()
            )),
        );
        assert_eq!(
            wrapper_path_row_with(&runner, &env, &layout, "/bin/zsh").level,
            Level::Fail
        );
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
    fn a_caret_pin_and_a_true_trust_entry_fail() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        std::fs::write(
            layout.npm().join("package.json"),
            r#"{"dependencies":{"@earendil-works/pi-coding-agent":"^0.99.1"}}"#,
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
    fn model_probes_use_runner_batches_with_exact_identities_and_deadlines() {
        struct Batched {
            runner: FakeRunner,
            batches: std::cell::RefCell<Vec<Vec<sh::Cmd>>>,
        }
        impl sh::Runner for Batched {
            fn run(&self, cmd: &sh::Cmd) -> Result<sh::Output> {
                self.runner.run(cmd)
            }
            fn run_parallel(&self, commands: &[sh::Cmd]) -> Vec<Result<sh::Output>> {
                self.batches.borrow_mut().push(commands.to_vec());
                self.runner.run_parallel(commands)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[]);
        link_into(&env, &layout);
        let runner = Batched {
            runner: scripted(&env),
            batches: Default::default(),
        };
        runner.runner.on("auth check", ok(r#"{"status":"ready"}"#));
        let models = [
            ("openai-codex", "gpt-6.1-sol"),
            ("openai-codex", "gpt-5.6-sol"),
        ];
        let rows = doctor_rows_with_models(&env, &layout, &runner, &models);
        for (_, model) in models {
            assert!(
                rows.iter()
                    .any(|row| row.label == format!("provider openai-codex/{model}")
                        && row.level == Level::Ok)
            );
        }
        let batches = runner.batches.borrow();
        assert_eq!(batches.len(), 2);
        assert!(batches[0].iter().all(|cmd| cmd.timeout == sh::SHORT));
        assert_eq!(batches[0].len(), 2);
        assert_eq!(batches[1].len(), 2);
        assert!(
            batches[1]
                .iter()
                .all(|cmd| cmd.timeout == LIVE_PROBE_TIMEOUT && cmd.own_group)
        );
        for (command, (_, model)) in batches[1].iter().zip(models) {
            assert!(
                command
                    .args
                    .windows(2)
                    .any(|pair| pair == ["--model", model])
            );
        }
    }

    #[test]
    fn check_and_doctor_share_setup_and_refuse_an_installed_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[]);
        link_into(&env, &layout);
        std::fs::write(layout.package_json(), r#"{"version":"wrong"}"#).unwrap();
        let runner = scripted(&env);
        runner.on("auth check", ok(r#"{"status":"ready"}"#));
        let doctor = doctor_rows_with_models(&env, &layout, &runner, &[("kimi-coding", "k3")]);
        let check = check_report_model(&env, &layout, &runner, "kimi-coding", Some("k3"));
        assert!(!check.ok);
        let setup: Vec<_> = doctor
            .into_iter()
            .filter(|row| !row.label.starts_with("provider "))
            .collect();
        let check_setup: Vec<_> = check
            .rows
            .into_iter()
            .filter(|row| row.label != "login" && row.label != "readiness")
            .collect();
        assert_eq!(setup, check_setup);
        assert!(
            setup
                .iter()
                .any(|row| row.label == "pin" && row.level == Level::Fail)
        );
    }

    #[test]
    fn signaled_model_output_cannot_establish_provider_failure() {
        let output = sh::Output {
            stderr: "unauthorized".into(),
            ..Default::default()
        };
        assert_eq!(
            live_result("kimi-coding", &output).unwrap_err().evidence,
            FailureEvidence::Unknown
        );
    }

    #[test]
    fn live_probe_has_measured_headroom_but_still_refuses_failures() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let runner = FakeRunner::new();
        runner.on(
            "auth check --provider openai-codex",
            ok(r#"{"status":"ready"}"#),
        );
        runner.on("--print Reply OK.", fail(1, "authentication failed"));
        let failure = auth_check_model(&runner, &layout, "openai-codex", Some("gpt-6-sol"));
        assert!(failure.is_err());
        assert_eq!(failure.unwrap_err().evidence, FailureEvidence::Provider);
        let calls = runner.calls.borrow();
        let probe = calls
            .iter()
            .find(|cmd| cmd.display().contains("--print Reply OK."))
            .unwrap();
        assert_eq!(probe.timeout, LIVE_PROBE_TIMEOUT);
        assert_eq!(probe.timeout, Duration::from_secs(30));
    }

    #[test]
    fn interrupted_auth_json_is_unknown_and_never_cached() {
        for timed_out in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let layout = installed_layout(dir.path());
            let runner = FakeRunner::new();
            runner.on(
                "auth check --provider opencode-go",
                sh::Output {
                    code: None,
                    stdout: r#"{"status":"not_ready","reason":"credentials_not_configured"}"#
                        .into(),
                    timed_out,
                    ..Default::default()
                },
            );
            for _ in 0..2 {
                let failure = auth_check(&runner, &layout, "opencode-go").unwrap_err();
                assert_eq!(failure.evidence, FailureEvidence::Unknown);
                assert!(!probe_cache_path(&layout, "opencode-go").exists());
            }
            assert_eq!(runner.count("auth check"), 2);
            assert_eq!(runner.count("--print"), 0);
        }
    }

    #[test]
    fn sol_61_readiness_uses_supported_thinking() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let runner = FakeRunner::new();
        runner.on(
            "auth check --provider openai-codex",
            ok(r#"{"status":"ready"}"#),
        );
        runner.on("--print Reply OK.", ok("OK"));
        assert!(auth_check_model(&runner, &layout, "openai-codex", Some("gpt-6.1-sol")).is_ok());
        let calls = runner.calls.borrow();
        let probe = calls
            .iter()
            .find(|cmd| cmd.display().contains("--print Reply OK."))
            .unwrap();
        assert_eq!(
            crate::pi::launch::flag_value(&probe.args, "--thinking").as_deref(),
            Some("high")
        );
    }

    #[test]
    fn named_model_readiness_reuses_only_the_same_recent_answer() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[]);
        let runner = scripted(&env);
        runner.on(
            "auth check --provider opencode-go",
            ok(r#"{"status":"ready"}"#),
        );
        assert!(auth_check_model(&runner, &layout, "opencode-go", Some("first")).is_ok());
        assert!(auth_check_model(&runner, &layout, "opencode-go", Some("first")).is_ok());
        assert_eq!(runner.count("auth check --provider opencode-go"), 1);
        assert_eq!(runner.count("--print Reply OK."), 1);
        assert!(auth_check_model(&runner, &layout, "opencode-go", Some("second")).is_ok());
        assert_eq!(runner.count("auth check --provider opencode-go"), 2);
        assert_eq!(runner.count("--print Reply OK."), 2);
        let calls = runner.calls.borrow();
        assert!(calls.iter().all(|cmd| cmd.own_group));
        assert!(calls.iter().all(|cmd| cmd.env
            == [(
                "PI_CODING_AGENT_DIR".into(),
                layout.agent().display().to_string()
            )]));
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
    }

    #[test]
    fn check_refuses_unknown_providers() {
        assert!(check_provider_allowed("moonshot").is_err());
        assert!(check_provider_allowed("kimi-coding").is_ok());
    }

    #[test]
    fn a_stale_ready_credential_fails_when_the_model_call_is_refused_and_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = {
            let custom = FakeRunner::new();
            let shell = sh::shell();
            let probe = path_probe(&shell);
            let link = env.home.join(".local/bin/pi");
            let resolution = if probe == "command -v pi" {
                format!("{}\n", link.display())
            } else {
                format!("pi is {}\n", link.display())
            };
            custom.on(&format!("{shell} -lic {probe}"), ok(&resolution));
            custom.on("herdr integration status", ok("pi: current\n"));
            custom.on(
                "auth check --provider kimi-coding",
                ok(r#"{"status":"ready"}"#),
            );
            custom.on("--print Reply OK.", fail(1, "subscription expired"));
            custom
        };
        let first = check_report_model(&env, &layout, &runner, "kimi-coding", None);
        assert!(!first.ok);
        let login = first.rows.iter().find(|row| row.label == "login").unwrap();
        assert_eq!(login.level, Level::Fail);
        assert!(login.detail.contains("subscription expired"), "{login:?}");
        let cache = std::fs::read_to_string(probe_cache_path(&layout, "kimi-coding")).unwrap();
        assert!(!cache.contains("subscription expired"), "{cache}");
        let second = check_report_model(&env, &layout, &runner, "kimi-coding", None);
        assert!(!second.ok);
        assert_eq!(runner.count("auth check"), 1);
        assert_eq!(runner.count("--print"), 1);
    }

    #[test]
    fn a_local_getcwd_error_is_unknown_and_never_a_login_failure() {
        let dir = tempfile::tempdir().unwrap();
        let layout = installed_layout(dir.path());
        let env = Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = {
            let custom = FakeRunner::new();
            let shell = sh::shell();
            let probe = path_probe(&shell);
            let link = env.home.join(".local/bin/pi");
            let resolution = if probe == "command -v pi" {
                format!("{}\n", link.display())
            } else {
                format!("pi is {}\n", link.display())
            };
            custom.on(&format!("{shell} -lic {probe}"), ok(&resolution));
            custom.on("herdr integration status", ok("pi: current\n"));
            custom.on(
                "auth check --provider openai-codex",
                fail(
                    1,
                    "shell-init: error retrieving current directory: getcwd: cannot access parent directories",
                ),
            );
            custom
        };

        let report = check_report_model(&env, &layout, &runner, "openai-codex", None);
        assert!(!report.ok);
        assert_eq!(report.failure_evidence(), FailureEvidence::Unknown);
        let failure = report
            .failures()
            .into_iter()
            .find(|row| row.label == "readiness")
            .unwrap();
        assert!(failure.detail.contains("getcwd"), "{failure:?}");
        assert!(!failure.detail.contains("herdr-pi login"), "{failure:?}");
        assert!(!report.failures().iter().any(|row| row.label == "login"));
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
        let report = check_report_model(&env, &layout, &runner, "kimi-coding", None);
        assert!(!report.ok);
        let json = report.json();
        assert_eq!(json["ok"], Value::Bool(false));
        assert_eq!(json["provider"], "kimi-coding");
        assert!(report.failures().iter().any(|r| r.label == "login"));
    }
}
