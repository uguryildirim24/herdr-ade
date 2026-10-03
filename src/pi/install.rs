//! Install the exact Pi package pin into ADE's own npm prefix.
//!
//! Exactly `@earendil-works/pi-coding-agent@0.99.1` into
//! `<ADE root>/pi/npm`, never global, never `pi install`.
//! Setup is an action Rolf runs; the library refuses a start without the pin.

use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::{Layout, PI_PACKAGE, PI_VERSION};
use crate::runner as sh;

/// The DeepSeek `contextWindow` merge (a second writer of the same
/// `models.json`; it derives its models from the recipe rows).
use super::provider as deepseek;

/// Plugin-owned guard beside the Herdr state hook. Doctor compares its bytes.
const GUARD_TS: &str = include_str!("../../extensions/herdr-pi-guard.ts");

/// The exact `npm install` argv. `--save-exact` is the pin;
/// a caret range is refused by doctor.
fn npm_install_args(layout: &Layout) -> Vec<String> {
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
pub(crate) fn installed_version(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.package_json()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// True when the prefix holds exactly the pinned version.
pub(crate) fn is_installed_exactly(layout: &Layout) -> bool {
    installed_version(layout).as_deref() == Some(PI_VERSION)
}

/// The bin the package declares, and the file it must point at.
fn cli_js_exists(layout: &Layout) -> bool {
    layout.cli_js().is_file()
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InstallReport {
    pub(crate) version: String,
    pub(crate) npm_line: String,
}

/// Install the pin with npm and check it landed. Never global; a failure is
/// an error, not a warning.
pub(crate) fn install(runner: &dyn sh::Runner, layout: &Layout) -> Result<InstallReport> {
    std::fs::create_dir_all(layout.npm())
        .with_context(|| format!("could not create {}", layout.npm().display()))?;
    let args = npm_install_args(layout);
    let output = runner.run(
        &sh::Cmd::new("npm", sh::SETUP)
            .args(args.clone())
            .own_group(),
    )?;
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

/// Pin the extension to the installed ADE sibling and this shared state root.
/// Both herdr-pi setup and herdr-ade install run from the installed bin folder.
fn guard_source(layout: &Layout) -> Result<String> {
    let binary = std::env::current_exe()
        .context("could not locate the installed ADE binary")?
        .with_file_name("herdr-ade");
    let root = layout.root.parent().context("pi folder has no ADE root")?;
    let root = std::path::absolute(root)?;
    Ok(GUARD_TS
        .replace(
            "\"__HERDR_ADE_BINARY__\"",
            &serde_json::to_string(&binary.to_string_lossy())?,
        )
        .replace(
            "\"__HERDR_ADE_ROOT__\"",
            &serde_json::to_string(&root.to_string_lossy())?,
        ))
}

/// Write the plugin-owned guard extension and return its path.
pub(crate) fn write_guard(layout: &Layout) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(layout.extensions())
        .with_context(|| format!("could not create {}", layout.extensions().display()))?;
    std::fs::write(layout.guard(), guard_source(layout)?)
        .with_context(|| format!("could not write {}", layout.guard().display()))?;
    Ok(layout.guard())
}

/// True when the installed guard is the exact extension compiled into this
/// binary. A marker alone cannot detect changed behavior.
pub(crate) fn guard_ok(layout: &Layout) -> bool {
    guard_source(layout).is_ok_and(|expected| {
        std::fs::read_to_string(layout.guard()).is_ok_and(|text| text == expected)
    })
}

/// The wrapper link command printed after setup. The wrapper itself
/// is written by setup; the symlink is his to make.
pub(crate) fn link_line(layout: &Layout, home: &std::path::Path) -> String {
    format!(
        "ln -s {} {}",
        layout.wrapper().display(),
        home.join(".local/bin/pi").display()
    )
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SetupReport {
    pub(crate) steps: Vec<String>,
    pub(crate) pi_folder: std::path::PathBuf,
    pub(crate) npm_prefix: std::path::PathBuf,
    pub(crate) link_line: String,
}

/// Write the DeepSeek `contextWindow` overrides into `models.json`. Runs on
/// every setup; the merge keeps every other key.
fn write_deepseek(env: &super::Env, layout: &Layout) -> Result<()> {
    let models = super::doctor::configured_deepseek_models(&env.config_dir())?;
    deepseek::write_overrides(&layout.models(), &models)
}

/// Setup: pinned install, shared folder, guard, the running herdr's state
/// hook, then print the one line Rolf types. Never a login.
pub(crate) fn setup(
    runner: &dyn sh::Runner,
    env: &super::Env,
    layout: &Layout,
) -> Result<SetupReport> {
    let install = install(runner, layout)?;
    let folder = super::folder::ensure(layout)?;
    let wrapper = super::launch::write_wrapper(layout)?;
    let guard = write_guard(layout)?;
    write_deepseek(env, layout)?;

    let integration = runner.run(
        &sh::Cmd::new(env.herdr_bin(), Duration::from_secs(120))
            .own_group()
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

    let steps = vec![
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

    #[test]
    fn npm_argv_is_pinned_and_never_global() {
        let layout = Layout::for_test("/state/pi");
        let args = npm_install_args(&layout);
        assert!(args.contains(&"--prefix".to_string()));
        assert!(args.contains(&"/state/pi/npm".to_string()));
        assert!(args.contains(&"--save-exact".to_string()));
        assert!(args.contains(&format!("{PI_PACKAGE}@0.99.1")));
        assert!(!args.iter().any(|a| a == "-g" || a == "--global"));
        assert!(!args.iter().any(|a| a.contains('^')));
    }

    #[test]
    fn a_guard_with_the_current_marker_but_old_untyped_behavior_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pi"));
        std::fs::create_dir_all(layout.extensions()).unwrap();
        std::fs::write(
            layout.guard(),
            "// herdr-pi-guard:version=3\npi.exec(\"ha\", [\"waiting\", \"fetch failed\"]);\n",
        )
        .unwrap();

        assert!(!guard_ok(&layout));
        write_guard(&layout).unwrap();
        assert!(guard_ok(&layout));
        let installed = std::fs::read_to_string(layout.guard()).unwrap();
        assert!(installed.contains("fetch failed"));
        assert!(!installed.contains("__HERDR_ADE_"));
        assert!(!installed.contains("herdr-ade-hooks.json"));
        assert!(installed.contains(&serde_json::to_string(dir.path()).unwrap()));
        assert!(
            installed.contains(
                "[\"--root\", ADE_ROOT, \"failed\", \"--class\", \"provider\", \"--provider-kind\", cls, text]"
            )
        );
    }
}
