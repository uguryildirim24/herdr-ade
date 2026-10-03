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
    if !owned_tabs(herdr, workspace, false)?.is_empty() {
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

/// Herdr's plugin focus reply is the existing API that proves a pane's
/// plugin and entrypoint. Preserve focus; labels and executable names alone
/// are not ownership evidence.
fn owned_tabs(
    herdr: &Herdr,
    workspace: &str,
    single_pane_only: bool,
) -> Result<Vec<crate::herdr::Pane>, HerdrError> {
    let tabs = herdr.tab_list()?;
    if !tabs.iter().any(|tab| tab.workspace_id == workspace) {
        return Ok(Vec::new());
    }
    let panes = herdr.pane_list()?;
    let snapshot = herdr.call(&["api", "snapshot"], CALL_TIMEOUT)?;
    let focused = snapshot["snapshot"]["focused_tab_id"].as_str();
    let result = (|| {
        let mut owned = Vec::new();
        for tab in tabs.iter().filter(|tab| tab.workspace_id == workspace) {
            let in_tab: Vec<_> = panes
                .iter()
                .filter(|pane| pane.tab_id == tab.tab_id)
                .collect();
            // A split Rundown still exists, but its tab must never be closed.
            if single_pane_only && in_tab.len() != 1 {
                continue;
            }
            for pane in in_tab {
                let reply =
                    match herdr.call(&["plugin", "pane", "focus", &pane.pane_id], CALL_TIMEOUT) {
                        Ok(reply) => reply,
                        Err(error) if error.code == "plugin_pane_not_found" => continue,
                        Err(error) => return Err(error),
                    };
                let proof = &reply["plugin_pane"];
                if proof["plugin_id"] == PLUGIN
                    && proof["entrypoint"] == ENTRYPOINT
                    && proof["pane"]["pane_id"] == pane.pane_id
                    && proof["pane"]["tab_id"] == tab.tab_id
                    && proof["pane"]["workspace_id"] == workspace
                {
                    owned.push(pane.clone());
                    if !single_pane_only {
                        return Ok(owned);
                    }
                }
            }
        }
        Ok(owned)
    })();
    if let Some(focused) = focused {
        herdr.call(&["tab", "focus", focused], CALL_TIMEOUT)?;
    }
    result
}

/// Reopen only existing, proven single-pane Rundown tabs. Never add a tab to
/// a workspace that did not have one, or close a tab that acquired a split.
pub(crate) fn reopen_existing(ctx: &crate::paths::Ctx) -> anyhow::Result<()> {
    for slug in crate::project::list_slugs(&ctx.root) {
        let project = crate::project::Project::load(&ctx.root, &slug)?;
        let Some(coordinator) = project.coordinator() else {
            continue;
        };
        let herdr = Herdr::new(ctx.env.herdr_bin(), &coordinator.socket, ctx.runner);
        let owned = owned_tabs(&herdr, &coordinator.workspace_id, true)?;
        if owned.is_empty() {
            continue;
        }
        let title = herdr.workspace_label(&coordinator.workspace_id)?;
        let mut closed = false;
        for old in owned {
            let panes = herdr.pane_list()?;
            let current: Vec<_> = panes
                .iter()
                .filter(|pane| pane.tab_id == old.tab_id)
                .collect();
            if current.len() == 1
                && current[0].pane_id == old.pane_id
                && current[0].workspace_id == old.workspace_id
            {
                herdr.tab_close(&old.tab_id)?;
                closed = true;
            }
        }
        if closed {
            ensure_tab(&herdr, &coordinator.workspace_id, &ctx.root, &slug, &title)?;
        }
    }
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
    fn ensure_does_not_duplicate_a_rundown_that_has_a_split() {
        let runner = FakeRunner::new();
        runner.on(
            "tab list",
            ok(r#"{"result":{"tabs":[{"tab_id":"w1:t1","workspace_id":"w1","label":"Renamed"}]}}"#),
        );
        runner.on(
            "pane list",
            ok(r#"{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1"},{"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1"}]}}"#),
        );
        runner.on(
            "api snapshot",
            ok(r#"{"result":{"snapshot":{"focused_tab_id":"w1:t1"}}}"#),
        );
        runner.on(
            "plugin pane focus w1:p1",
            ok(r#"{"result":{"plugin_pane":{"plugin_id":"herdr-ade","entrypoint":"rundown","pane":{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1"}}}}"#),
        );
        runner.on("tab focus w1:t1", ok(r#"{"result":{}}"#));
        let herdr = Herdr::new("herdr", "scratch.sock", &runner);
        ensure_tab(&herdr, "w1", Path::new("/ade"), "demo", "Demo").unwrap();
        assert_eq!(runner.count("plugin pane open"), 0);
        assert_eq!(runner.count("tab close"), 0);
        assert_eq!(runner.count("plugin pane focus w1:p2"), 0);
        assert_eq!(runner.count("tab focus w1:t1"), 1);
    }

    #[test]
    fn refresh_closes_only_the_single_pane_with_exact_plugin_provenance() {
        for gained_split in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().join("root");
            let project = crate::project::create(&root, "demo", "", vec![]).unwrap();
            project
                .update_coordinator(|coordinator| {
                    coordinator.socket = "scratch.sock".into();
                    coordinator.workspace_id = "w1".into();
                })
                .unwrap();
            let closed = std::rc::Rc::new(std::cell::Cell::new(false));
            let runner = FakeRunner::new();
            let flag = closed.clone();
            runner.on_fn(|cmd| cmd.display().contains("tab list"), move |_| {
                let mut tabs = serde_json::json!([
                    {"tab_id":"w1:t1","workspace_id":"w1","label":"Rundown"},
                    {"tab_id":"w1:t3","workspace_id":"w1","label":"Rundown"},
                    {"tab_id":"w1:t4","workspace_id":"w1","label":"Rundown"},
                    {"tab_id":"w2:t5","workspace_id":"w2","label":"Rundown"}
                ]);
                if !flag.get() { tabs.as_array_mut().unwrap().push(serde_json::json!({"tab_id":"w1:t2","workspace_id":"w1","label":"Renamed"})); }
                Ok(ok(&serde_json::json!({"result":{"tabs":tabs}}).to_string()))
            });
            let flag = closed.clone();
            let reads = std::cell::Cell::new(0);
            runner.on_fn(|cmd| cmd.display().contains("pane list"), move |_| {
                reads.set(reads.get() + 1);
                let mut panes = serde_json::json!([
                    {"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1"},
                    {"pane_id":"w1:p3","tab_id":"w1:t3","workspace_id":"w1"},
                    {"pane_id":"w1:p4","tab_id":"w1:t3","workspace_id":"w1"},
                    {"pane_id":"w1:p5","tab_id":"w1:t4","workspace_id":"w1"},
                    {"pane_id":"w2:p1","tab_id":"w2:t5","workspace_id":"w2"}
                ]);
                if !flag.get() { panes.as_array_mut().unwrap().push(serde_json::json!({"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1"})); }
                if gained_split && reads.get() > 1 { panes.as_array_mut().unwrap().push(serde_json::json!({"pane_id":"w1:p6","tab_id":"w1:t2","workspace_id":"w1"})); }
                Ok(ok(&serde_json::json!({"result":{"panes":panes}}).to_string()))
            });
            runner.on(
                "api snapshot",
                ok(r#"{"result":{"snapshot":{"focused_tab_id":"w1:t1"}}}"#),
            );
            runner.on(
                "plugin pane focus w1:p1",
                fail(
                    1,
                    r#"{"error":{"code":"plugin_pane_not_found","message":"not a plugin pane"}}"#,
                ),
            );
            runner.on("plugin pane focus w1:p2", ok(r#"{"result":{"plugin_pane":{"plugin_id":"herdr-ade","entrypoint":"rundown","pane":{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1"}}}}"#));
            runner.on("plugin pane focus w1:p5", ok(r#"{"result":{"plugin_pane":{"plugin_id":"other","entrypoint":"rundown","pane":{"pane_id":"w1:p5","tab_id":"w1:t4","workspace_id":"w1"}}}}"#));
            runner.on("tab focus w1:t1", ok(r#"{"result":{}}"#));
            let flag = closed.clone();
            runner.on_fn(
                |cmd| cmd.display().contains("tab close w1:t2"),
                move |_| {
                    flag.set(true);
                    Ok(ok(r#"{"result":{}}"#))
                },
            );
            runner.on(
                "workspace get w1",
                ok(r#"{"result":{"workspace":{"label":"Demo"}}}"#),
            );
            runner.on(
                "plugin pane open",
                ok(r#"{"result":{"plugin_pane":{"pane":{"tab_id":"w1:t6"}}}}"#),
            );
            runner.on("tab rename w1:t6 Rundown", ok(r#"{"result":{}}"#));
            let env = crate::paths::Env::for_test(home.path(), &[]);
            let ctx = crate::paths::Ctx {
                env: &env,
                root,
                config_dir: home.path().join("cfg"),
                runner: &runner,
                detached_ticker: false,
            };
            reopen_existing(&ctx).unwrap();
            assert_eq!(runner.count("tab close"), usize::from(!gained_split));
            assert_eq!(runner.count("plugin pane open"), usize::from(!gained_split));
            assert!(runner.count("tab focus w1:t1") > 0);
            for untouched in ["w1:p3", "w1:p4", "w2:p1"] {
                assert_eq!(runner.count(&format!("plugin pane focus {untouched}")), 0);
            }
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
