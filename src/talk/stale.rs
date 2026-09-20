//! Staleness: the seven things that can be older than the program (LEAN U4).
//!
//! Every check compares what is running with what is installed and names the
//! exact thing to do. A check that cannot be read says nothing; it never
//! invents a stale row.
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::herdr;
use crate::paths::Ctx;
use crate::project::Project;
use crate::runner::{Cmd, Runner};
use crate::thread;

const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub what: String,
    pub remedy: String,
}

#[derive(Debug, Clone, Default)]
pub struct Stale {
    pub items: Vec<Item>,
}

/// Compare what is running with what is installed. Best effort: an unreadable
/// source produces no row.
pub fn scan(ctx: &Ctx, project: &Project) -> Stale {
    let mut stale = Stale::default();

    // 1. This project screen, the running plugin binary against the installed
    //    one. The running process reports its own build id.
    let installed_plugin = ctx.env.home.join(".local/bin/herdr-ade");
    if let Some(version) = plugin_version(ctx.runner, &installed_plugin)
        && version.as_str() != crate::VERSION
    {
        stale.items.push(Item {
            what: "This project screen is older than the installed program.".into(),
            remedy: format!(
                "Restart this screen: press Ctrl+C, then run `ha talk {}`.",
                project.slug
            ),
        });
    }

    // 2. The client window against the installed herdr binary.
    let herdr_bin = ctx.env.herdr_bin();
    if let Some(client) = json(
        ctx.runner,
        &Cmd::new(herdr_bin.clone(), CHECK_TIMEOUT).args(["status", "client", "--json"]),
    ) {
        let running = client
            .get("version")
            .and_then(Value::as_str)
            .and_then(herdr::parse_version);
        if let Some(installed) = herdr_version(ctx.runner, &herdr_bin)
            && running.is_some_and(|version| version != installed)
        {
            stale.items.push(Item {
                what: "The client window is older than the installed program.".into(),
                remedy: "Reopen the client window so it runs the installed program.".into(),
            });
        }
    }

    // 3. The local herdr server image.
    if server_stale(ctx.runner, &herdr_bin, None) {
        stale.items.push(Item {
            what: "The local herdr server is still running an older program.".into(),
            remedy: "Hand the server over to the installed program: `herdr server restart`.".into(),
        });
    }

    // 4. The box's herdr server image.
    if let Some(target) = box_target(ctx, project)
        && server_stale(ctx.runner, &herdr_bin, Some(&target))
    {
        stale.items.push(Item {
            what: "The box's herdr server is still running an older program.".into(),
            remedy: format!("Hand the box server over: `ssh {target} herdr server restart`."),
        });
    }

    // 5 and 6. A skill file that moved on since the agent was primed.
    if let Some(repo) = plugin_repo(ctx) {
        if let Some(coordinator) = project.coordinator()
            && skill_behind(&repo, "coordinator", &coordinator.launch.skill_hash)
        {
            stale.items.push(Item {
                what: "The coordinator is running older skill instructions.".into(),
                remedy: "Restart the coordinator pane so it reads the new instructions.".into(),
            });
        }
        for lane in thread::list(project) {
            if lane.status == thread::Status::Resolved {
                continue;
            }
            let role = if lane.role.is_empty() {
                "lane"
            } else {
                lane.role.as_str()
            };
            if skill_behind(&repo, role, &lane.launch.skill_hash) {
                stale.items.push(Item {
                    what: format!("Lane {} is running older skill instructions.", lane.id),
                    remedy: format!("Restart lane {} so it reads the new instructions.", lane.id),
                });
            }
        }
    }

    // 7. pi points at a relay port the relay no longer uses.
    if let Some(remedy) = pi_port_behind(ctx) {
        stale.items.push(Item {
            what: "The helper program points at an old port.".into(),
            remedy,
        });
    }

    stale
}

fn plugin_version(runner: &dyn Runner, path: &Path) -> Option<String> {
    let output = runner
        .run(&Cmd::new(path.to_string_lossy().into_owned(), CHECK_TIMEOUT).arg("--version"))
        .ok()?;
    if !output.success() {
        return None;
    }
    output
        .stdout
        .split_whitespace()
        .find(|token| token.chars().any(|c| c.is_ascii_digit()) && token.contains('+'))
        .map(str::to_string)
}

fn herdr_version(runner: &dyn Runner, bin: &str) -> Option<herdr::Version> {
    let output = runner
        .run(&Cmd::new(bin, CHECK_TIMEOUT).arg("--version"))
        .ok()?;
    if !output.success() {
        return None;
    }
    herdr::parse_version(&output.stdout)
}

fn json(runner: &dyn Runner, cmd: &Cmd) -> Option<Value> {
    let output = runner.run(cmd).ok()?;
    if !output.success() {
        return None;
    }
    serde_json::from_str(&output.stdout).ok()
}

/// The server says whether its running image matches the installed binary.
fn server_stale(runner: &dyn Runner, bin: &str, target: Option<&str>) -> bool {
    let output = match target {
        Some(target) => crate::remote::ssh(
            runner,
            target,
            &format!("{bin} status server --json"),
            None,
            CHECK_TIMEOUT,
        )
        .ok(),
        None => runner
            .run(&Cmd::new(bin, CHECK_TIMEOUT).args(["status", "server", "--json"]))
            .ok(),
    };
    let Some(output) = output else {
        return false;
    };
    if !output.success() {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(&output.stdout) else {
        return false;
    };
    flag(&value, "server_binary_stale") || flag(&value, "restart_needed")
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The SSH target of the machine this project's lanes run on.
fn box_target(ctx: &Ctx, project: &Project) -> Option<String> {
    let machine = thread::list(project)
        .into_iter()
        .find(|lane| lane.is_remote())
        .map(|lane| lane.machine_route().to_string())?;
    let profile =
        crate::remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, &machine)
            .ok()?;
    (!profile.target.is_empty()).then_some(profile.target)
}

/// The plugin source checkout, where the skill files live.
fn plugin_repo(ctx: &Ctx) -> Option<PathBuf> {
    for repo in crate::harness::repos(&ctx.config_dir).ok()? {
        let path = PathBuf::from(repo.path);
        if path.join("skill/COORDINATOR.md").is_file() {
            return Some(path);
        }
    }
    None
}

/// True when the skill file on disk no longer matches what the agent was
/// primed with. An unrecorded or unreadable skill is never called stale.
fn skill_behind(repo: &Path, role: &str, recorded: &str) -> bool {
    if recorded.is_empty() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(repo.join("skill").join(crate::lane::skill_file(role)))
    else {
        return false;
    };
    thread::sha256_hex(text.as_bytes()) != recorded
}

/// pi's `pro` provider URL against the relay's `serve.json` port.
fn pi_port_behind(ctx: &Ctx) -> Option<String> {
    let models = std::fs::read_to_string(ctx.root.join("pi/agent/models.json")).ok()?;
    let models: Value = serde_json::from_str(&models).ok()?;
    let configured = models.pointer("/providers/pro/baseUrl")?.as_str()?;
    let configured = url_port(configured)?;
    let serve = std::fs::read_to_string(ctx.root.join("pro-bridge/serve.json")).ok()?;
    let serve: Value = serde_json::from_str(&serve).ok()?;
    let relay = serve.get("port")?.as_u64()? as u16;
    (configured != relay)
        .then(|| "Run `herdr-pi setup` so the helper uses the relay's current port.".into())
}

fn url_port(url: &str) -> Option<u16> {
    let rest = url.split("://").nth(1)?;
    let authority = rest.split('/').next()?;
    let port = authority.rsplit_once(':')?.1;
    port.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::testkit::fixture;
    use crate::runner::fake::ok;

    #[test]
    fn a_screen_older_than_the_installed_program_is_named_with_its_remedy() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let installed = ctx
            .env
            .home
            .join(".local/bin/herdr-ade")
            .to_string_lossy()
            .into_owned();
        let needle = installed.clone();
        fx.world.runner.on_fn(
            move |cmd| cmd.program == needle,
            |_| Ok(ok("herdr-ade 9.9.9+old.1\n")),
        );
        let stale = scan(&ctx, &fx.project);
        let row = stale
            .items
            .iter()
            .find(|item| item.what.contains("screen is older"))
            .expect("a stale screen row");
        assert!(row.remedy.contains("Ctrl+C"), "{}", row.remedy);
        assert!(row.remedy.contains("ha talk demo"), "{}", row.remedy);
    }

    #[test]
    fn a_current_screen_and_matching_ports_say_nothing() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let installed = ctx
            .env
            .home
            .join(".local/bin/herdr-ade")
            .to_string_lossy()
            .into_owned();
        let needle = installed.clone();
        fx.world.runner.on_fn(
            move |cmd| cmd.program == needle,
            |_| Ok(ok(&format!("herdr-ade {}\n", crate::VERSION))),
        );
        std::fs::create_dir_all(ctx.root.join("pi/agent")).unwrap();
        std::fs::write(
            ctx.root.join("pi/agent/models.json"),
            r#"{"providers":{"pro":{"baseUrl":"http://127.0.0.1:1234/v1"}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(ctx.root.join("pro-bridge")).unwrap();
        std::fs::write(ctx.root.join("pro-bridge/serve.json"), r#"{"port":1234}"#).unwrap();
        let stale = scan(&ctx, &fx.project);
        assert!(stale.items.is_empty(), "{:?}", stale.items);
    }

    #[test]
    fn an_old_relay_port_is_named_with_its_remedy() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let installed = ctx
            .env
            .home
            .join(".local/bin/herdr-ade")
            .to_string_lossy()
            .into_owned();
        let needle = installed.clone();
        fx.world.runner.on_fn(
            move |cmd| cmd.program == needle,
            |_| Ok(ok(&format!("herdr-ade {}\n", crate::VERSION))),
        );
        std::fs::create_dir_all(ctx.root.join("pi/agent")).unwrap();
        std::fs::write(
            ctx.root.join("pi/agent/models.json"),
            r#"{"providers":{"pro":{"baseUrl":"http://127.0.0.1:1111/v1"}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(ctx.root.join("pro-bridge")).unwrap();
        std::fs::write(ctx.root.join("pro-bridge/serve.json"), r#"{"port":2222}"#).unwrap();
        let stale = scan(&ctx, &fx.project);
        let row = stale
            .items
            .iter()
            .find(|item| item.what.contains("old port"))
            .expect("a stale port row");
        assert!(row.remedy.contains("herdr-pi setup"), "{}", row.remedy);
    }
}
