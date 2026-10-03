//! `ha open` adds ADE's Rundown pane to a project's workspace when it is
//! missing. The tab itself is `herdr-rundown`.

use std::path::Path;

use crate::herdr::{CALL_TIMEOUT, Herdr, HerdrError};

const PLUGIN: &str = "herdr-ade";
const ENTRYPOINT: &str = "rundown";
pub(crate) const LABEL: &str = "Rundown";

/// Adds the Rundown tab to `workspace` unless one is already there.
pub(crate) fn ensure_tab(
    herdr: &Herdr,
    workspace: &str,
    root: &Path,
    slug: &str,
    title: &str,
) -> Result<(), HerdrError> {
    if herdr
        .tab_list()?
        .iter()
        .any(|tab| tab.workspace_id == workspace && tab.label == LABEL)
    {
        return Ok(());
    }
    let project = format!("HERDR_RUNDOWN_PROJECT={slug}");
    let name = format!("HERDR_RUNDOWN_TITLE={title}");
    let root = format!("HERDR_ADE_ROOT={}", root.display());
    let opened = herdr.call(
        &[
            "plugin",
            "pane",
            "open",
            "--plugin",
            PLUGIN,
            "--entrypoint",
            ENTRYPOINT,
            "--placement",
            "tab",
            "--workspace",
            workspace,
            "--env",
            &project,
            "--env",
            &name,
            "--env",
            &root,
            "--no-focus",
        ],
        CALL_TIMEOUT,
    )?;
    let Some(tab) = opened["plugin_pane"]["pane"]["tab_id"].as_str() else {
        return Err(HerdrError {
            code: "failed".into(),
            message: "herdr's plugin pane reply has no tab id".into(),
        });
    };
    herdr.tab_rename(tab, LABEL)?;
    println!("added the Rundown tab ({tab})");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, fail, ok};

    #[test]
    fn open_uses_the_rundown_pane_shipped_in_ade() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../../herdr-plugin.toml")).unwrap();
        assert_eq!(manifest["id"].as_str(), Some(PLUGIN));
        let pane = manifest["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pane| pane["id"].as_str() == Some(ENTRYPOINT))
            .unwrap();
        assert_eq!(pane["placement"].as_str(), Some("tab"));
        assert_eq!(
            pane["command"][0].as_str(),
            Some("target/release/herdr-rundown")
        );
        assert!(
            manifest["actions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|action| { !action["id"].as_str().unwrap().starts_with("pi-") })
        );

        let runner = FakeRunner::new();
        runner.on("tab list", ok(r#"{"result":{"tabs":[]}}"#));
        runner.on(
            "plugin pane open --plugin herdr-ade --entrypoint rundown",
            ok(r#"{"result":{"plugin_pane":{"pane":{"tab_id":"w1:t2"}}}}"#),
        );
        runner.on("tab rename w1:t2 Rundown", ok(r#"{"result":{}}"#));
        let herdr = Herdr::new("herdr", "scratch.sock", &runner);
        ensure_tab(&herdr, "w1", Path::new("/ade"), "demo", "Demo").unwrap();
        assert_eq!(runner.count("plugin pane open"), 1);
        let calls = runner.calls.borrow();
        let opened = calls
            .iter()
            .find(|call| call.display().contains("plugin pane open"))
            .unwrap();
        for argument in [
            "HERDR_RUNDOWN_PROJECT=demo",
            "HERDR_RUNDOWN_TITLE=Demo",
            "HERDR_ADE_ROOT=/ade",
            "--no-focus",
        ] {
            assert!(opened.args.iter().any(|arg| arg == argument));
        }
    }

    #[test]
    fn a_failed_rundown_open_is_an_error_not_a_success_with_a_warning() {
        let runner = FakeRunner::new();
        runner.on("tab list", ok(r#"{"result":{"tabs":[]}}"#));
        runner.on(
            "plugin pane open",
            fail(
                1,
                r#"{"error":{"code":"plugin_not_found","message":"ADE is not installed"}}"#,
            ),
        );
        let herdr = Herdr::new("herdr", "scratch.sock", &runner);
        assert_eq!(
            ensure_tab(&herdr, "w1", Path::new("/ade"), "demo", "Demo")
                .unwrap_err()
                .code,
            "plugin_not_found"
        );
    }
}
