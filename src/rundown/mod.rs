//! The Rundown tab's two hooks in `herdr-ade`: `ha harness install` links the
//! `rundown` plugin folder, and `ha open` adds the tab to a project's
//! workspace when it is missing. The tab itself is `herdr-rundown`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::herdr::{CALL_TIMEOUT, Herdr, HerdrError};
use crate::paths::Ctx;
use crate::runner::Cmd;

const PLUGIN: &str = "rundown";
const ENTRYPOINT: &str = "rundown";
pub(crate) const LABEL: &str = "Rundown";
const LINK_TIMEOUT: Duration = Duration::from_secs(30);

/// Registers `<repo>/rundown` with herdr. Linking again replaces the entry,
/// so every install points the plugin at the repository it just built.
pub(crate) fn link_plugin(ctx: &Ctx, repo: &str) -> Result<()> {
    let folder = Path::new(repo).join(PLUGIN);
    let out = ctx.runner.run(
        &Cmd::new(ctx.env.herdr_bin(), LINK_TIMEOUT)
            .args(["plugin", "link"])
            .arg(folder.to_string_lossy().into_owned()),
    )?;
    if !out.success() {
        bail!(
            "`herdr plugin link {}` failed: {}",
            folder.display(),
            out.error_text()
        );
    }
    Ok(())
}

/// Adds the Rundown tab to `workspace` unless one is already there. Never
/// fails `open`: a missing tab is reported in one line.
pub(crate) fn ensure_tab(herdr: &Herdr, workspace: &str, root: &Path, slug: &str, title: &str) {
    match add_tab(herdr, workspace, root, slug, title) {
        Ok(Some(tab)) => println!("added the Rundown tab ({tab})"),
        Ok(None) => {}
        Err(error) => println!("could not add the Rundown tab: {error}"),
    }
}

fn add_tab(
    herdr: &Herdr,
    workspace: &str,
    root: &Path,
    slug: &str,
    title: &str,
) -> Result<Option<String>, HerdrError> {
    if herdr
        .tab_list()?
        .iter()
        .any(|tab| tab.workspace_id == workspace && tab.label == LABEL)
    {
        return Ok(None);
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
    Ok(Some(tab.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, ok};

    #[test]
    fn open_adds_the_tab_once_and_names_it() {
        let runner = FakeRunner::new();
        runner.on(
            "tab list",
            ok(
                r#"{"result":{"tabs":[{"tab_id":"w1:t1","workspace_id":"w1","label":"coordinator"},
                {"tab_id":"w2:t1","workspace_id":"w2","label":"Rundown"}]}}"#,
            ),
        );
        runner.on(
            "plugin pane open",
            ok(r#"{"result":{"plugin_pane":{"pane":{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1"}}}}"#),
        );
        runner.on("tab rename", ok(r#"{"result":{}}"#));
        let herdr = Herdr::new("herdr", "/tmp/sock", &runner);

        let added = add_tab(&herdr, "w1", Path::new("/root"), "demo", "Demo Project").unwrap();
        assert_eq!(added.as_deref(), Some("w1:t2"));
        let calls = runner.calls.borrow();
        let open = calls
            .iter()
            .find(|c| c.display().contains("plugin pane open"))
            .unwrap();
        for arg in [
            "HERDR_RUNDOWN_PROJECT=demo",
            "HERDR_RUNDOWN_TITLE=Demo Project",
            "HERDR_ADE_ROOT=/root",
            "--no-focus",
        ] {
            assert!(open.args.iter().any(|a| a == arg), "{arg}: {:?}", open.args);
        }
        assert!(
            calls
                .iter()
                .any(|c| c.args.join(" ").ends_with("tab rename w1:t2 Rundown"))
        );
        drop(calls);

        assert_eq!(
            add_tab(&herdr, "w2", Path::new("/root"), "other", "Other").unwrap(),
            None,
            "a workspace that has the tab keeps it"
        );
        assert_eq!(runner.count("plugin pane open"), 1);
    }
}
