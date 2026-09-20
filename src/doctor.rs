//! `doctor`: what is installed, where things resolve, and whether it fits.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::herdr::{self, Herdr};
use crate::paths::{self, Ctx, Env, SessionFlags};
use crate::project;
use crate::runner::{Cmd, Runner};

const TOOL_TIMEOUT: Duration = Duration::from_secs(10);

/// Prints the report and returns whether every required check passed.
pub fn run(ctx: &Ctx, session: &SessionFlags) -> Result<bool> {
    let (mut text, mut healthy) = report(ctx.env, &ctx.root, &ctx.config_dir, session, ctx.runner);
    // The pi rows read the process's own layout (SPEC-pi §3.4).
    match crate::pi_ade::doctor_rows_with(ctx.runner, &ctx.root) {
        Ok((rows, pi_healthy)) => {
            healthy &= pi_healthy;
            for row in rows {
                let _ = writeln!(text, "{}", row.line());
            }
        }
        Err(error) => {
            healthy = false;
            let _ = writeln!(text, "[FAIL] pi: {error:#}");
        }
    }
    print!("{text}");
    Ok(healthy)
}

fn report(
    env: &Env,
    root: &Path,
    config_dir: &Path,
    session: &SessionFlags,
    runner: &dyn Runner,
) -> (String, bool) {
    let mut out = String::new();
    let mut healthy = true;
    let mut check = |out: &mut String, ok: Option<bool>, label: &str, detail: String| {
        let mark = match ok {
            Some(true) => "ok  ",
            Some(false) => {
                healthy = false;
                "FAIL"
            }
            None => "warn",
        };
        let _ = writeln!(out, "[{mark}] {label}: {detail}");
    };

    let binary = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|e| format!("unknown ({e})"));
    let prefix = crate::coordinator::command_prefix(Path::new(&binary), root);
    let _ = writeln!(out, "plugin:     herdr-ade");
    let _ = writeln!(out, "crate:      herdr-ade");
    let _ = writeln!(out, "binary:     {binary}");
    let _ = writeln!(out, "prefix:     {prefix}");
    let _ = writeln!(out, "version:    {}", crate::VERSION);
    let _ = writeln!(out, "root:       {}", root.display());
    let _ = writeln!(out, "config dir: {}", config_dir.display());
    let _ = writeln!(
        out,
        "ticker:     {}",
        crate::ticker::lock_path(root).display()
    );
    let manifest: toml::Table =
        toml::from_str(include_str!("../herdr-plugin.toml")).unwrap_or_default();
    let commands = |key: &str, field: &str| -> Vec<String> {
        manifest
            .get(key)
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry.get(field)? {
                toml::Value::String(s) => Some(s.clone()),
                toml::Value::Array(a) => Some(
                    a.iter()
                        .filter_map(toml::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
            .collect()
    };
    let _ = writeln!(
        out,
        "startup:    {}",
        commands("startup", "command").join("; ")
    );
    let mut actions = commands("actions", "id");
    actions.sort();
    actions.dedup();
    let _ = writeln!(out, "actions:    {}", actions.join(", "));
    let _ = writeln!(out, "log:        herdr plugin log --plugin herdr-ade");
    let _ = writeln!(out);

    let bin = env.herdr_bin();
    let parent_cli = herdr::parent_on_start_supported(&bin, runner);
    match herdr::version(&bin, runner) {
        Ok(version) if version >= herdr::MIN_VERSION => {
            check(&mut out, Some(true), "herdr", format!("{version} ({bin})"))
        }
        Ok(version) if version == herdr::Version(0, 9, 0) && parent_cli => check(
            &mut out,
            Some(true),
            "herdr",
            format!("{version} ({bin}); fork string 0.9.0 accepted until install day (0.9.1)"),
        ),
        Ok(version) => check(
            &mut out,
            Some(false),
            "herdr",
            format!(
                "{version} ({bin}); {} or later is required",
                herdr::MIN_VERSION
            ),
        ),
        Err(error) => check(&mut out, Some(false), "herdr", format!("{error:#}")),
    }
    check(
        &mut out,
        if parent_cli { Some(true) } else { None },
        "parent",
        if parent_cli {
            "CLI `agent start --parent` (fork)".into()
        } else {
            "CLI has no `--parent`; post-start fallback only".into()
        },
    );

    match paths::resolve_session(session, env, runner) {
        Ok(found) => {
            let reachable = Herdr::new(&bin, &found.socket, runner).reachable();
            let name = found.name.as_deref().unwrap_or("-");
            check(
                &mut out,
                if reachable { Some(true) } else { None },
                "session",
                format!(
                    "{} (name: {name}){}",
                    found.socket.display(),
                    if reachable { "" } else { "; not reachable" }
                ),
            );
        }
        Err(error) => check(&mut out, Some(false), "session", format!("{error:#}")),
    }

    for (tool, args, required) in [
        ("git", vec!["--version"], true),
        ("ssh", vec!["-V"], true),
        ("rsync", vec!["--version"], false),
        ("gh", vec!["--version"], false),
    ] {
        let result = runner.run(&Cmd::new(tool, TOOL_TIMEOUT).args(args));
        match result {
            Ok(o) if o.success() => {
                let text = if o.stdout.trim().is_empty() {
                    &o.stderr
                } else {
                    &o.stdout
                };
                let line = text.lines().next().unwrap_or("").trim().to_string();
                check(&mut out, Some(true), tool, line);
            }
            Ok(o) => check(&mut out, required.then_some(false), tool, o.error_text()),
            Err(error) => check(
                &mut out,
                required.then_some(false),
                tool,
                format!("{error:#}"),
            ),
        }
    }
    match runner.run(&Cmd::new("gh", TOOL_TIMEOUT).args(["auth", "status"])) {
        Ok(o) if o.success() => check(&mut out, Some(true), "gh auth", "logged in".into()),
        Ok(o) => check(
            &mut out,
            None,
            "gh auth",
            format!(
                "{}; pull request follow-up will not work",
                o.error_text().lines().next().unwrap_or("not logged in")
            ),
        ),
        Err(_) => check(&mut out, None, "gh auth", "gh is not installed".into()),
    }

    if root.is_dir() {
        let count = project::list_slugs(root).len();
        check(&mut out, Some(true), "root", format!("{count} project(s)"));
    } else {
        check(
            &mut out,
            None,
            "root",
            "does not exist yet; `new` creates it".into(),
        );
    }

    match crate::ticker::lock_state(root) {
        crate::ticker::LockState::Free => check(&mut out, None, "ticker", "not running".into()),
        crate::ticker::LockState::Held(info) => check(
            &mut out,
            Some(true),
            "ticker",
            format!(
                "running, version {} (this binary: {}), root {}",
                info.version,
                crate::VERSION,
                info.root
            ),
        ),
    }

    for slug in project::list_slugs(root) {
        let Ok(project) = project::Project::load(root, &slug) else {
            continue;
        };
        let label = format!("project {slug}");
        if let Some(warning) = crate::thread::memory_use(&project).warning() {
            check(&mut out, None, &format!("{label} memory"), warning);
        }
        if let Ok(text) = std::fs::read_to_string(project.project_md())
            && let Ok(front) = project::project_md_front(&text)
        {
            let legacy = project::legacy_agent_keys(front);
            if !legacy.is_empty() {
                check(
                    &mut out,
                    Some(false),
                    &label,
                    format!(
                        "PROJECT.md still has {}; D2 removed these keys",
                        legacy.join(", ")
                    ),
                );
                continue;
            }
        }
        let Some(record) = project.coordinator() else {
            check(
                &mut out,
                Some(true),
                &label,
                format!("{}; never opened", project.status()),
            );
            continue;
        };
        if !Path::new(&record.socket).exists() {
            check(
                &mut out,
                None,
                &label,
                format!(
                    "recorded socket {} no longer exists; `open --rebind` moves it",
                    record.socket
                ),
            );
            continue;
        }
        let herdr = Herdr::new(&bin, &record.socket, runner);
        match herdr.pane_list() {
            Err(error) => check(
                &mut out,
                None,
                &label,
                format!("session at {} unreachable: {error}", record.socket),
            ),
            Ok(panes) => {
                let workspace = panes.iter().any(|p| p.workspace_id == record.workspace_id);
                let pane = panes
                    .iter()
                    .any(|p| crate::coordinator::pane_matches(&record, p));
                check(
                    &mut out,
                    if pane { Some(true) } else { None },
                    &label,
                    format!(
                        "{}; socket {}; workspace {} {}; coordinator pane {} {}",
                        project.status(),
                        record.socket,
                        record.workspace_id,
                        if workspace { "exists" } else { "is gone" },
                        record.pane_id,
                        if pane {
                            "exists"
                        } else {
                            "is gone (run `open`)"
                        },
                    ),
                );
            }
        }
    }

    let ctx = Ctx {
        env,
        root: root.to_path_buf(),
        config_dir: config_dir.to_path_buf(),
        runner,
        detached_ticker: false,
    };
    match crate::launch::doctor_rows(&ctx) {
        Ok(rows) => {
            for row in rows {
                check(&mut out, row.ok, &row.label, row.detail.clone());
            }
        }
        Err(error) => check(&mut out, Some(false), "recipes", format!("{error:#}")),
    }

    // Machines that projects use need an SSH target for report and library copies.
    let mut machines = std::collections::BTreeSet::new();
    for slug in project::list_slugs(root) {
        let Ok(project) = project::Project::load(root, &slug) else {
            continue;
        };
        if let Ok((settings, _)) = project.read_project_md() {
            machines.extend(settings.repos.into_iter().filter_map(|r| r.machine));
        }
        machines.extend(
            crate::thread::list(&project)
                .into_iter()
                .filter(|t| t.is_remote() && t.status != crate::thread::Status::Resolved)
                .map(|t| t.machine),
        );
    }
    for machine in machines {
        match crate::remote::machine_profile(runner, &bin, config_dir, &machine) {
            Ok(profile) if profile.is_local() => check(
                &mut out,
                Some(true),
                &format!("machine {machine}"),
                "on this Mac".into(),
            ),
            Ok(profile) => {
                check(
                    &mut out,
                    Some(true),
                    &format!("machine {machine}"),
                    format!("ssh target {}", profile.target),
                );
                for (ok, label, detail) in box_rows(runner, &bin, &profile) {
                    check(&mut out, ok, &label, detail);
                }
            }
            Err(error) => check(
                &mut out,
                Some(false),
                &format!("machine {machine}"),
                format!("{error:#}"),
            ),
        }
    }

    (out, healthy)
}

/// One saved machine's box rows (SPEC-remote §§2–3, R11): boot service,
/// server, host, listeners, repository mapping, Git identity and GitHub
/// reach, per-kind logins, and live CPU/RAM/disk capacity with the 12 GB
/// gate. One read-only SSH call.
fn box_rows(
    runner: &dyn Runner,
    herdr_bin: &str,
    profile: &crate::contracts::MachineProfile,
) -> Vec<(Option<bool>, String, String)> {
    let label = &profile.label;
    if profile.target.is_empty() {
        return vec![(
            Some(false),
            format!("box {label}"),
            "has no SSH target".into(),
        )];
    }
    let repos: Vec<&crate::contracts::BoxRepoMap> = crate::contracts::BOX_REPOS.iter().collect();
    let mut script = String::from(
        "set -u\n\
         printf 'host\\t%s\\n' \"$(hostname 2>/dev/null || true)\"\n\
         printf 'boot\\t%s\\n' \"$(systemctl --user is-enabled herdr.service 2>/dev/null || echo unknown)\"\n\
         printf 'server\\t%s\\n' \"$(\"$HOME/.local/bin/herdr\" --version 2>/dev/null | head -n1 || echo missing)\"\n\
         printf 'tailscale\\t%s\\n' \"$(tailscale ip -4 2>/dev/null | head -n1 || true)\"\n\
         printf 'nproc\\t%s\\n' \"$(nproc 2>/dev/null || echo 0)\"\n\
         printf 'mem_avail_kb\\t%s\\n' \"$(awk '/MemAvailable/{print $2}' /proc/meminfo 2>/dev/null || echo 0)\"\n\
         printf 'df_free\\t%s\\n' \"$(df -B1 --output=avail / 2>/dev/null | tail -n1 | tr -d ' ')\"\n\
         printf 'listeners\\t%s\\n' \"$(ss -tln 2>/dev/null | tail -n +2 | wc -l | tr -d ' ')\"\n\
         printf 'git_name\\t%s\\n' \"$(git config --global user.name 2>/dev/null || true)\"\n\
         printf 'git_email\\t%s\\n' \"$(git config --global user.email 2>/dev/null || true)\"\n\
         printf 'gh\\t%s\\n' \"$(gh auth status >/dev/null 2>&1 && echo ok || echo missing)\"\n\
         printf 'rules\\t%s\\n' \"$(sha256sum \"$HOME/.config/herdr-ade/RULES.md\" 2>/dev/null | cut -d' ' -f1 || true)\"\n\
         \"$HOME/.local/bin/claude\" auth status >/dev/null 2>&1 && printf 'login_claude\\tok\\n' || printf 'login_claude\\tmissing\\n'\n\
         \"$HOME/.local/bin/codex\" login status >/dev/null 2>&1 && printf 'login_codex\\tok\\n' || printf 'login_codex\\tmissing\\n'\n\
         \"$HOME/.local/bin/agy\" models >/dev/null 2>&1 && printf 'login_agy\\tok\\n' || printf 'login_agy\\tmissing\\n'\n",
    );
    // Pi readiness is read on the box through its own wrapper and login store
    // (SPEC-remote §3.3, SPEC-pi §3.4, item 101): never Mac auth.
    for provider in crate::pi::roles::enabled_providers() {
        let provider = crate::remote::quote(provider);
        script.push_str(&format!(
            "HERDR_ADE_ROOT=\"$HOME/.herdr-ade\" \"$HOME/.local/bin/herdr-pi\" check {provider} >/dev/null 2>&1 && printf 'pi_%s\\tok\\n' {provider} || printf 'pi_%s\\tfail\\n' {provider}\n"
        ));
    }
    for repo in repos {
        let path = crate::remote::quote(repo.box_path);
        script.push_str(&format!(
            "if [ -d {path}/.git ]; then printf 'repo %s\\tok\\n' {path}; else printf 'repo %s\\tmissing\\n' {path}; fi\n"
        ));
    }
    let facts = match crate::remote::ssh(
        runner,
        &profile.target,
        &script,
        None,
        crate::remote::SSH_START_TIMEOUT,
    ) {
        Ok(out) if out.success() => parse_facts(&out.stdout),
        Ok(out) => {
            return vec![(
                None,
                format!("box {label}"),
                format!("unreachable: {}", out.error_text()),
            )];
        }
        Err(error) => {
            return vec![(
                None,
                format!("box {label}"),
                format!("unreachable: {error:#}"),
            )];
        }
    };
    let fact = |key: &str| facts.get(key).cloned().unwrap_or_default();
    let mut rows = Vec::new();
    rows.push((
        env_bool(&fact("boot"), &["enabled"]),
        format!("box {label} boot"),
        format!("systemd user unit herdr.service: {}", fact("boot")),
    ));
    rows.push((
        if fact("server").starts_with("herdr") {
            Some(true)
        } else {
            None
        },
        format!("box {label} server"),
        format!("{} ({})", fact("server"), "$HOME/.local/bin/herdr"),
    ));
    let tailscale = fact("tailscale");
    rows.push((
        if tailscale.is_empty() {
            None
        } else {
            Some(true)
        },
        format!("box {label} host"),
        format!("{} (Tailscale {tailscale})", fact("host")),
    ));
    rows.push((
        Some(true),
        format!("box {label} listeners"),
        format!(
            "{} non-loopback TCP listeners (check `ss -tln`)",
            fact("listeners")
        ),
    ));
    for repo in crate::contracts::BOX_REPOS.iter() {
        let key = format!("repo {}", repo.box_path);
        let value = fact(&key);
        rows.push((
            env_bool(&value, &["ok"]),
            format!("box {label} repo {}", repo.box_path),
            format!("clone {value}"),
        ));
    }
    let git = format!("{} <{}>", fact("git_name"), fact("git_email"));
    rows.push((
        if fact("git_name").is_empty() || fact("git_email").is_empty() {
            Some(false)
        } else {
            Some(true)
        },
        format!("box {label} git"),
        git,
    ));
    rows.push((
        env_bool(&fact("gh"), &["ok"]),
        format!("box {label} gh"),
        format!("gh auth status: {}", fact("gh")),
    ));
    for kind in ["claude", "codex", "agy"] {
        let value = fact(&format!("login_{kind}"));
        rows.push((
            env_bool(&value, &["ok"]),
            format!("box {label} login {kind}"),
            format!("{kind} authentication: {value}"),
        ));
    }
    // The box pane probe (SPEC-remote §3.3): a fresh pane with the lane PATH
    // answers `type -a -P pi` and `command -v` in its own shell.
    match box_pane_probe(runner, herdr_bin, profile) {
        Ok(answer) => {
            let (pi, tools) = parse_box_probe(&answer);
            let wrapper = "/home/ubuntu/.local/bin/pi";
            rows.push((
                if pi == wrapper {
                    Some(true)
                } else {
                    Some(false)
                },
                format!("box {label} wrapper"),
                if pi.is_empty() {
                    "the box pane did not answer `type -a -P pi`".into()
                } else {
                    format!("`type -a -P pi` first hit: {pi}")
                },
            ));
            let missing: Vec<&str> = ["cargo", "just", "claude", "codex", "agy", "node"]
                .into_iter()
                .filter(|tool| {
                    !tools
                        .iter()
                        .any(|found| found.ends_with(&format!("/{tool}")))
                })
                .collect();
            rows.push((
                if missing.is_empty() {
                    Some(true)
                } else {
                    Some(false)
                },
                format!("box {label} tools"),
                if missing.is_empty() {
                    tools.join(" ")
                } else {
                    format!("the pane cannot find: {}", missing.join(" "))
                },
            ));
        }
        Err(error) => rows.push((
            Some(false),
            format!("box {label} wrapper"),
            format!("the box pane probe failed: {error:#}"),
        )),
    }
    for provider in crate::pi::roles::enabled_providers() {
        let value = fact(&format!("pi_{provider}"));
        rows.push((
            env_bool(&value, &["ok"]),
            format!("box {label} pi {provider}"),
            format!("herdr-pi check {provider}: {value}"),
        ));
    }
    let nproc: u64 = fact("nproc").parse().unwrap_or(0);
    let mem_gb = fact("mem_avail_kb")
        .parse::<u64>()
        .map(|kb| kb as f64 / 1_000_000.0)
        .unwrap_or(0.0);
    let disk_gb = fact("df_free")
        .parse::<u64>()
        .map(|bytes| bytes as f64 / 1_000_000_000.0)
        .unwrap_or(0.0);
    let cpu_fit = nproc.saturating_sub(1);
    let mem_fit = (mem_gb / 4.0) as u64;
    let disk_fit = (disk_gb / 5.0) as u64;
    let fits = cpu_fit.min(mem_fit).min(disk_fit);
    let capacity_ok = disk_gb >= 12.0;
    rows.push((
        if capacity_ok { Some(true) } else { Some(false) },
        format!("box {label} capacity"),
        format!(
            "{nproc} OCPU, {mem_gb:.1} GB RAM free, {disk_gb:.1} GB disk free; about {fits} more lane(s) fit; refuses below 12 GB free"
        ),
    ));
    let rules = fact("rules");
    rows.push((
        if rules.is_empty() { None } else { Some(true) },
        format!("box {label} rules"),
        if rules.is_empty() {
            "no generated RULES.md recorded".into()
        } else {
            format!("RULES.md sha256 {rules}")
        },
    ));
    rows
}

fn parse_facts(text: &str) -> std::collections::BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(key, value)| (key.to_string(), value.trim().to_string()))
        .collect()
}

/// The probe a fresh box pane runs in its own Bash shell (SPEC-remote §3.3).
const BOX_PROBE: &str = "printf '@@pi '; type -a -P pi 2>/dev/null | head -n1; \
                         printf '@@cmd\\n'; command -v cargo just claude codex agy node 2>/dev/null; \
                         printf '@@done\\n'";

/// Creates a fresh box pane with the lane PATH, runs [`BOX_PROBE`] in that
/// pane's own shell, reads the answer and closes the workspace. This is the
/// §3.3 probe: never a bare `ssh` command string, never `bash -lic`.
fn box_pane_probe(
    runner: &dyn Runner,
    herdr_bin: &str,
    profile: &crate::contracts::MachineProfile,
) -> Result<String> {
    let herdr = Herdr::new(herdr_bin, "", runner).on_machine(&profile.id);
    let env = vec![format!("PATH={}", crate::contracts::BOX_PATH)];
    let created = herdr
        .workspace_create_env(
            Path::new(crate::contracts::BOX_HOME),
            "ha-doctor-probe",
            false,
            &env,
        )
        .map_err(|error| anyhow::anyhow!("box probe pane: {error}"))?;
    let pane = created.pane_id.clone();
    let answer = (|| -> Result<String> {
        herdr
            .pane_run(&pane, BOX_PROBE)
            .map_err(|error| anyhow::anyhow!("box probe run: {error}"))?;
        for _ in 0..50 {
            let text = herdr
                .pane_read_text(&pane, "recent")
                .map_err(|error| anyhow::anyhow!("box probe read: {error}"))?;
            if text.contains("@@done") {
                return Ok(text);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        anyhow::bail!("the box pane did not answer the probe in time")
    })();
    let _ = herdr.workspace_close(&created.workspace_id);
    answer
}

/// The `@@pi` first hit and the `command -v` paths from a probe answer.
fn parse_box_probe(text: &str) -> (String, Vec<String>) {
    let mut pi = String::new();
    let mut tools = Vec::new();
    let mut in_tools = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("@@pi ") {
            pi = rest.trim().to_string();
        } else if line == "@@cmd" {
            in_tools = true;
        } else if line == "@@done" {
            in_tools = false;
        } else if in_tools && !line.is_empty() {
            tools.push(line.to_string());
        }
    }
    (pi, tools)
}

fn env_bool(value: &str, ok: &[&str]) -> Option<bool> {
    if ok.contains(&value) {
        Some(true)
    } else {
        Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, fail, ok};

    fn runner_with_herdr(version: &str) -> FakeRunner {
        let runner = FakeRunner::new();
        runner.on("herdr --version", ok(version));
        runner.on("session list --json", ok(r#"{"sessions":[]}"#));
        runner.on("git --version", ok("git version 2.50.0\n"));
        runner.on("ssh -V", ok(""));
        runner.on("rsync --version", ok("rsync 3\n"));
        runner.on("gh --version", ok("gh version 2\n"));
        runner.on("gh auth status", fail(1, "not logged in"));
        runner
    }

    #[test]
    fn old_herdr_fails_and_names_the_minimum() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = runner_with_herdr("herdr 0.9.0\n");
        let (text, healthy) = report(
            &env,
            &home.path().join("root"),
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(!healthy);
        assert!(text.contains("[FAIL] herdr: 0.9.0"), "{text}");
        assert!(text.contains("0.9.1 or later"));
    }

    #[test]
    fn new_herdr_passes_and_warnings_do_not_fail() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = runner_with_herdr("herdr 0.9.1\n");
        let root = home.path().join("root");
        let (text, healthy) = report(
            &env,
            &root,
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(healthy, "{text}");
        assert!(text.contains("[warn] gh auth"));
        assert!(text.contains("[warn] root"));
        assert!(text.contains(&format!("root:       {}", root.display())));
        assert!(!root.exists(), "doctor must not create the root");
        assert!(text.contains("plugin:     herdr-ade"), "{text}");
        assert!(text.contains("crate:      herdr-ade"), "{text}");
        assert!(text.contains("prefix:"), "{text}");
    }

    #[test]
    fn a_project_over_its_memory_budget_warns_and_does_not_fail() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        std::fs::create_dir_all(project.dir().join("memory")).unwrap();
        std::fs::write(project.dir().join("MEMORY.md"), "# Memory\n- state\n").unwrap();
        std::fs::write(
            project.dir().join("memory/state.md"),
            "x".repeat(crate::thread::MEMORY_CAP_CHARS + 1),
        )
        .unwrap();
        let runner = runner_with_herdr("herdr 0.9.1\n");
        let (text, healthy) = report(
            &env,
            &root,
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(healthy, "{text}");
        assert!(text.contains("[warn] project demo memory"), "{text}");
        assert!(text.contains("memory/state.md"), "{text}");
        assert!(text.contains("memory/archive/"), "{text}");
    }

    fn box_facts() -> String {
        let mut lines: Vec<String> = [
            "host\tremote-host",
            "boot\tenabled",
            "server\therdr 0.9.1",
            "tailscale\t100.64.0.1",
            "nproc\t16",
            "mem_avail_kb\t40000000",
            "df_free\t100000000000",
            "listeners\t2",
            "git_name\tuguryildirim24",
            "git_email\trolf@example.com",
            "gh\tok",
            "rules\tabc",
            "login_claude\tok",
            "login_codex\tok",
            "login_agy\tok",
            "repo /home/ubuntu/projects/herdr\tok",
            "repo /home/ubuntu/projects/herdr-ade\tok",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        for provider in crate::pi::roles::enabled_providers() {
            lines.push(format!("pi_{provider}\tok"));
        }
        lines.join("\n") + "\n"
    }

    fn probe_fakes(runner: &FakeRunner) {
        runner.on(
            "workspace create",
            ok(r#"{"result":{"root_pane":{"workspace_id":"w9","tab_id":"w9:t1","pane_id":"w9:p1","cwd":"/home/ubuntu"}}}"#),
        );
        runner.on("pane run", ok(r#"{"result":{}}"#));
        runner.on(
            "pane read",
            ok("@@pi /home/ubuntu/.local/bin/pi\n@@cmd\n/home/ubuntu/.cargo/bin/cargo\n/home/ubuntu/.cargo/bin/just\n/home/ubuntu/.local/bin/claude\n/home/ubuntu/.local/bin/codex\n/home/ubuntu/.local/bin/agy\n/usr/local/bin/node\n@@done\n"),
        );
        runner.on("workspace close", ok(r#"{"result":{}}"#));
    }

    fn box_profile() -> crate::contracts::MachineProfile {
        crate::contracts::MachineProfile {
            id: "abc".into(),
            label: "oci".into(),
            target: "me@box".into(),
            session: "default".into(),
        }
    }

    #[test]
    fn box_rows_read_the_box_and_gate_on_free_disk() {
        let runner = FakeRunner::new();
        runner.on("ssh", ok(&box_facts()));
        probe_fakes(&runner);
        let rows = box_rows(&runner, "herdr", &box_profile());
        let find = |label: &str| {
            rows.iter()
                .find(|(_, name, _)| name == label)
                .map(|(ok, _, detail)| (*ok, detail.clone()))
                .unwrap_or_else(|| panic!("no row {label}"))
        };
        assert_eq!(find("box oci boot").0, Some(true));
        assert_eq!(find("box oci wrapper").0, Some(true));
        assert!(
            find("box oci wrapper")
                .1
                .contains("/home/ubuntu/.local/bin/pi")
        );
        assert_eq!(find("box oci tools").0, Some(true));
        assert_eq!(
            find("box oci repo /home/ubuntu/projects/herdr").0,
            Some(true)
        );
        assert_eq!(find("box oci capacity").0, Some(true));
        assert!(find("box oci capacity").1.contains("refuses below 12 GB"));
        let calls = runner.calls.borrow();
        let ssh = calls
            .iter()
            .find(|call| call.program == "ssh")
            .unwrap()
            .display();
        let script = calls
            .iter()
            .find(|call| call.program == "ssh")
            .and_then(|call| call.args.last())
            .unwrap();
        assert!(
            script.starts_with(&format!("sh -c 'PATH={}", crate::contracts::BOX_PATH)),
            "{script}"
        );
        assert!(
            ssh.contains("\"$HOME/.local/bin/claude\" auth status"),
            "{ssh}"
        );
        assert!(
            ssh.contains("\"$HOME/.local/bin/codex\" login status"),
            "{ssh}"
        );
        assert!(ssh.contains("\"$HOME/.local/bin/agy\" models"), "{ssh}");
        drop(calls);

        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&box_facts().replace("df_free\t100000000000", "df_free\t5000000000")),
        );
        probe_fakes(&runner);
        assert_eq!(find_row(&runner, "box oci capacity").0, Some(false));

        let runner = FakeRunner::new();
        runner.on("ssh", fail(255, "ssh: connect timed out"));
        let rows = box_rows(&runner, "herdr", &box_profile());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, None);
        assert!(rows[0].2.contains("unreachable"));
    }

    #[test]
    fn box_wrapper_probe_fails_closed_when_the_pane_answers_another_path() {
        let runner = FakeRunner::new();
        runner.on("ssh", ok(&box_facts()));
        runner.on(
            "workspace create",
            ok(r#"{"result":{"root_pane":{"workspace_id":"w9","tab_id":"w9:t1","pane_id":"w9:p1","cwd":"/home/ubuntu"}}}"#),
        );
        runner.on("pane run", ok(r#"{"result":{}}"#));
        runner.on("pane read", ok("@@pi /usr/local/bin/pi\n@@cmd\n@@done\n"));
        runner.on("workspace close", ok(r#"{"result":{}}"#));
        let row = find_row(&runner, "box oci wrapper");
        assert_eq!(row.0, Some(false));
        assert!(row.2.contains("/usr/local/bin/pi"), "{}", row.2);
    }

    fn find_row(runner: &FakeRunner, label: &str) -> (Option<bool>, String, String) {
        box_rows(runner, "herdr", &box_profile())
            .into_iter()
            .find(|(_, name, _)| name == label)
            .unwrap_or_else(|| panic!("no row {label}"))
    }

    #[test]
    fn fork_0_9_0_with_parent_is_accepted() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = runner_with_herdr("herdr 0.9.0\n");
        runner.on(
            "agent start --help",
            ok("usage: herdr agent start <name> --kind KIND --pane ID [--parent PANE_ID]\n"),
        );
        let (text, healthy) = report(
            &env,
            &home.path().join("root"),
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(healthy, "{text}");
        assert!(text.contains("install day"), "{text}");
        assert!(text.contains("[ok  ] parent:"), "{text}");
    }
}
