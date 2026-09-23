//! Staleness: the seven things that can be older than the program (LEAN U4).
//!
//! Every check compares what is running with what is installed and names the
//! exact thing to do. A check that cannot be read says nothing; it never
//! invents a stale row.
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::paths::Ctx;
use crate::project::Project;
use crate::runner::{Cmd, Runner};
use crate::thread;

const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const BOX_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) what: String,
    pub(crate) remedy: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Stale {
    pub(crate) items: Vec<Item>,
    /// At least one check could not be read. Unknown is not the same as
    /// current, so the screen says so instead of staying silent.
    pub(crate) unknown: bool,
}

/// Compare what is running with what is installed. A check that can be read
/// and is current says nothing; a check that cannot be read is recorded as
/// unknown.
pub(crate) fn scan(ctx: &Ctx, project: &Project) -> Stale {
    let mut stale = Stale::default();

    // 1. This project screen, the running plugin binary against the installed
    //    one. The running process reports its own build id.
    let installed_plugin = ctx.env.home.join(".local/bin/herdr-ade");
    if let Some(version) = plugin_version(ctx.runner, &installed_plugin)
        && !crate::build::same_commit(&version, crate::VERSION)
    {
        stale.items.push(Item {
            what: "This project screen is older than the installed program.".into(),
            remedy: format!(
                "Restart this screen: press Ctrl+C, then run `ha talk {}`.",
                project.slug
            ),
        });
    }

    // 2. `status client` describes the short-lived CLI that runs the command,
    // not the already-open window. Find that long-running `herdr` process and
    // compare its start with the installed binary's modification time. Fork
    // builds keep the same 0.9.1 version, so a version comparison cannot see
    // this stale window.
    let herdr_bin = ctx.env.herdr_bin();
    if let Some(client) = json(
        ctx.runner,
        &Cmd::new(herdr_bin.clone(), CHECK_TIMEOUT).args(["status", "client", "--json"]),
    ) && client
        .get("binary")
        .and_then(Value::as_str)
        .is_some_and(|binary| client_started_before_install(ctx.runner, Path::new(binary)))
    {
        stale.items.push(Item {
            what: "The client window is older than the installed program.".into(),
            remedy: "Reopen the client window so it runs the installed program.".into(),
        });
    }

    // 3. The local herdr server image.
    match server_stale(ctx.runner, &herdr_bin, None) {
        Some(true) => stale.items.push(Item {
            what: "The local herdr server is still running an older program.".into(),
            remedy: "Hand the server over to the installed program: `herdr server restart`.".into(),
        }),
        None => stale.unknown = true,
        Some(false) => {}
    }

    // 4. The box's herdr server image.
    if let Some((target, path)) = box_target(ctx, project) {
        match server_stale(ctx.runner, &herdr_bin, Some((&target, &path))) {
            Some(true) => {
                let restart = format!(
                    "PATH={}; export PATH; herdr server restart",
                    crate::remote::quote(&path)
                );
                stale.items.push(Item {
                    what: "The box's herdr server is still running an older program.".into(),
                    remedy: format!(
                        "Hand the box server over: `ssh {} {}`.",
                        crate::remote::quote(&target),
                        crate::remote::quote(&restart)
                    ),
                });
            }
            None => stale.unknown = true,
            Some(false) => {}
        }
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
            if brief_behind(&lane) {
                stale.items.push(Item {
                    what: format!("Lane {} is running an older brief.", lane.id),
                    remedy: format!("Restart lane {} so it reads the current brief.", lane.id),
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

/// The version reported by the installed plugin binary, if it can be read.
/// The running screen compares this with its own `VERSION` before handing over.
pub(crate) fn installed_version(ctx: &Ctx) -> Option<String> {
    plugin_version(ctx.runner, &ctx.env.home.join(".local/bin/herdr-ade"))
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

fn client_started_before_install(runner: &dyn Runner, binary: &Path) -> bool {
    let installed = std::fs::metadata(binary)
        .and_then(|metadata| metadata.modified())
        .ok();
    let Some(installed) = installed else {
        return false;
    };
    let output = runner
        .run(&Cmd::new("/bin/ps", CHECK_TIMEOUT).args(["-axo", "pid=,comm=,args="]))
        .ok();
    let Some(output) = output.filter(|output| output.success()) else {
        return false;
    };
    for line in output.stdout.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 3
            || Path::new(fields[2])
                .file_name()
                .and_then(|name| name.to_str())
                != Some("herdr")
        {
            continue;
        }
        let elapsed = runner
            .run(&Cmd::new("/bin/ps", CHECK_TIMEOUT).args(["-p", fields[0], "-o", "etime="]))
            .ok()
            .filter(|output| output.success())
            .and_then(|output| parse_elapsed(output.stdout.trim()));
        if let Some(elapsed) = elapsed
            && std::time::SystemTime::now()
                .checked_sub(elapsed)
                .is_some_and(|started| started < installed)
        {
            return true;
        }
    }
    false
}

fn parse_elapsed(text: &str) -> Option<Duration> {
    let (days, clock) = match text.rsplit_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, text),
    };
    let fields: Vec<_> = clock.split(':').collect();
    let (hours, minutes, seconds): (u64, u64, u64) = match fields.as_slice() {
        [minutes, seconds] => (0, minutes.parse().ok()?, seconds.parse().ok()?),
        [hours, minutes, seconds] => (
            hours.parse().ok()?,
            minutes.parse().ok()?,
            seconds.parse().ok()?,
        ),
        _ => return None,
    };
    Some(Duration::from_secs(
        days * 86_400 + hours * 3_600 + minutes * 60 + seconds,
    ))
}

fn json(runner: &dyn Runner, cmd: &Cmd) -> Option<Value> {
    let output = runner.run(cmd).ok()?;
    if !output.success() {
        return None;
    }
    serde_json::from_str(&output.stdout).ok()
}

/// The server says whether its running image matches the installed binary.
/// `None` means the check could not be read, which is not the same as current.
/// A remote check asks the box for its own `herdr` on the box's `PATH`; the
/// plugin's binary path is a Mac path and does not exist there.
fn server_stale(runner: &dyn Runner, bin: &str, remote: Option<(&str, &str)>) -> Option<bool> {
    let output = match remote {
        Some((target, path)) => crate::remote::ssh_check(
            runner,
            target,
            &crate::remote::with_path(path, "herdr status server --json"),
            BOX_CHECK_TIMEOUT,
            "server-status",
        )
        .ok(),
        None => runner
            .run(&Cmd::new(bin, CHECK_TIMEOUT).args(["status", "server", "--json"]))
            .ok(),
    }?;
    if !output.success() {
        return None;
    }
    let value = serde_json::from_str::<Value>(&output.stdout).ok()?;
    Some(flag(&value, "server_binary_stale") || flag(&value, "restart_needed"))
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The SSH target of the machine this project's lanes run on.
fn box_target(ctx: &Ctx, project: &Project) -> Option<(String, String)> {
    let machine = thread::list(project)
        .into_iter()
        .find(|lane| lane.is_remote())
        .map(|lane| lane.machine_route().to_string())?;
    let profile =
        crate::remote::machine_profile(ctx.runner, &ctx.env.herdr_bin(), &ctx.config_dir, &machine)
            .ok()?;
    let declaration = crate::remote::machine_declaration(&ctx.config_dir, &profile.label).ok()?;
    (!profile.target.is_empty()).then_some((profile.target, declaration.path))
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

/// Local lanes read their frozen runtime `brief.md`. A remote brief lives on
/// its box and its bootstrap receipt is checked by the courier instead.
fn brief_behind(lane: &thread::Thread) -> bool {
    if lane.launch.brief_hash.is_empty() {
        return false;
    }
    if lane.is_remote() {
        return false;
    }
    let path = PathBuf::from(&lane.thread_dir).join("brief.md");
    std::fs::read(&path)
        .ok()
        .is_some_and(|text| thread::sha256_hex(&text) != lane.launch.brief_hash)
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

    #[test]
    fn a_client_started_before_the_installed_binary_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("herdr");
        std::fs::write(&binary, b"new binary").unwrap();
        let runner = crate::runner::fake::FakeRunner::new();
        runner.on("-axo", ok("42 herdr herdr\n"));
        runner.on("-p 42", ok("01:00:00\n"));
        assert!(client_started_before_install(&runner, &binary));
        assert_eq!(
            parse_elapsed("2-03:04:05"),
            Some(Duration::from_secs(183_845))
        );
    }

    #[test]
    fn a_changed_lane_brief_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("brief.md"), "new").unwrap();
        let lane = thread::Thread {
            id: "t-0001".into(),
            worktree_path: dir.path().to_string_lossy().into_owned(),
            thread_dir: dir.path().to_string_lossy().into_owned(),
            launch: crate::contracts::Launch {
                brief_hash: thread::sha256_hex(b"old"),
                ..crate::contracts::Launch::default()
            },
            ..thread::Thread::default()
        };
        assert!(brief_behind(&lane));
    }

    #[test]
    fn the_box_server_check_asks_the_box_for_its_own_herdr() {
        let runner = crate::runner::fake::FakeRunner::new();
        runner.on("herdr status server", ok(r#"{"server_binary_stale":true}"#));
        let state = server_stale(
            &runner,
            "/home/agent/.local/bin/herdr",
            Some(("remote-host", "/custom/bin:/bin")),
        );
        assert_eq!(state, Some(true));
        let calls: Vec<String> = runner.calls.borrow().iter().map(|c| c.display()).collect();
        assert!(
            calls.iter().any(|c| c.contains("herdr status server")),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.contains("PATH=/custom/bin:/bin; export PATH")),
            "{calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c.contains("/home/agent")),
            "the Mac path must not cross to the box: {calls:?}"
        );
    }

    #[test]
    fn an_unreadable_server_check_is_unknown_not_current() {
        let runner = crate::runner::fake::FakeRunner::new();
        // No rule: the check fails, and that is unknown, not fine.
        assert_eq!(
            server_stale(&runner, "herdr", Some(("remote-host", "/bin"))),
            None
        );
        runner.on("status server", ok("not json"));
        assert_eq!(server_stale(&runner, "herdr", None), None);
    }
}
