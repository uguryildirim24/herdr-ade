//! The pinned install (SPEC-pi v2 §3.2).
//!
//! Exactly `@earendil-works/pi-coding-agent@0.85.1` into
//! `<ADE root>/pi/npm`, never global, never `pi install`.
//! Setup is an action Rolf runs; the library refuses a start without the pin.

use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::{GUARD_MARKER, Layout, PI_PACKAGE, PI_VERSION, sh};

/// The `pro` provider writer, shared with `herdr-pro` by path (the file is
/// self-contained, so both binaries can compile it).
#[path = "../pro/provider.rs"]
mod provider;

/// The DeepSeek `contextWindow` merge (a second writer of the same
/// `models.json`; it derives its models from the recipe rows).
use super::provider as deepseek;

/// The guard extension, plugin-owned, beside the herdr state hook
/// (SPEC-pi v2 §3.3, §3.7). Doctor checks the marker.
pub const GUARD_TS: &str = include_str!("../../extensions/herdr-pi-guard.ts");

/// The exact `npm install` argv (SPEC-pi v2 §3.2). `--save-exact` is the pin;
/// a caret range is refused by doctor.
pub fn npm_install_args(layout: &Layout) -> Vec<String> {
    vec![
        "install".to_string(),
        "--prefix".to_string(),
        layout.npm().display().to_string(),
        "--save-exact".to_string(),
        "--no-fund".to_string(),
        "--no-audit".to_string(),
        format!("{PI_PACKAGE}@{PI_VERSION}"),
    ]
}

/// What the installed `package.json` says, when anything is installed.
pub fn installed_version(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.package_json()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// True when the prefix holds exactly the pinned version.
pub fn is_installed_exactly(layout: &Layout) -> bool {
    installed_version(layout).as_deref() == Some(PI_VERSION)
}

/// The bin the package declares, and the file it must point at.
pub fn cli_js_exists(layout: &Layout) -> bool {
    layout.cli_js().is_file()
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstallReport {
    pub version: String,
    pub npm_line: String,
}

/// Install the pin with npm and check it landed. Never global; a failure is
/// an error, not a warning.
pub fn install(runner: &dyn sh::Runner, layout: &Layout) -> Result<InstallReport> {
    std::fs::create_dir_all(layout.npm())
        .with_context(|| format!("could not create {}", layout.npm().display()))?;
    let args = npm_install_args(layout);
    let output = runner.run(&sh::Cmd::new("npm", sh::SETUP).args(args.clone()))?;
    if !output.success() {
        bail!("npm install failed: {}", output.error_text());
    }
    let version = installed_version(layout).ok_or_else(|| {
        anyhow::anyhow!(
            "{} has no version after npm install",
            layout.package_json().display()
        )
    })?;
    if version != PI_VERSION {
        bail!("installed pi is {version}, not the pinned {PI_VERSION}");
    }
    if !cli_js_exists(layout) {
        bail!("the pinned package has no {}", layout.cli_js().display());
    }
    Ok(InstallReport {
        version,
        npm_line: format!("npm {}", args.join(" ")),
    })
}

/// Write the plugin-owned guard extension and return its path.
pub fn write_guard(layout: &Layout) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(layout.extensions())
        .with_context(|| format!("could not create {}", layout.extensions().display()))?;
    std::fs::write(layout.guard(), GUARD_TS)
        .with_context(|| format!("could not write {}", layout.guard().display()))?;
    Ok(layout.guard())
}

/// True when the guard is present with the plugin's marker (SPEC-pi v2 §3.9).
pub fn guard_ok(layout: &Layout) -> bool {
    std::fs::read_to_string(layout.guard())
        .map(|text| text.contains(GUARD_MARKER))
        .unwrap_or(false)
}

/// The one line Rolf types after setup (SPEC-pi v2 §3.2). The wrapper itself
/// is written by setup; the symlink is his to make.
pub fn link_line(layout: &Layout, home: &std::path::Path) -> String {
    format!(
        "ln -s {} {}",
        layout.wrapper().display(),
        home.join(".local/bin/pi").display()
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetupReport {
    pub steps: Vec<String>,
    pub pi_folder: std::path::PathBuf,
    pub npm_prefix: std::path::PathBuf,
    pub link_line: String,
}

/// Write the `pro` provider into `models.json` when the relay has written its
/// `serve.json`. Returns the relay base URL that was written.
fn write_provider(layout: &Layout) -> Result<Option<String>> {
    let Some(ade_root) = layout.root.parent() else {
        return Ok(None);
    };
    let state = ade_root.join("pro-bridge/serve.json");
    let text = match std::fs::read_to_string(&state) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", state.display()));
        }
    };
    let value: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("{} does not parse", state.display()))?;
    let port = value
        .get("port")
        .and_then(serde_json::Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .with_context(|| format!("{} has no valid port", state.display()))?;
    let token = value
        .get("token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .with_context(|| format!("{} has no token", state.display()))?;
    let base = provider::base_url(port);
    provider::write_merged(&layout.models(), &base, token)?;
    Ok(Some(base))
}

/// Write the DeepSeek `contextWindow` overrides into `models.json`. Runs on
/// every setup, with or without the relay's `serve.json`; the merge keeps the
/// `pro` provider and every other key.
fn write_deepseek(layout: &Layout) -> Result<()> {
    deepseek::write_overrides(&layout.models())
}

/// Setup: pinned install, shared folder, guard, the running herdr's state
/// hook, then print the one line Rolf types. Never a login.
pub fn setup(runner: &dyn sh::Runner, env: &super::Env, layout: &Layout) -> Result<SetupReport> {
    let install = install(runner, layout)?;
    let folder = super::folder::ensure(layout)?;
    let wrapper = super::launch::write_wrapper(layout)?;
    let guard = write_guard(layout)?;
    let provider = write_provider(layout)?;
    write_deepseek(layout)?;

    let integration = runner.run(
        &sh::Cmd::new(env.herdr_bin(), Duration::from_secs(120))
            .args(["integration", "install", "pi"])
            .env("PI_CODING_AGENT_DIR", layout.agent().display().to_string()),
    )?;
    if !integration.success() {
        bail!(
            "`{} integration install pi` failed: {}",
            env.herdr_bin(),
            integration.error_text()
        );
    }

    let mut steps = vec![
        format!(
            "installed {PI_PACKAGE}@{PI_VERSION} into {} (`{}`)",
            layout.npm().display(),
            install.npm_line
        ),
        format!(
            "wrote the shared pi folder {} (settings.json, models.json, trust.json)",
            layout.agent().display()
        ),
        format!("wrote the wrapper {}", wrapper.display()),
        format!("wrote the guard {}", guard.display()),
        format!(
            "installed the herdr state hook into {}",
            layout.extensions().display()
        ),
        format!(
            "wrote the DeepSeek compaction override (contextWindow {}) into {}",
            deepseek::DEEPSEEK_CONTEXT_WINDOW,
            layout.models().display()
        ),
    ];
    if let Some(base) = &provider {
        steps.push(format!(
            "wrote the `pro` provider ({base}) into {}",
            layout.models().display()
        ));
    } else {
        steps.push(format!(
            "no `pro` provider yet: {} is missing; run `herdr-pro serve`, then `herdr-pi setup` again",
            layout
                .root
                .parent()
                .unwrap_or(&layout.root)
                .join("pro-bridge/serve.json")
                .display()
        ));
    }
    let _ = folder;
    Ok(SetupReport {
        steps,
        pi_folder: layout.agent(),
        npm_prefix: layout.npm(),
        link_line: link_line(layout, &env.home),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi::sh::fake::{FakeRunner, ok};

    #[test]
    fn npm_argv_is_pinned_and_never_global() {
        let layout = Layout::for_test("/state/pi");
        let args = npm_install_args(&layout);
        assert!(args.contains(&"--prefix".to_string()));
        assert!(args.contains(&"/state/pi/npm".to_string()));
        assert!(args.contains(&"--save-exact".to_string()));
        assert!(args.contains(&format!("{PI_PACKAGE}@0.85.1")));
        assert!(!args.iter().any(|a| a == "-g" || a == "--global"));
        assert!(!args.iter().any(|a| a.contains('^')));
    }

    #[test]
    fn install_reads_the_prefix_package_json_and_refuses_a_caret() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pi"));
        std::fs::create_dir_all(layout.package().join("dist/bundle")).unwrap();
        std::fs::write(layout.package_json(), r#"{"name":"x","version":"0.85.1"}"#).unwrap();
        std::fs::write(layout.cli_js(), "// cli").unwrap();
        assert!(is_installed_exactly(&layout));
        std::fs::write(layout.package_json(), r#"{"name":"x","version":"0.86.0"}"#).unwrap();
        assert!(!is_installed_exactly(&layout));
    }

    #[test]
    fn install_runs_npm_and_checks_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pi"));
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.display().contains("npm install"),
            |cmd| {
                let prefix = crate::pi::launch::flag_value(&cmd.args, "--prefix").unwrap();
                let package = std::path::Path::new(&prefix)
                    .join("node_modules/@earendil-works/pi-coding-agent");
                std::fs::create_dir_all(package.join("dist/bundle")).unwrap();
                std::fs::write(package.join("package.json"), r#"{"version":"0.85.1"}"#).unwrap();
                std::fs::write(package.join("dist/bundle/cli.js"), "// cli").unwrap();
                Ok(ok("added 1 package\n"))
            },
        );
        let report = install(&runner, &layout).unwrap();
        assert_eq!(report.version, "0.85.1");
        assert_eq!(runner.count("npm install"), 1);
    }

    #[test]
    fn setup_writes_the_provider_when_the_relay_has_run() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pi"));
        std::fs::create_dir_all(layout.agent()).unwrap();
        let pro = dir.path().join("pro-bridge");
        std::fs::create_dir_all(&pro).unwrap();
        std::fs::write(
            pro.join("serve.json"),
            r#"{"port":1234,"pid":1,"started":"now","token":"tok"}"#,
        )
        .unwrap();
        let base = write_provider(&layout).unwrap().unwrap();
        assert_eq!(base, "http://127.0.0.1:1234/v1");
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(layout.models()).unwrap()).unwrap();
        assert_eq!(value["providers"]["pro"]["apiKey"], "tok");
        assert_eq!(
            value["providers"]["pro"]["baseUrl"],
            "http://127.0.0.1:1234/v1"
        );
    }

    #[test]
    fn setup_writes_every_step_and_a_second_run_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pi"));
        let env = crate::pi::Env::for_test(dir.path(), &[("HERDR_BIN_PATH", "/h/herdr")]);
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.program == "npm",
            |cmd| {
                let prefix = crate::pi::launch::flag_value(&cmd.args, "--prefix").unwrap();
                let package = std::path::Path::new(&prefix)
                    .join("node_modules/@earendil-works/pi-coding-agent");
                std::fs::create_dir_all(package.join("dist/bundle")).unwrap();
                std::fs::write(package.join("package.json"), r#"{"version":"0.85.1"}"#).unwrap();
                std::fs::write(package.join("dist/bundle/cli.js"), "// cli").unwrap();
                Ok(ok(""))
            },
        );
        runner.on("/h/herdr integration install pi", ok("installed pi\n"));
        let report = setup(&runner, &env, &layout).unwrap();
        assert_eq!(report.steps.len(), 7);
        assert!(
            report
                .steps
                .last()
                .unwrap()
                .contains("no `pro` provider yet"),
            "{:?}",
            report.steps
        );
        assert!(report.link_line.starts_with("ln -s "));
        assert!(layout.settings().exists());
        assert!(layout.guard().exists());
        assert!(layout.wrapper().exists());
        // The relay never ran, so the DeepSeek override is the only provider
        // row, and setup wrote it itself.
        let models: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(layout.models()).unwrap()).unwrap();
        assert_eq!(
            models["providers"]["opencode-go"]["modelOverrides"]["deepseek-v4.1-flash"]["contextWindow"],
            deepseek::DEEPSEEK_CONTEXT_WINDOW
        );
        let first = std::fs::read_to_string(layout.models()).unwrap();
        let second = setup(&runner, &env, &layout).unwrap();
        assert_eq!(second.steps.len(), 7);
        assert_eq!(std::fs::read_to_string(layout.models()).unwrap(), first);
        let integration = runner
            .calls
            .borrow()
            .iter()
            .find(|c| c.display().contains("integration install pi"))
            .cloned()
            .unwrap();
        assert!(
            integration
                .env
                .iter()
                .any(|(k, v)| k == "PI_CODING_AGENT_DIR"
                    && v == &layout.agent().display().to_string())
        );
    }
}
