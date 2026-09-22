//! `doctor`: what is installed, where things resolve, and whether it fits.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::herdr::{self, Herdr};
use crate::paths::{self, Ctx, Env, SessionFlags};
use crate::project;
use crate::runner::{Cmd, Runner};

const TOOL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct CheckResult {
    pub(crate) status: String,
    pub(crate) label: String,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct DoctorOutcome {
    pub(crate) healthy: bool,
    pub(crate) checks: Vec<CheckResult>,
    #[serde(skip)]
    pub(crate) message: String,
}

/// One agent runtime's existing doctor probe. Recipes identify the runtime by
/// `kind`; placement never carries a separate allowlist of recipe ids.
#[derive(Debug, Clone)]
struct NativeProbe {
    kind: String,
    cache_key: String,
    program: String,
    args: Vec<String>,
}

/// Build the real readiness call from the adapter declaration and the exact
/// routed recipe. No provider or model is selected in doctor code.
fn native_probe(
    adapter: &crate::adapters::Adapter,
    recipe: &crate::contracts::Recipe,
) -> Option<NativeProbe> {
    if adapter.doctor.args.is_empty() {
        return None;
    }
    let mut args = Vec::new();
    for value in &adapter.doctor.args {
        if value == "{args}" {
            args.extend(crate::adapters::launch_args(adapter, recipe));
        } else {
            args.push(value.clone());
        }
    }
    let digest = crate::thread::sha256_hex(args.join("\0").as_bytes());
    Some(NativeProbe {
        kind: recipe.kind.clone(),
        cache_key: format!("{}-{}", recipe.kind, &digest[..12]),
        program: adapter.binary.clone(),
        args,
    })
}

fn probe_error(kind: &str, output: &crate::runner::Output) -> String {
    let detail = output.error_text();
    if output.timed_out {
        return format!("{kind} readiness probe timed out; provider status is unknown");
    }
    if detail.is_empty() {
        return format!(
            "{kind} readiness probe failed with exit {:?}; provider status is unknown",
            output.code
        );
    }
    if crate::pi::doctor::positive_sign_in_evidence(&detail) {
        format!(
            "{kind} provider rejected its smallest model; its stored sign-in may no longer work: {detail}"
        )
    } else {
        format!(
            "{kind} readiness probe failed locally or returned an unrecognized response: {detail}"
        )
    }
}

fn run_native_probe(
    ctx: &Ctx,
    probe: &NativeProbe,
    timeout: Duration,
) -> Result<crate::runner::Output> {
    let cache_dir = ctx.root.join(".readiness");
    let cache = cache_dir.join(format!("native-{}.json", probe.cache_key));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if let Ok(bytes) = std::fs::read(&cache)
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let (Some(checked), Some(ok)) = (
            value
                .get("checked_unix")
                .and_then(serde_json::Value::as_u64),
            value.get("ok").and_then(serde_json::Value::as_bool),
        )
        && now.saturating_sub(checked) <= crate::pi::doctor::READINESS_CACHE_TTL.as_secs()
        && (ok
            || value
                .get("provider_failure")
                .and_then(serde_json::Value::as_bool)
                .is_some())
    {
        let provider_failure = value
            .get("provider_failure")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        return Ok(crate::runner::Output {
            code: Some(if ok { 0 } else { 1 }),
            stderr: if ok {
                String::new()
            } else if provider_failure {
                "authentication failed (cached provider refusal)".into()
            } else {
                value
                    .get("detail")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("readiness probe failed locally (cached)")
                    .to_string()
            },
            ..Default::default()
        });
    }
    let output = ctx.runner.run(
        &Cmd::new(&probe.program, timeout)
            .args(probe.args.iter().map(String::as_str))
            .cwd(&ctx.root),
    )?;
    let provider_failure = !output.timed_out
        && !output.success()
        && crate::pi::doctor::positive_sign_in_evidence(&output.error_text());
    // Unknown diagnostics may be local, or may be provider text we do not
    // recognize. Do not persist either. A later check reruns and keeps the
    // original text in its immediate result.
    if (output.success() || provider_failure)
        && ctx.root.is_dir()
        && std::fs::create_dir_all(&cache_dir).is_ok()
    {
        let value = serde_json::json!({
            "checked_unix": now,
            "ok": output.success(),
            "provider_failure": provider_failure,
        });
        let staged = cache_dir.join(format!(
            ".native-{}-{}",
            probe.cache_key,
            std::process::id()
        ));
        if std::fs::write(&staged, value.to_string()).is_ok() {
            let _ = std::fs::rename(&staged, &cache);
        }
        let _ = std::fs::remove_file(staged);
    }
    Ok(output)
}

fn native_probe_command(probe: &NativeProbe) -> String {
    std::iter::once(probe.program.as_str())
        .chain(probe.args.iter().map(String::as_str))
        .map(crate::remote::quote)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A box-side real model call cached for one ticker interval. The cache holds
/// only `ok`, `failed` or `missing`; provider output never lands on disk.
fn box_native_probe_script(
    probe: &NativeProbe,
    fact: bool,
    machine: &crate::remote::MachineDeclaration,
) -> String {
    let command = native_probe_command(probe);
    let finish = if fact {
        format!("printf 'login_{}\\t%s\\n' \"$probe_status\"", probe.kind)
    } else {
        "[ \"$probe_status\" = ok ]".into()
    };
    format!(
        "PATH={path}; export PATH\n\
         probe_dir={root}/.readiness\n\
         probe_cache=\"$probe_dir/native-{cache_key}\"\n\
         probe_status=\n\
         probe_now=$(date +%s)\n\
         probe_then=$(stat -c %Y \"$probe_cache\" 2>/dev/null || echo 0)\n\
         if [ $((probe_now-probe_then)) -le {ttl} ]; then probe_status=$(cat \"$probe_cache\" 2>/dev/null || true); fi\n\
         if [ -z \"$probe_status\" ]; then\n\
           if ! command -v {program} >/dev/null 2>&1; then probe_status=missing;\n\
           elif {command} >/dev/null 2>&1; then probe_status=ok;\n\
           else probe_status=failed; fi\n\
           mkdir -p \"$probe_dir\"\n\
           printf '%s\\n' \"$probe_status\" > \"$probe_cache.tmp.$$\"\n\
           mv -f \"$probe_cache.tmp.$$\" \"$probe_cache\"\n\
         fi\n\
         {finish}\n",
        path = crate::remote::quote(&machine.path),
        root = crate::remote::quote(&machine.root),
        cache_key = probe.cache_key,
        ttl = crate::pi::doctor::READINESS_CACHE_TTL.as_secs(),
        program = crate::remote::quote(&probe.program),
    )
}

/// Whether the chosen recipe can run on this Mac. This is the same provider
/// or login probe the doctor owns, not a placement-specific capability table.
pub(crate) fn recipe_ready_local(ctx: &Ctx, launch: &crate::contracts::Launch) -> Result<()> {
    let adapter = crate::adapters::declaration(&ctx.config_dir, &launch.kind)?;
    if adapter.doctor.readiness == "pi" {
        let provider = crate::pi::launch::flag_value(&launch.args, "--provider")
            .context("pi_args_forbidden: a provider launch names no --provider")?;
        let model = crate::pi::launch::flag_value(&launch.args, "--model")
            .context("pi_args_forbidden: a provider launch names no --model")?;
        return crate::pi_ade::check_with(ctx.runner, &ctx.root, &provider, &model)
            .map(|_| ())
            .with_context(|| format!("pi_not_ready: provider {provider}"));
    }
    let recipe = crate::contracts::Recipe {
        kind: launch.kind.clone(),
        args: launch.args.clone(),
        ..Default::default()
    };
    let probe = native_probe(&adapter, &recipe).with_context(|| {
        format!(
            "no doctor readiness probe exists for agent kind `{}`",
            launch.kind
        )
    })?;
    let output = run_native_probe(
        ctx,
        &probe,
        Duration::from_millis(launch.ready_timeout_ms.max(1_000)),
    )
    .with_context(|| format!("{} is not installed", probe.program))?;
    if !output.success() {
        anyhow::bail!(probe_error(&probe.kind, &output));
    }
    Ok(())
}

/// Whether the chosen recipe can run on a saved box. SSH injects the exact
/// lane PATH, so `command -v` and the login command measure the environment a
/// fresh lane receives. Pi keeps using `herdr-pi check`, as the doctor does.
pub(crate) fn recipe_ready_on_box(
    ctx: &Ctx,
    profile: &crate::contracts::MachineProfile,
    launch: &crate::contracts::Launch,
) -> Result<()> {
    let adapter = crate::adapters::declaration(&ctx.config_dir, &launch.kind)?;
    if adapter.doctor.readiness == "pi" {
        let provider = crate::pi::launch::flag_value(&launch.args, "--provider")
            .context("pi_args_forbidden: a provider launch names no --provider")?;
        let model = crate::pi::launch::flag_value(&launch.args, "--model")
            .context("pi_args_forbidden: a provider launch names no --model")?;
        let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
        return crate::pi_ade::check_on_machine(
            ctx.runner,
            &ctx.root,
            &profile.target,
            &machine,
            &provider,
            &model,
        )
        .with_context(|| format!("provider {provider}"));
    }
    let recipe = crate::contracts::Recipe {
        kind: launch.kind.clone(),
        args: launch.args.clone(),
        ..Default::default()
    };
    let probe = native_probe(&adapter, &recipe).with_context(|| {
        format!(
            "no doctor readiness probe exists for agent kind `{}`",
            launch.kind
        )
    })?;
    let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    let script = box_native_probe_script(&probe, false, &machine);
    let rooted = crate::runner::CwdRunner::new(ctx.runner, &ctx.root);
    let output = crate::remote::ssh(
        &rooted,
        &profile.target,
        &script,
        None,
        Duration::from_millis(launch.ready_timeout_ms.max(1_000)),
    )?;
    if !output.success() {
        anyhow::bail!(probe_error(&probe.kind, &output));
    }
    Ok(())
}

/// Builds the human report and its typed check results from the same facts.
pub(crate) fn run(ctx: &Ctx, session: &SessionFlags) -> Result<DoctorOutcome> {
    let (mut text, mut healthy, mut checks) =
        report_with_checks(ctx.env, &ctx.root, &ctx.config_dir, session, ctx.runner);
    // The pi rows read only providers named by enabled configured recipes;
    // unused built-in provider knowledge never causes a doctor failure.
    let pi_models: Vec<(String, String)> = crate::launch::parse_launch_config(&ctx.config_dir)
        .map(|config| {
            let routed = config.routing.recipe_ids();
            config
                .recipes
                .iter()
                .filter(|(id, recipe)| {
                    routed.contains(id.as_str())
                        && recipe.enabled
                        && config
                            .adapters
                            .get(&recipe.kind)
                            .is_some_and(|adapter| adapter.doctor.readiness == "pi")
                })
                .filter_map(|(_, recipe)| {
                    crate::pi::launch::flag_value(&recipe.args, "--model")
                        .map(|model| (recipe.provider.clone(), model))
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default();
    match crate::pi_ade::doctor_rows_with(ctx.runner, &ctx.root, &pi_models) {
        Ok((rows, pi_healthy)) => {
            healthy &= pi_healthy;
            for row in rows {
                let status = match row.level {
                    crate::pi::doctor::Level::Ok => "ok",
                    crate::pi::doctor::Level::Warn => "warning",
                    crate::pi::doctor::Level::Fail => "failed",
                };
                checks.push(CheckResult {
                    status: status.into(),
                    label: row.label.clone(),
                    detail: row.detail.clone(),
                });
                let _ = writeln!(text, "{}", row.line());
            }
        }
        Err(error) => {
            healthy = false;
            let detail = format!("{error:#}");
            checks.push(CheckResult {
                status: "failed".into(),
                label: "pi".into(),
                detail: detail.clone(),
            });
            let _ = writeln!(text, "[FAIL] pi: {detail}");
        }
    }
    Ok(DoctorOutcome {
        healthy,
        checks,
        message: text,
    })
}

#[cfg(test)]
fn report(
    env: &Env,
    root: &Path,
    config_dir: &Path,
    session: &SessionFlags,
    runner: &dyn Runner,
) -> (String, bool) {
    let (text, healthy, _) = report_with_checks(env, root, config_dir, session, runner);
    (text, healthy)
}

fn report_with_checks(
    env: &Env,
    root: &Path,
    config_dir: &Path,
    session: &SessionFlags,
    runner: &dyn Runner,
) -> (String, bool, Vec<CheckResult>) {
    let stable_runner = crate::runner::CwdRunner::new(runner, root);
    let runner: &dyn Runner = &stable_runner;
    let mut out = String::new();
    let mut healthy = true;
    let mut checks = Vec::new();
    let mut check = |out: &mut String, ok: Option<bool>, label: &str, detail: String| {
        let (mark, status) = match ok {
            Some(true) => ("ok  ", "ok"),
            Some(false) => {
                healthy = false;
                ("FAIL", "failed")
            }
            None => ("warn", "warning"),
        };
        checks.push(CheckResult {
            status: status.into(),
            label: label.into(),
            detail: detail.clone(),
        });
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
            if reachable {
                let herdr = Herdr::new(&bin, &found.socket, runner);
                check_workspace_leaks(&mut out, &mut check, root, "local", "this Mac", &herdr);
            }
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

    let doctor_config = match crate::launch::doctor_config(config_dir) {
        Ok(config) => config,
        Err(error) => {
            check(
                &mut out,
                Some(false),
                "doctor settings",
                format!("{error:#}"),
            );
            crate::launch::DoctorConfig::default()
        }
    };
    match local_free_disk_gb(runner) {
        Ok(free) => check(
            &mut out,
            Some(free >= doctor_config.min_free_disk_gb),
            "machine local disk",
            disk_detail(free, doctor_config.min_free_disk_gb),
        ),
        Err(error) => check(
            &mut out,
            None,
            "machine local disk",
            format!("free space unknown: {error:#}"),
        ),
    }

    if root.is_dir() {
        let count = project::list_slugs(root).len();
        check(&mut out, Some(true), "root", format!("{count} project(s)"));
        let ctx = Ctx {
            env,
            root: root.to_path_buf(),
            config_dir: config_dir.to_path_buf(),
            runner,
            detached_ticker: false,
        };
        let (leftovers, data_kept, errors) = finished_worktrees(&ctx, None);
        check(
            &mut out,
            worktree_check_status(&leftovers, &errors),
            "finished worktrees local",
            worktree_check_detail(&leftovers, &errors),
        );
        if !data_kept.is_empty() {
            check(
                &mut out,
                None,
                "worktree data kept local",
                data_kept.join("; "),
            );
        }
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
        crate::ticker::LockState::Held(info) => {
            let (status, detail) = ticker_folder_check(&info);
            check(&mut out, status, "ticker", detail);
        }
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
            let removed = project::removed_project_keys(front);
            if !removed.is_empty() {
                check(
                    &mut out,
                    Some(false),
                    &label,
                    format!(
                        "PROJECT.md has removed settings: {}; delete these lines",
                        removed.join(", ")
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
                // The name must resolve to the bound pane, or nothing can wake
                // this coordinator. An `agent list` error is a warn, not a
                // false failure: the pane is still the honest reading.
                let named = match herdr.agent_list() {
                    Ok(agents) => Some(
                        agents
                            .iter()
                            .any(|a| crate::coordinator::agent_matches(&record, a)),
                    ),
                    Err(_) => None,
                };
                let unread = crate::steps::announced_unread(&project);
                let mut detail = format!(
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
                );
                if pane {
                    detail.push_str(&format!(
                        "; agent `{}` {}",
                        record.agent_name,
                        match named {
                            Some(true) => "resolves".to_string(),
                            Some(false) =>
                                "does not resolve (the ticker restores it; `open` otherwise)"
                                    .to_string(),
                            None => "could not be read".to_string(),
                        },
                    ));
                }
                if record.name_restored > 0 {
                    detail.push_str(&format!("; name restored {}x", record.name_restored));
                }
                if let Some(passes) = unread {
                    detail.push_str(&format!(
                        "; announced inbox items unread for {passes} ticker passes"
                    ));
                }
                let ok = if unread.is_some() {
                    Some(false)
                } else if !pane {
                    None
                } else {
                    named
                };
                check(&mut out, ok, &label, detail);
            }
        }
    }

    let worker = config_dir.join(crate::harness::BOX_WORKER_MARKER).is_file();
    if worker {
        check(
            &mut out,
            Some(true),
            "lane worker",
            "RULES.md is local; routing recipes and saved-machine checks stay on the coordinator"
                .into(),
        );
    } else {
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

        // Check every machine placement can choose, not only machines with a live
        // thread. This includes configured defaults, repository rows and every
        // enabled saved profile (an explicit `--machine` can choose any of them).
        match machines_to_check(root, config_dir, runner, &bin)
            .and_then(|machines| Ok((machines, crate::launch::parse_launch_config(config_dir)?)))
        {
            Ok((machines, config)) => {
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
                            for (ok, label, detail) in box_rows(
                                runner,
                                &bin,
                                config_dir,
                                &profile,
                                &config.recipes,
                                config.doctor.min_free_disk_gb,
                            ) {
                                check(&mut out, ok, &label, detail);
                            }
                            let herdr = Herdr::new(&bin, "", runner).on_machine(&profile.id);
                            check_workspace_leaks(
                                &mut out,
                                &mut check,
                                root,
                                &profile.id,
                                &format!("machine {}", profile.label),
                                &herdr,
                            );
                            let ctx = Ctx {
                                env,
                                root: root.to_path_buf(),
                                config_dir: config_dir.to_path_buf(),
                                runner,
                                detached_ticker: false,
                            };
                            let (mut leftovers, data_kept, mut errors) =
                                finished_worktrees(&ctx, Some(&profile));
                            let (builds, build_errors) = finished_build_folders(&ctx, &profile);
                            leftovers.extend(builds);
                            errors.extend(build_errors);
                            check(
                                &mut out,
                                worktree_check_status(&leftovers, &errors),
                                &format!("finished worktrees {}", profile.label),
                                worktree_check_detail(&leftovers, &errors),
                            );
                            if !data_kept.is_empty() {
                                check(
                                    &mut out,
                                    None,
                                    &format!("worktree data kept {}", profile.label),
                                    data_kept.join("; "),
                                );
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
            }
            Err(error) => check(&mut out, Some(false), "machines", format!("{error:#}")),
        }
    }

    (out, healthy, checks)
}

fn ticker_folder_check(info: &crate::ticker::Info) -> (Option<bool>, String) {
    let cwd = Path::new(&info.cwd);
    let cwd_ok = !info.cwd.is_empty() && cwd.is_dir();
    let version_ok = crate::build::same_commit(&info.version, crate::VERSION);
    (
        if cwd_ok && version_ok {
            Some(true)
        } else {
            Some(false)
        },
        format!(
            "running, version {} (this binary: {}), root {}, folder {}{}{}",
            info.version,
            crate::VERSION,
            info.root,
            if info.cwd.is_empty() {
                "not recorded"
            } else {
                &info.cwd
            },
            if cwd_ok {
                ""
            } else {
                "; folder no longer exists"
            },
            if version_ok {
                ""
            } else {
                "; ticker build is stale"
            }
        ),
    )
}

fn local_free_disk_gb(runner: &dyn Runner) -> Result<f64> {
    let output = runner.run(&Cmd::new("df", TOOL_TIMEOUT).args(["-Pk", "/"]))?;
    if !output.success() {
        anyhow::bail!("{}", output.error_text());
    }
    let available_kb = output
        .stdout
        .lines()
        .rev()
        .find_map(|line| line.split_whitespace().nth(3)?.parse::<u64>().ok())
        .context("`df -Pk /` did not report available blocks")?;
    Ok(available_kb as f64 * 1024.0 / 1_000_000_000.0)
}

fn display_gb(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn disk_detail(free_gb: f64, minimum_gb: f64) -> String {
    format!(
        "{free_gb:.1} GB free; refuses below {} GB free",
        display_gb(minimum_gb)
    )
}

fn worktree_check_status(leftovers: &[String], errors: &[String]) -> Option<bool> {
    if !leftovers.is_empty() {
        Some(false)
    } else if !errors.is_empty() {
        None
    } else {
        Some(true)
    }
}

fn worktree_check_detail(leftovers: &[String], errors: &[String]) -> String {
    match (leftovers.is_empty(), errors.is_empty()) {
        (true, true) => "none whose work is done".into(),
        (false, true) => format!(
            "remove these finished worktrees or build folders: {}",
            leftovers.join(", ")
        ),
        (true, false) => format!("unknown; could not check: {}", errors.join("; ")),
        (false, false) => format!(
            "remove these finished worktrees or build folders: {}; unknown for: {}",
            leftovers.join(", "),
            errors.join("; ")
        ),
    }
}

/// Finished thread worktrees that still exist on one machine. Completion is
/// derived from the same records as `thread resolve`; existence is checked on
/// the machine that owns the checkout.
fn thread_is_on_machine(
    thread: &crate::thread::Thread,
    profile: &crate::contracts::MachineProfile,
) -> bool {
    thread.is_remote()
        && if thread.machine_id.is_empty() {
            thread.machine == profile.label
        } else {
            thread.machine_id == profile.id
        }
}

fn finished_worktrees(
    ctx: &Ctx,
    remote: Option<&crate::contracts::MachineProfile>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut candidates = Vec::new();
    let mut errors = Vec::new();
    for slug in project::list_slugs(&ctx.root) {
        let Ok(project) = project::Project::load(&ctx.root, &slug) else {
            continue;
        };
        let (threads, unreadable) = crate::thread::list_with_errors(&project);
        errors.extend(
            unreadable
                .into_iter()
                .map(|error| format!("{slug}: thread state unknown: {error:#}")),
        );
        for thread in threads {
            if thread.status != crate::thread::Status::Resolved
                || thread.worktree_path.is_empty()
                || (thread.kind != crate::thread::Kind::Worktree
                    && !crate::threads::managed_git_folder(&project, &thread))
            {
                continue;
            }
            let on_machine = match remote {
                None => !thread.is_remote(),
                Some(profile) => thread_is_on_machine(&thread, profile),
            };
            if !on_machine {
                continue;
            }
            match crate::threads::finished_worktree_reason(ctx, &project, &thread) {
                Ok(None) => candidates.push((project.clone(), thread)),
                Ok(Some(_)) => {}
                Err(error) => errors.push(format!("{}: {error:#}", thread.id)),
            }
        }
    }
    // Existence on a box is one read-only fact call for every candidate. The
    // shell always exits zero after printing each yes/no answer, so a healthy
    // "gone" result cannot enter the command-failure ledger and a transport
    // failure remains distinguishable from a negative answer.
    let remote_exists = remote.map(|profile| {
        let script = candidates
            .iter()
            .enumerate()
            .map(|(index, (_, thread))| {
                format!(
                    "if [ -d {} ]; then printf '{}\\t1\\n'; else printf '{}\\t0\\n'; fi",
                    crate::remote::quote(&thread.worktree_path),
                    index,
                    index
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        if candidates.is_empty() {
            return Vec::new();
        }
        match crate::remote::ssh(ctx.runner, &profile.target, &script, None, TOOL_TIMEOUT) {
            Ok(output) if output.success() => {
                let facts: BTreeMap<usize, bool> = output
                    .stdout
                    .lines()
                    .filter_map(|line| {
                        let (index, exists) = line.split_once('\t')?;
                        let exists = match exists {
                            "0" => false,
                            "1" => true,
                            _ => return None,
                        };
                        Some((index.parse().ok()?, exists))
                    })
                    .collect();
                let mut answers = Vec::with_capacity(candidates.len());
                for (index, (_, thread)) in candidates.iter().enumerate() {
                    match facts.get(&index) {
                        Some(exists) => answers.push(*exists),
                        None => {
                            errors.push(format!(
                                "{}: box worktree existence answer was missing",
                                thread.worktree_path
                            ));
                            answers.push(false);
                        }
                    }
                }
                answers
            }
            Ok(output) => {
                errors.push(format!("box worktree check: {}", output.error_text()));
                vec![false; candidates.len()]
            }
            Err(error) => {
                errors.push(format!("box worktree check: {error:#}"));
                vec![false; candidates.len()]
            }
        }
    });

    let mut leftovers = Vec::new();
    let mut data_kept = Vec::new();
    for (index, (project, thread)) in candidates.into_iter().enumerate() {
        let path = &thread.worktree_path;
        let exists = remote_exists
            .as_ref()
            .map_or_else(|| Path::new(path).is_dir(), |answers| answers[index]);
        if !exists {
            continue;
        }
        match crate::threads::inspect_worktree_for_removal(ctx, &project, &thread) {
            Ok(inspection) if !inspection.dirty.is_empty() => leftovers.push(path.clone()),
            Ok(inspection) if !inspection.ignored_data.is_empty() => data_kept.push(format!(
                "{} ({})",
                path,
                crate::worktrees::describe_data(&inspection.ignored_data)
            )),
            Ok(_) => leftovers.push(path.clone()),
            Err(error) => errors.push(format!("{}: {error:#}", thread.id)),
        }
    }
    (leftovers, data_kept, errors)
}

/// Rebuildable box output whose owning thread is no longer open. The folder
/// names come from the same helper that sets `CARGO_TARGET_DIR` at launch.
fn finished_build_folders(
    ctx: &Ctx,
    profile: &crate::contracts::MachineProfile,
) -> (Vec<String>, Vec<String>) {
    let mut active = BTreeSet::new();
    let mut uncertain_projects = BTreeSet::new();
    let mut errors = Vec::new();
    let machine_paths = match crate::remote::machine_declaration(&ctx.config_dir, &profile.label) {
        Ok(machine) => machine,
        Err(error) => return (Vec::new(), vec![format!("build root unknown: {error:#}")]),
    };
    for slug in project::list_slugs(&ctx.root) {
        let project = match project::Project::load(&ctx.root, &slug) {
            Ok(project) => project,
            Err(error) => {
                uncertain_projects.insert(slug.clone());
                errors.push(format!("{slug}: build ownership unknown: {error:#}"));
                continue;
            }
        };
        let (threads, unreadable) = crate::thread::list_with_errors(&project);
        if !unreadable.is_empty() {
            // A record that cannot be read may describe an open remote thread.
            // Do not call any folder for this project orphaned until its owner
            // can be established.
            uncertain_projects.insert(slug.clone());
        }
        errors.extend(
            unreadable
                .into_iter()
                .map(|error| format!("{slug}: build ownership unknown: {error:#}")),
        );
        for thread in threads {
            if thread.status != crate::thread::Status::Resolved
                && thread_is_on_machine(&thread, profile)
            {
                active.insert(format!("{slug}-{}", thread.id));
            }
        }
    }

    let root = machine_paths.build;
    let script = format!(
        "printf '__HERDR_BUILDS__\\n'; if test -d {root}; then find {root} -mindepth 1 -maxdepth 1 -type d -exec du -sk -- {{}} +; fi; printf '__HERDR_BUILDS_DONE__\\n'",
        root = crate::remote::quote(&root),
    );
    let output = match crate::remote::ssh(ctx.runner, &profile.target, &script, None, TOOL_TIMEOUT)
    {
        Ok(output) if output.success() => output,
        Ok(output) => {
            errors.push(output.error_text());
            return (Vec::new(), errors);
        }
        Err(error) => {
            errors.push(format!("{error:#}"));
            return (Vec::new(), errors);
        }
    };

    let mut in_list = false;
    let mut complete = false;
    let mut leftovers = Vec::new();
    for line in output.stdout.lines() {
        match line.trim() {
            "__HERDR_BUILDS__" => {
                in_list = true;
                continue;
            }
            "__HERDR_BUILDS_DONE__" => {
                complete = true;
                break;
            }
            _ if !in_list => continue,
            _ => {}
        }
        let Some((kb, path)) = line.split_once(char::is_whitespace) else {
            errors.push(format!("build folder size unreadable: {line}"));
            continue;
        };
        let path = path.trim();
        let Some(name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
            errors.push(format!("build folder path unreadable: {path}"));
            continue;
        };
        let Ok(kb) = kb.parse::<u64>() else {
            errors.push(format!("build folder size unreadable: {line}"));
            continue;
        };
        let ownership_unknown = uncertain_projects.iter().any(|slug| {
            name.strip_prefix(slug)
                .and_then(|suffix| suffix.strip_prefix('-'))
                .is_some_and(|id| crate::thread::validate_id(id).is_ok())
        });
        if !active.contains(name) && !ownership_unknown {
            leftovers.push(format!(
                "{} ({})",
                path,
                crate::worktrees::human_size(kb.saturating_mul(1024))
            ));
        }
    }
    if !complete {
        errors.push(format!("could not list build folders under {root}"));
    }
    (leftovers, errors)
}

fn check_workspace_leaks(
    out: &mut String,
    check: &mut impl FnMut(&mut String, Option<bool>, &str, String),
    root: &Path,
    machine: &str,
    display: &str,
    herdr: &Herdr<'_>,
) {
    let workspaces = match herdr.workspace_list() {
        Ok(workspaces) => workspaces,
        Err(error) => {
            check(
                out,
                Some(false),
                &format!("{display} workspaces"),
                format!("could not list workspaces: {error}"),
            );
            return;
        }
    };
    let agents = match herdr.agent_list() {
        Ok(agents) => agents,
        Err(error) => {
            check(
                out,
                Some(false),
                &format!("{display} workspaces"),
                format!("could not list agents: {error}"),
            );
            return;
        }
    };
    let open_threads: Vec<_> = project::list_slugs(root)
        .into_iter()
        .filter_map(|slug| project::Project::load(root, &slug).ok())
        .flat_map(|project| crate::thread::list(&project))
        .filter(|thread| {
            matches!(
                thread.status,
                crate::thread::Status::Starting | crate::thread::Status::Open
            ) && if machine == "local" {
                !thread.is_remote()
            } else {
                thread.is_remote() && thread.machine_route() == machine
            }
        })
        .collect();
    let open: BTreeSet<String> = open_threads
        .iter()
        .map(|thread| thread.workspace_id.clone())
        .filter(|workspace| !workspace.is_empty())
        .collect();
    let default_shell = default_shell_workspaces(&workspaces, &agents);
    let leaked: Vec<_> = workspaces
        .iter()
        .filter(|workspace| !default_shell.contains(&workspace.workspace_id))
        .filter(|workspace| {
            !open.contains(&workspace.workspace_id)
                && !agents
                    .iter()
                    .any(|agent| agent.workspace_id == workspace.workspace_id)
        })
        .collect();
    let ids = leaked
        .iter()
        .map(|workspace| workspace.workspace_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    check(
        out,
        Some(leaked.is_empty()),
        &format!("{display} workspaces"),
        if leaked.is_empty() {
            format!(
                "{} total; no unowned agentless workspaces",
                workspaces.len()
            )
        } else {
            format!(
                "{} of {} hold no agent and belong to no open lane: {ids}",
                leaked.len(),
                workspaces.len()
            )
        },
    );

    if machine == "local" {
        return;
    }
    let tabs = match herdr.tab_list() {
        Ok(tabs) => tabs,
        Err(error) => {
            check(
                out,
                Some(false),
                &format!("{display} project tabs"),
                format!("could not list tabs: {error}"),
            );
            return;
        }
    };
    let labels: BTreeSet<String> = project::list_slugs(root)
        .into_iter()
        .filter_map(|slug| project::Project::load(root, &slug).ok())
        .filter_map(|project| {
            project
                .read_project_md()
                .ok()
                .map(|(settings, _)| project::display_name(&settings.name, &project.slug))
        })
        .collect();
    let mut duplicate_labels = Vec::new();
    let mut project_workspaces = BTreeSet::new();
    for label in labels {
        let matching: Vec<_> = workspaces
            .iter()
            .filter(|workspace| workspace.label == label)
            .collect();
        if matching.len() > 1 {
            duplicate_labels.push(format!("{label} ({})", matching.len()));
        }
        project_workspaces.extend(
            matching
                .into_iter()
                .filter(|workspace| !default_shell.contains(&workspace.workspace_id))
                .map(|workspace| workspace.workspace_id.clone()),
        );
    }
    let open_tabs: BTreeSet<_> = open_threads
        .iter()
        .map(|thread| thread.tab_id.clone())
        .filter(|tab| !tab.is_empty())
        .collect();
    let orphan_tabs: Vec<_> = tabs
        .iter()
        .filter(|tab| project_workspaces.contains(&tab.workspace_id))
        .filter(|tab| {
            !open_tabs.contains(&tab.tab_id)
                && !agents.iter().any(|agent| agent.tab_id == tab.tab_id)
        })
        .map(|tab| tab.tab_id.as_str())
        .collect();
    let healthy = duplicate_labels.is_empty() && orphan_tabs.is_empty();
    let detail = if healthy {
        format!(
            "{} project workspaces; every tab has an agent or an open lane",
            project_workspaces.len()
        )
    } else {
        format!(
            "duplicate labels: {}; shell tabs with no open lane: {}",
            if duplicate_labels.is_empty() {
                "none".to_string()
            } else {
                duplicate_labels.join(", ")
            },
            if orphan_tabs.is_empty() {
                "none".to_string()
            } else {
                orphan_tabs.join(", ")
            }
        )
    };
    check(
        out,
        Some(healthy),
        &format!("{display} project tabs"),
        detail,
    );
}

/// The machine's own home shell: the workspace the client hides while the
/// machine has another workspace (fork `is_default_shell_space`): label `~`, no
/// agent, one tab holding one pane. `herdr workspace list` does not expose the
/// fork's `custom_label` bit, so a workspace hand-labelled exactly `~` is
/// indistinguishable here and is also skipped.
fn default_shell_workspaces(
    workspaces: &[herdr::Workspace],
    agents: &[herdr::Agent],
) -> BTreeSet<String> {
    if workspaces.len() <= 1 {
        return BTreeSet::new();
    }
    workspaces
        .iter()
        .filter(|workspace| {
            workspace.label == "~"
                && workspace.tab_count == 1
                && workspace.pane_count == 1
                && !agents
                    .iter()
                    .any(|agent| agent.workspace_id == workspace.workspace_id)
        })
        .map(|workspace| workspace.workspace_id.clone())
        .collect()
}

fn machines_to_check(
    root: &Path,
    config_dir: &Path,
    runner: &dyn Runner,
    herdr_bin: &str,
) -> Result<std::collections::BTreeSet<String>> {
    let dispatch = crate::launch::parse_launch_config(config_dir)?
        .dispatch
        .machine;
    let mut machines = std::collections::BTreeSet::new();
    if !dispatch.is_empty() {
        machines.insert(dispatch.clone());
    }
    machines.extend(crate::remote::registered_machine_names(
        runner, herdr_bin, config_dir,
    )?);

    let add_repos = |machines: &mut std::collections::BTreeSet<String>,
                     repos: Vec<crate::project::Repo>| {
        for repo in repos {
            if let Some(machine) = repo.machine.filter(|machine| !machine.is_empty()) {
                machines.insert(machine);
            }
            // A box_path is what makes the dispatch default eligible for this
            // repository. Keep that relationship explicit even though the
            // dispatch row itself is also checked when no project is open.
            if repo.box_path.is_some() && !dispatch.is_empty() {
                machines.insert(dispatch.clone());
            }
        }
    };
    add_repos(&mut machines, crate::harness::repos(config_dir)?);
    for slug in project::list_slugs(root) {
        let Ok(project) = project::Project::load(root, &slug) else {
            continue;
        };
        if let Ok((settings, _)) = project.read_project_md() {
            add_repos(&mut machines, settings.repos);
        }
        machines.extend(
            crate::thread::list(&project)
                .into_iter()
                .filter(|thread| thread.is_remote() && !thread.worktree_path.is_empty())
                .map(|thread| thread.machine_route().to_string()),
        );
    }
    Ok(machines)
}

/// One saved machine's box rows (SPEC-remote §§2–3, R11): boot service,
/// server, host, listeners, repository mapping, Git identity and GitHub
/// reach, enabled recipes' readiness, and live CPU/RAM/disk capacity with the
/// configured free-space gate. One read-only SSH call.
fn box_rows(
    runner: &dyn Runner,
    herdr_bin: &str,
    config_dir: &Path,
    profile: &crate::contracts::MachineProfile,
    recipes: &BTreeMap<String, crate::contracts::Recipe>,
    min_free_disk_gb: f64,
) -> Vec<(Option<bool>, String, String)> {
    let label = &profile.label;
    if profile.target.is_empty() {
        return vec![(
            Some(false),
            format!("box {label}"),
            "has no SSH target".into(),
        )];
    }
    let machine_paths = match crate::remote::machine_declaration(config_dir, label) {
        Ok(machine) => machine,
        Err(error) => {
            return vec![(
                Some(false),
                format!("box {label}"),
                format!("machine path declaration is missing: {error:#}"),
            )];
        }
    };
    // The executable recipes own which checks exist. Native probe definitions
    // only describe how to check a runtime; they never select one to require.
    let mut natives = BTreeMap::new();
    let mut providers = BTreeSet::new();
    let mut rows = Vec::new();
    let launch_config = crate::launch::parse_launch_config(config_dir).ok();
    let routed = launch_config
        .as_ref()
        .map(|config| config.routing.recipe_ids());
    let adapters = crate::adapters::declarations(config_dir).unwrap_or_default();
    for (id, recipe) in recipes.iter().filter(|(id, recipe)| {
        recipe.enabled
            && routed.as_ref().is_none_or(|ids| ids.contains(id.as_str()))
            && machine_paths.runs_kind(&recipe.kind)
    }) {
        let Some(adapter) = adapters.get(&recipe.kind) else {
            rows.push((
                Some(false),
                format!("box {label} recipe {id}"),
                format!("no adapter exists for agent kind `{}`", recipe.kind),
            ));
            continue;
        };
        if adapter.doctor.readiness == "pi" {
            match crate::pi::launch::validate_provider_column(&recipe.provider, &recipe.args) {
                Ok(()) => {
                    let model =
                        crate::pi::launch::flag_value(&recipe.args, "--model").unwrap_or_default();
                    providers.insert((recipe.provider.as_str(), model));
                }
                Err(error) => rows.push((
                    Some(false),
                    format!("box {label} recipe {id}"),
                    format!("{error:#}"),
                )),
            }
        } else if let Some(probe) = native_probe(adapter, recipe) {
            natives.insert(id.clone(), probe);
        } else {
            rows.push((
                Some(false),
                format!("box {label} recipe {id}"),
                format!(
                    "no doctor readiness probe exists for agent kind `{}`",
                    recipe.kind
                ),
            ));
        }
    }
    let mut repos: Vec<crate::project::Repo> =
        crate::harness::repos(config_dir).unwrap_or_default();
    repos.extend(machine_paths.repos.clone());
    let mut script = format!(
        "set -u\n\
         PATH={path}; export PATH\n\
         printf 'host\\t%s\\n' \"$(hostname 2>/dev/null || true)\"\n\
         printf 'boot\\t%s\\n' \"$(systemctl --user is-enabled herdr.service 2>/dev/null || echo unknown)\"\n\
         herdr_bin=$(command -v herdr 2>/dev/null || true)\n\
         printf 'server\\t%s\\n' \"$(\"$herdr_bin\" --version 2>/dev/null | head -n1 || echo missing)\"\n\
         printf 'tailscale\\t%s\\n' \"$(tailscale ip -4 2>/dev/null | head -n1 || true)\"\n\
         printf 'nproc\\t%s\\n' \"$(nproc 2>/dev/null || echo 0)\"\n\
         printf 'mem_avail_kb\\t%s\\n' \"$(awk '/MemAvailable/{{print $2}}' /proc/meminfo 2>/dev/null || echo 0)\"\n\
         printf 'df_free\\t%s\\n' \"$(df -B1 --output=avail / 2>/dev/null | tail -n1 | tr -d ' ')\"\n\
         printf 'listeners\\t%s\\n' \"$(ss -tln 2>/dev/null | tail -n +2 | wc -l | tr -d ' ')\"\n\
         printf 'git_name\\t%s\\n' \"$(git config --global user.name 2>/dev/null || true)\"\n\
         printf 'git_email\\t%s\\n' \"$(git config --global user.email 2>/dev/null || true)\"\n\
         printf 'gh\\t%s\\n' \"$(gh auth status >/dev/null 2>&1 && echo ok || echo missing)\"\n\
         printf 'rules\\t%s\\n' \"$(sha256sum {home}/.config/herdr-ade/RULES.md 2>/dev/null | cut -d' ' -f1 || true)\"\n\
",
        path = crate::remote::quote(&machine_paths.path),
        home = crate::remote::quote(&machine_paths.home),
    );
    for probe in natives.values() {
        script.push_str(&box_native_probe_script(probe, true, &machine_paths));
    }
    // Pi readiness is read on the box through its own wrapper and login store
    // (SPEC-remote §3.3, SPEC-pi §3.4, item 101): never Mac auth.
    for (provider, model) in &providers {
        let provider = crate::remote::quote(provider);
        let model = crate::remote::quote(model);
        script.push_str(&format!(
            "HERDR_ADE_ROOT={root} {pi_bin} check {provider} --model {model} >/dev/null 2>&1 && printf 'pi_%s/%s\\tok\\n' {provider} {model} || printf 'pi_%s/%s\\tfail\\n' {provider} {model}\n",
            root = crate::remote::quote(&machine_paths.root),
            pi_bin = crate::remote::quote(&machine_paths.pi_bin),
        ));
    }
    for repo in &repos {
        let Some(box_path) = &repo.box_path else {
            continue;
        };
        let path = crate::remote::quote(box_path);
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
                Some(false),
                format!("box {label}"),
                format!("unreachable: {}", out.error_text()),
            )];
        }
        Err(error) => {
            return vec![(
                Some(false),
                format!("box {label}"),
                format!("unreachable: {error:#}"),
            )];
        }
    };
    let fact = |key: &str| facts.get(key).cloned().unwrap_or_default();
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
        format!("{} (from machine PATH)", fact("server")),
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
    for repo in &repos {
        let Some(box_path) = &repo.box_path else {
            continue;
        };
        let key = format!("repo {box_path}");
        let value = fact(&key);
        rows.push((
            env_bool(&value, &["ok"]),
            format!("box {label} repo {box_path}"),
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
    for probe in natives.values() {
        let kind = &probe.kind;
        let value = fact(&format!("login_{kind}"));
        rows.push((
            env_bool(&value, &["ok"]),
            format!("box {label} login {kind}"),
            if value == "ok" {
                format!("{kind} reached its smallest model")
            } else {
                format!(
                    "{kind} readiness probe did not succeed ({value}); provider status is unknown"
                )
            },
        ));
    }
    // The box pane probe (SPEC-remote §3.3): a fresh pane with the lane PATH
    // answers `type -a -P pi` and `command -v` in its own shell.
    let required_tools: Vec<&str> = ["cargo", "just", "node"]
        .into_iter()
        .chain(natives.values().map(|probe| probe.program.as_str()))
        .collect();
    match box_pane_probe(
        runner,
        herdr_bin,
        profile,
        Some(&machine_paths),
        &required_tools,
    ) {
        Ok(answer) => {
            let (pi, tools) = parse_box_probe(&answer);
            let wrapper = Path::new(&machine_paths.pi_bin)
                .parent()
                .map(|dir| dir.join("pi").to_string_lossy().into_owned())
                .unwrap_or_default();
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
            // Only native recipes require their standalone executables.
            let missing: Vec<&str> = required_tools
                .iter()
                .copied()
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
    for (provider, model) in providers {
        let value = fact(&format!("pi_{provider}/{model}"));
        rows.push((
            env_bool(&value, &["ok"]),
            format!("box {label} pi {provider}/{model}"),
            format!("herdr-pi check {provider} --model {model}: {value}"),
        ));
    }
    let nproc = fact("nproc").parse::<u64>().ok();
    let mem_gb = fact("mem_avail_kb")
        .parse::<u64>()
        .ok()
        .map(|kb| kb as f64 / 1_000_000.0);
    let disk_gb = fact("df_free")
        .parse::<u64>()
        .ok()
        .map(|bytes| bytes as f64 / 1_000_000_000.0);
    let fits = nproc.zip(mem_gb).zip(disk_gb).map(|((cpu, memory), disk)| {
        cpu.saturating_sub(1)
            .min((memory / 4.0) as u64)
            .min((disk / 5.0) as u64)
    });
    rows.push((
        disk_gb.map(|disk| disk >= min_free_disk_gb),
        format!("box {label} capacity"),
        format!(
            "{} OCPU, {} GB RAM free, {} GB disk free; {}; refuses below {} GB free",
            nproc.map_or_else(|| "unknown".into(), |value| value.to_string()),
            mem_gb.map_or_else(|| "unknown".into(), |value| format!("{value:.1}")),
            disk_gb.map_or_else(|| "unknown".into(), |value| format!("{value:.1}")),
            fits.map_or_else(
                || "additional lane capacity unknown".into(),
                |value| format!("about {value} more lane(s) fit")
            ),
            display_gb(min_free_disk_gb)
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

/// Creates a fresh box pane with the lane PATH, runs the tool probe in that
/// pane's own shell, reads the answer and closes the workspace. This is the
/// §3.3 probe: never a bare `ssh` command string, never `bash -lic`.
fn box_pane_probe(
    runner: &dyn Runner,
    herdr_bin: &str,
    profile: &crate::contracts::MachineProfile,
    machine: Option<&crate::remote::MachineDeclaration>,
    tools: &[&str],
) -> Result<String> {
    let tools = tools
        .iter()
        .map(|tool| crate::remote::quote(tool))
        .collect::<Vec<_>>()
        .join(" ");
    let probe = format!(
        "printf '@@pi '; type -a -P pi 2>/dev/null | head -n1; \
         printf '@@cmd\\n'; command -v {tools} 2>/dev/null; \
         printf '@@done\\n'"
    );
    let herdr = Herdr::new(herdr_bin, "", runner).on_machine(&profile.id);
    let machine = machine.context("machine path declaration is missing")?;
    let env = vec![format!("PATH={}", machine.path)];
    let created = herdr
        .workspace_create_env(Path::new(&machine.home), "ha-doctor-probe", false, &env)
        .map_err(|error| anyhow::anyhow!("box probe pane: {error}"))?;
    let pane = created.pane_id.clone();
    let answer = (|| -> Result<String> {
        herdr
            .pane_run(&pane, &probe)
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

    const ROUTING_CONFIG: &str = r#"[routing]
default = "pi_codex_sol_high"
retries = 1
fallback = []

[[routing.rules]]
workflow = "coordinator"
recipe = "claude_coordinator_opus"

[[routing.rules]]
product = "spec"
recipe = "claude_fable_xhigh"

[[routing.rules]]
product = "web-research"
recipe = "agy_gemini_flash"

[[routing.rules]]
capability = "native-chat"
recipe = "claude_fable_xhigh"
"#;

    fn write_routing_config(config: &Path) {
        std::fs::create_dir_all(config).unwrap();
        std::fs::write(config.join("config.toml"), ROUTING_CONFIG).unwrap();
    }

    fn machine_config(kinds: &[&str]) -> tempfile::TempDir {
        let config = tempfile::tempdir().unwrap();
        let kinds = kinds
            .iter()
            .map(|kind| format!("\"{kind}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            config.path().join("config.toml"),
            format!(
                "[machines.oci]\nlabel = \"oci\"\ntarget = \"oci-pi\"\nsession = \"default\"\nhome = \"/home/ubuntu\"\nroot = \"/home/ubuntu/.herdr-ade\"\nworktrees = \"/home/ubuntu/projects\"\nbuild = \"/home/ubuntu/build/lanes\"\npath = \"/home/ubuntu/.local/bin:/home/ubuntu/.cargo/bin:/usr/local/bin:/usr/bin:/bin\"\nade_bin = \"/home/ubuntu/.local/bin/herdr-ade\"\npi_bin = \"/home/ubuntu/.local/bin/herdr-pi\"\nkinds = [{kinds}]\n"
            ),
        )
        .unwrap();
        config
    }

    fn runner_with_machine_list(version: &str, machines: &str) -> FakeRunner {
        let runner = FakeRunner::new();
        runner.on("herdr --version", ok(version));
        runner.on("session list --json", ok(r#"{"sessions":[]}"#));
        runner.on("git --version", ok("git version 2.50.0\n"));
        runner.on("df -Pk /", ok("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk 200000000 1000000 199000000 1% /\n"));
        runner.on("ssh -V", ok(""));
        runner.on("rsync --version", ok("rsync 3\n"));
        runner.on("gh --version", ok("gh version 2\n"));
        runner.on_fn(
            |cmd| cmd.program == "gh" && cmd.args == ["auth", "status"],
            |_| Ok(fail(1, "not logged in")),
        );
        runner.on("machine list --json", ok(machines));
        runner.on("workspace list", ok(r#"{"result":{"workspaces":[]}}"#));
        runner.on("tab list", ok(r#"{"result":{"tabs":[]}}"#));
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        runner
    }

    fn runner_with_herdr(version: &str) -> FakeRunner {
        runner_with_machine_list(version, "[]")
    }

    /// Like `runner_with_herdr`, but the local session answers `pane list` and
    /// `agent list` with the given JSON, so a project row can be read.
    fn runner_with_project(version: &str, panes: &str, agents: &str) -> FakeRunner {
        let runner = FakeRunner::new();
        runner.on("herdr --version", ok(version));
        runner.on("session list --json", ok(r#"{"sessions":[]}"#));
        runner.on("git --version", ok("git version 2.50.0\n"));
        runner.on("df -Pk /", ok("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk 200000000 1000000 199000000 1% /\n"));
        runner.on("ssh -V", ok(""));
        runner.on("rsync --version", ok("rsync 3\n"));
        runner.on("gh --version", ok("gh version 2\n"));
        runner.on_fn(
            |cmd| cmd.program == "gh" && cmd.args == ["auth", "status"],
            |_| Ok(fail(1, "not logged in")),
        );
        runner.on("machine list --json", ok("[]"));
        runner.on("workspace list", ok(r#"{"result":{"workspaces":[]}}"#));
        runner.on("pane list", ok(panes));
        runner.on("agent list", ok(agents));
        runner
    }

    /// A project with a coordinator record bound to `pane` and a real socket
    /// file, so `report` reads it as an opened project.
    fn opened_project(home: &Path, root: &Path, pane: &str, agent_name: &str) -> project::Project {
        let project = project::create(root, "demo", "", vec![]).unwrap();
        let socket = home.join("herdr.sock");
        std::fs::write(&socket, b"").unwrap();
        let cwd = project.canonical_dir().to_string_lossy().into_owned();
        project
            .update_coordinator(|c| {
                c.socket = socket.to_string_lossy().into_owned();
                c.workspace_id = "w1".into();
                c.tab_id = "w1:t1".into();
                c.pane_id = pane.into();
                c.agent_name = agent_name.into();
                c.cwd = cwd;
            })
            .unwrap();
        project
    }

    #[test]
    fn a_cached_native_failure_still_names_the_stored_sign_in_without_provider_output() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.program == "claude",
            |_| Ok(fail(1, "subscription expired")),
        );
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let launch = crate::contracts::Launch {
            kind: "claude".into(),
            ready_timeout_ms: 1_000,
            ..Default::default()
        };

        let first = recipe_ready_local(&ctx, &launch).unwrap_err().to_string();
        assert!(first.contains("subscription expired"), "{first}");
        let cache_path = std::fs::read_dir(ctx.root.join(".readiness"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let cache = std::fs::read_to_string(cache_path).unwrap();
        assert!(!cache.contains("subscription expired"), "{cache}");

        let cached = recipe_ready_local(&ctx, &launch).unwrap_err().to_string();
        assert!(
            cached.contains("stored sign-in may no longer work"),
            "{cached}"
        );
        assert!(!cached.contains("subscription expired"), "{cached}");
        assert_eq!(runner.count("claude"), 1);
        let calls = runner.calls.borrow();
        let probe = calls.iter().find(|call| call.program == "claude").unwrap();
        assert_eq!(probe.cwd.as_deref(), Some(ctx.root.as_path()));
    }

    #[test]
    fn a_timed_out_native_probe_is_unknown_and_is_not_cached() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.program == "claude",
            |_| {
                Ok(crate::runner::Output {
                    stderr: "authentication failed before the probe stalled".into(),
                    timed_out: true,
                    ..Default::default()
                })
            },
        );
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };
        let launch = crate::contracts::Launch {
            kind: "claude".into(),
            ready_timeout_ms: 1_000,
            ..Default::default()
        };

        for _ in 0..2 {
            let error = recipe_ready_local(&ctx, &launch).unwrap_err().to_string();
            assert!(error.contains("timed out"), "{error}");
            assert!(!error.contains("stored sign-in"), "{error}");
        }
        assert_eq!(runner.count("claude"), 2);
        assert!(!ctx.root.join(".readiness/native-claude.json").exists());
    }

    #[test]
    fn agy_probe_arguments_match_the_installed_cli() {
        let config = tempfile::tempdir().unwrap();
        let adapter = crate::adapters::declaration(config.path(), "agy").unwrap();
        let recipe = crate::contracts::Recipe {
            kind: "agy".into(),
            args: vec![
                "--model".into(),
                "gemini-3.8-flash-high".into(),
                "--dangerously-skip-permissions".into(),
            ],
            ..Default::default()
        };
        let probe = native_probe(&adapter, &recipe).unwrap();
        assert_eq!(
            probe.args,
            [
                "--model",
                "gemini-3.8-flash-high",
                "--dangerously-skip-permissions",
                "-p",
                "Reply only OK.",
                "--print-timeout",
                "60s",
            ]
        );
    }

    #[test]
    fn an_unknown_flag_diagnostic_is_a_local_fault() {
        let output = crate::runner::Output {
            code: Some(1),
            stderr: "flags provided but not defined: -max-turns".into(),
            ..Default::default()
        };
        let error = probe_error("agy", &output);
        assert!(error.contains("failed locally"), "{error}");
        assert!(!error.contains("stored sign-in"), "{error}");
    }

    #[test]
    fn agentless_workspaces_without_an_open_lane_fail_the_doctor_row() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        crate::thread::allocate(&project, |thread| {
            thread.status = crate::thread::Status::Open;
            thread.machine = "oci".into();
            thread.machine_id = "abc".into();
            thread.workspace_id = "w3".into();
        })
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "workspace list",
            ok(r#"{"result":{"workspaces":[{"workspace_id":"w1"},{"workspace_id":"w2"},{"workspace_id":"w3"}]}}"#),
        );
        runner.on(
            "agent list",
            ok(r#"{"result":{"agents":[{"workspace_id":"w2","tab_id":"w2:t1","pane_id":"w2:p1"}]}}"#),
        );
        runner.on("tab list", ok(r#"{"result":{"tabs":[]}}"#));
        let herdr = Herdr::new("herdr", "", &runner).on_machine("abc");
        let mut text = String::new();
        let mut healthy = true;
        let mut check = |out: &mut String, ok: Option<bool>, label: &str, detail: String| {
            healthy &= ok != Some(false);
            let _ = writeln!(out, "{label}: {detail}");
        };

        check_workspace_leaks(&mut text, &mut check, &root, "abc", "machine oci", &herdr);

        assert!(!healthy);
        assert!(
            text.contains("1 of 3 hold no agent and belong to no open lane: w1"),
            "{text}"
        );
    }

    #[test]
    fn a_machines_own_home_workspace_is_not_a_leak() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "workspace list",
            ok(r#"{"result":{"workspaces":[{"workspace_id":"w1","label":"~","tab_count":1,"pane_count":1},{"workspace_id":"w2"}]}}"#),
        );
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        runner.on("tab list", ok(r#"{"result":{"tabs":[]}}"#));
        let herdr = Herdr::new("herdr", "", &runner).on_machine("abc");
        let mut text = String::new();
        let mut healthy = true;
        let mut check = |out: &mut String, ok: Option<bool>, label: &str, detail: String| {
            healthy &= ok != Some(false);
            let _ = writeln!(out, "{label}: {detail}");
        };

        check_workspace_leaks(&mut text, &mut check, &root, "abc", "machine oci", &herdr);

        assert!(!healthy);
        assert!(
            text.contains("1 of 2 hold no agent and belong to no open lane: w2"),
            "{text}"
        );
    }

    #[test]
    fn duplicate_project_workspaces_and_unowned_shell_tabs_fail_doctor() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "workspace list",
            ok(r#"{"result":{"workspaces":[{"workspace_id":"w1","label":"Demo"},{"workspace_id":"w2","label":"Demo"}]}}"#),
        );
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        runner.on(
            "tab list",
            ok(r#"{"result":{"tabs":[{"workspace_id":"w1","tab_id":"w1:t1","label":"shell"}]}}"#),
        );
        let herdr = Herdr::new("herdr", "", &runner).on_machine("abc");
        let mut text = String::new();
        let mut healthy = true;
        let mut check = |out: &mut String, ok: Option<bool>, label: &str, detail: String| {
            healthy &= ok != Some(false);
            let _ = writeln!(out, "{label}: {detail}");
        };

        check_workspace_leaks(&mut text, &mut check, &root, "abc", "machine oci", &herdr);

        assert!(!healthy);
        assert!(text.contains("duplicate labels: Demo (2)"), "{text}");
        assert!(
            text.contains("shell tabs with no open lane: w1:t1"),
            "{text}"
        );
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
        write_routing_config(&home.path().join("cfg"));
        let runner = runner_with_herdr("herdr 0.9.1\n");
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
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
    fn doctor_refuses_the_removed_parallel_thread_setting() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let config = home.path().join("cfg");
        write_routing_config(&config);
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let text = std::fs::read_to_string(project.project_md()).unwrap();
        std::fs::write(
            project.project_md(),
            text.replacen("+++\n", "+++\nmax_parallel_threads = 9\n", 1),
        )
        .unwrap();
        let runner = runner_with_herdr("herdr 0.9.1\n");
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );

        let (text, healthy) = report(&env, &root, &config, &SessionFlags::default(), &runner);

        assert!(!healthy, "{text}");
        assert!(
            text.contains(
                "[FAIL] project demo: PROJECT.md has removed settings: max_parallel_threads; delete these lines"
            ),
            "{text}"
        );
    }

    #[test]
    fn doctor_without_a_routing_table_names_the_config_fix() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("config.toml"), "").unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = runner_with_herdr("herdr 0.9.1\n");
        let (text, healthy) = report(
            &env,
            &home.path().join("root"),
            &config,
            &SessionFlags::default(),
            &runner,
        );
        assert!(!healthy, "{text}");
        assert!(
            text.contains(
                "routing_default_missing: add [routing] with default = \"<recipe>\" to config.toml"
            ),
            "{text}"
        );
    }

    #[test]
    fn doctor_rejects_a_rule_with_an_unknown_recipe() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            "[routing]\ndefault = \"pi_codex_sol_high\"\n\n[[routing.rules]]\nworkflow = \"reviewer\"\nrecipe = \"missing\"\n",
        )
        .unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = runner_with_herdr("herdr 0.9.1\n");
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        let (text, healthy) = report(
            &env,
            &home.path().join("root"),
            &config,
            &SessionFlags::default(),
            &runner,
        );
        assert!(!healthy, "{text}");
        assert!(text.contains("routing_recipe_unknown: missing"), "{text}");
    }

    #[test]
    fn a_project_over_its_memory_budget_warns_and_does_not_fail() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        write_routing_config(&home.path().join("cfg"));
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        crate::talk::append(
            &project,
            None,
            crate::talk::Entry::Rolf {
                request: "q-1".into(),
                text: "Keep helper briefs focused.".into(),
                answer: None,
            },
        )
        .unwrap();
        let note = crate::note::add(
            &project,
            crate::note::Kind::Memory,
            &"x".repeat(crate::thread::MEMORY_CAP_CHARS + 1),
            "q-1",
            None,
            vec![],
        )
        .unwrap();
        let runner = runner_with_herdr("herdr 0.9.1\n");
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        let (text, healthy) = report(
            &env,
            &root,
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(healthy, "{text}");
        assert!(text.contains("[warn] project demo memory"), "{text}");
        assert!(text.contains(&note.id), "{text}");
        assert!(text.contains("replace stale dated notes"), "{text}");
    }

    #[test]
    fn doctor_accepts_a_ticker_with_the_same_commit_and_another_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let commit = crate::build::commit_version(crate::VERSION).unwrap();
        let info = crate::ticker::Info {
            version: format!("{commit}.9999999999"),
            pid: 42,
            root: dir.path().display().to_string(),
            cwd: dir.path().display().to_string(),
            started: project::now(),
            tools: Vec::new(),
        };

        let (status, detail) = ticker_folder_check(&info);

        assert_eq!(status, Some(true), "{detail}");
        assert!(!detail.contains("stale"), "{detail}");
    }

    #[test]
    fn doctor_flags_a_ticker_whose_recorded_folder_was_removed() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("disposable-worktree");
        std::fs::create_dir(&folder).unwrap();
        std::fs::remove_dir(&folder).unwrap();
        let info = crate::ticker::Info {
            version: crate::VERSION.into(),
            pid: 42,
            root: dir.path().display().to_string(),
            cwd: folder.display().to_string(),
            started: project::now(),
            tools: Vec::new(),
        };
        let (status, detail) = ticker_folder_check(&info);
        assert_eq!(status, Some(false));
        assert!(detail.contains("folder no longer exists"), "{detail}");
        assert!(
            detail.contains(folder.to_string_lossy().as_ref()),
            "{detail}"
        );
    }

    #[test]
    fn resolved_box_worktree_absence_is_one_check_and_no_ledger_failure() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let mut ids = Vec::new();
        for number in 1..=2 {
            let thread = crate::thread::allocate(&project, |thread| {
                thread.kind = crate::thread::Kind::Worktree;
                thread.status = crate::thread::Status::Resolved;
                thread.machine = "oci".into();
                thread.machine_id = "box-1".into();
                thread.worktree_path = format!("/box/worktree-{number}");
                thread.repo = "/repo".into();
                thread.branch = format!("lane-{number}");
            })
            .unwrap();
            ids.push(thread.id);
        }
        let rounds = project.state_dir().join("rounds");
        std::fs::create_dir_all(&rounds).unwrap();
        let record = crate::contracts::RoundRecord {
            phase: crate::contracts::RoundPhase::Abandoned,
            round: "r1".into(),
            branch: "main".into(),
            plain: "The work is closed.".into(),
            policy_hash: "policy".into(),
            manifest: crate::contracts::AdmissionManifest {
                revision: 1,
                members: ids
                    .into_iter()
                    .map(|thread| crate::contracts::ManifestMember { thread, pin: None })
                    .collect(),
            },
            repo: "/repo".into(),
            abandoned_reason: Some("not needed".into()),
            ..crate::contracts::RoundRecord::default()
        };
        std::fs::write(rounds.join("r1.toml"), toml::to_string(&record).unwrap()).unwrap();
        let fake = FakeRunner::new();
        fake.on("ssh", ok("0\t0\n1\t0\n"));
        let recording = crate::ledger::RecordingRunner(&fake);
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: home.path().join("cfg"),
            runner: &recording,
            detached_ticker: false,
        };
        let _scope = crate::ledger::Scope::new(&[&project]);

        let profile = crate::contracts::MachineProfile {
            id: "box-1".into(),
            label: "oci".into(),
            target: "me@box".into(),
            session: "default".into(),
        };
        let (leftovers, data, errors) = finished_worktrees(&ctx, Some(&profile));

        assert!(leftovers.is_empty());
        assert!(data.is_empty());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(fake.count("ssh"), 1);
        assert!(crate::ledger::list(&project).unwrap().is_empty());
        assert!(!project.dir().join("ledger.jsonl").exists());
    }

    #[test]
    fn an_open_working_thread_is_not_a_finished_worktree() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join(crate::harness::BOX_WORKER_MARKER),
            "lane worker\n",
        )
        .unwrap();
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let worktree = home.path().join("live-worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        crate::thread::allocate(&project, |thread| {
            thread.kind = crate::thread::Kind::Worktree;
            thread.status = crate::thread::Status::Open;
            thread.last_group = "working".into();
            thread.worktree_path = worktree.to_string_lossy().into_owned();
            thread.repo = "/repo".into();
            thread.branch = "lane".into();
        })
        .unwrap();
        let runner = runner_with_herdr("herdr 0.9.1\n");

        let (text, healthy, checks) =
            report_with_checks(&env, &root, &config, &SessionFlags::default(), &runner);

        assert!(healthy, "{text}");
        assert!(
            !text.contains(worktree.to_string_lossy().as_ref()),
            "{text}"
        );
        assert!(checks.iter().any(|check| {
            check.status == "ok"
                && check.label == "finished worktrees local"
                && check.detail == "none whose work is done"
        }));
    }

    #[test]
    fn unreadable_thread_state_makes_finished_worktrees_unknown() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join(crate::harness::BOX_WORKER_MARKER),
            "lane worker\n",
        )
        .unwrap();
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        std::fs::write(project.dir().join("threads/t-0001.toml"), "status = [\n").unwrap();
        let runner = runner_with_herdr("herdr 0.9.1\n");

        let (text, healthy, checks) =
            report_with_checks(&env, &root, &config, &SessionFlags::default(), &runner);

        assert!(healthy, "{text}");
        assert!(text.contains("thread state unknown"), "{text}");
        assert!(checks.iter().any(|check| {
            check.status == "warning"
                && check.label == "finished worktrees local"
                && check.detail.contains("unknown; could not check")
        }));
    }

    #[test]
    fn a_finished_worktree_left_on_disk_fails_the_doctor_row() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join(crate::harness::BOX_WORKER_MARKER),
            "lane worker\n",
        )
        .unwrap();
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let worktree = home.path().join("finished-worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        let thread = crate::thread::allocate(&project, |thread| {
            thread.kind = crate::thread::Kind::Worktree;
            thread.status = crate::thread::Status::Resolved;
            thread.worktree_path = worktree.to_string_lossy().into_owned();
            thread.repo = "/repo".into();
            thread.branch = "lane".into();
        })
        .unwrap();
        let rounds = project.state_dir().join("rounds");
        std::fs::create_dir_all(&rounds).unwrap();
        let record = crate::contracts::RoundRecord {
            phase: crate::contracts::RoundPhase::Abandoned,
            round: "r1".into(),
            branch: "main".into(),
            plain: "The work is closed.".into(),
            policy_hash: "policy".into(),
            manifest: crate::contracts::AdmissionManifest {
                revision: 1,
                members: vec![crate::contracts::ManifestMember {
                    thread: thread.id,
                    pin: None,
                }],
            },
            repo: "/repo".into(),
            abandoned_reason: Some("not needed".into()),
            ..crate::contracts::RoundRecord::default()
        };
        std::fs::write(rounds.join("r1.toml"), toml::to_string(&record).unwrap()).unwrap();
        let runner = runner_with_herdr("herdr 0.9.1\n");
        runner.on("status --porcelain --ignored --untracked-files=all", ok(""));

        let (text, healthy, checks) =
            report_with_checks(&env, &root, &config, &SessionFlags::default(), &runner);

        assert!(!healthy, "{text}");
        assert!(text.contains("[FAIL] finished worktrees local"), "{text}");
        assert!(text.contains(worktree.to_string_lossy().as_ref()), "{text}");
        assert!(checks.iter().any(|check| {
            check.status == "failed"
                && check.label == "finished worktrees local"
                && check.detail.contains(worktree.to_string_lossy().as_ref())
        }));
    }

    #[test]
    fn ignored_data_is_listed_separately_from_finished_worktree_leaks() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join(crate::harness::BOX_WORKER_MARKER),
            "lane worker\n",
        )
        .unwrap();
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let worktree = home.path().join("finished-worktree-with-data");
        std::fs::create_dir_all(worktree.join("camber-runs")).unwrap();
        std::fs::write(worktree.join("camber-runs/raw.bin"), vec![0; 2048]).unwrap();
        let thread = crate::thread::allocate(&project, |thread| {
            thread.kind = crate::thread::Kind::Worktree;
            thread.status = crate::thread::Status::Resolved;
            thread.worktree_path = worktree.to_string_lossy().into_owned();
            thread.repo = "/repo".into();
            thread.branch = "lane".into();
        })
        .unwrap();
        let rounds = project.state_dir().join("rounds");
        std::fs::create_dir_all(&rounds).unwrap();
        let record = crate::contracts::RoundRecord {
            phase: crate::contracts::RoundPhase::Abandoned,
            round: "r1".into(),
            branch: "main".into(),
            plain: "The work closed.".into(),
            policy_hash: "policy".into(),
            manifest: crate::contracts::AdmissionManifest {
                revision: 1,
                members: vec![crate::contracts::ManifestMember {
                    thread: thread.id,
                    pin: None,
                }],
            },
            repo: "/repo".into(),
            abandoned_reason: Some("not needed".into()),
            ..crate::contracts::RoundRecord::default()
        };
        std::fs::write(rounds.join("r1.toml"), toml::to_string(&record).unwrap()).unwrap();
        let runner = runner_with_herdr("herdr 0.9.1\n");
        runner.on(
            "status --porcelain --ignored --untracked-files=all",
            ok("!! camber-runs/raw.bin\n"),
        );

        let (text, healthy, checks) =
            report_with_checks(&env, &root, &config, &SessionFlags::default(), &runner);

        assert!(healthy, "{text}");
        assert!(text.contains("[warn] worktree data kept local"), "{text}");
        assert!(
            text.contains("camber-runs") && text.contains("KiB"),
            "{text}"
        );
        assert!(checks.iter().any(|check| {
            check.status == "warning"
                && check.label == "worktree data kept local"
                && check.detail.contains("camber-runs")
        }));
    }

    #[test]
    fn a_lane_worker_does_not_validate_coordinator_routing() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join(crate::harness::BOX_WORKER_MARKER),
            "lane worker\n",
        )
        .unwrap();
        let runner = runner_with_herdr("herdr 0.9.1\n");

        let (text, _) = report(
            &env,
            &home.path().join("root"),
            &config,
            &SessionFlags::default(),
            &runner,
        );

        assert!(text.contains("[ok  ] lane worker:"), "{text}");
        assert!(!text.contains("routing_recipe_missing"), "{text}");
        assert!(!text.contains("[FAIL] recipes:"), "{text}");
        assert_eq!(
            runner.count("agent start --help"),
            1,
            "only the parent CLI check runs; recipe validation is skipped"
        );
        assert_eq!(runner.count("machine list"), 0);
    }

    #[test]
    fn a_coordinator_whose_name_does_not_resolve_fails_the_project_row() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        write_routing_config(&home.path().join("cfg"));
        let root = home.path().join("root");
        let project = opened_project(home.path(), &root, "w1:p1", "hp-demo-coordinator");
        let cwd = project.canonical_dir().to_string_lossy().into_owned();
        let panes = format!(
            r#"{{"result":{{"panes":[{{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"{cwd}"}}]}}}}"#
        );
        let agents = format!(
            r#"{{"result":{{"agents":[{{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"{cwd}","name":"someone-else","agent":"claude","agent_status":"idle"}}]}}}}"#
        );
        let runner = runner_with_project("herdr 0.9.1\n", &panes, &agents);
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        let (text, healthy) = report(
            &env,
            &root,
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(!healthy, "{text}");
        assert!(text.contains("does not resolve"), "{text}");
    }

    #[test]
    fn announced_items_unread_across_passes_fail_the_project_row() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        write_routing_config(&home.path().join("cfg"));
        let root = home.path().join("root");
        let project = opened_project(home.path(), &root, "w1:p1", "hp-demo-coordinator");
        let cwd = project.canonical_dir().to_string_lossy().into_owned();
        let panes = format!(
            r#"{{"result":{{"panes":[{{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"{cwd}"}}]}}}}"#
        );
        let agents = format!(
            r#"{{"result":{{"agents":[{{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"{cwd}","name":"hp-demo-coordinator","agent":"claude","agent_status":"idle"}}]}}}}"#
        );
        let runner = runner_with_project("herdr 0.9.1\n", &panes, &agents);
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        runner.on("notification show", ok(r#"{"result":{"shown":true}}"#));
        // Announce an item and let the ticker count the passes where it stays
        // unread. `nudge = false` keeps the announcement out of the runner's
        // prompt path; the pass count is the same either way.
        crate::inbox::write(&project, "routine", "r", "due", "Prompt").unwrap();
        let herdr = Herdr::new("herdr", "", &runner);
        let settings = project::Settings {
            nudge: false,
            ..project::Settings::default()
        };
        let mut state = crate::steps::load_state(&project);
        for _ in 0..(crate::steps::UNREAD_NUDGE_PASSES + 1) {
            crate::steps::nudge(&project, &mut state, &settings, &herdr, None).unwrap();
        }
        crate::steps::save_state(&project, &state).unwrap();
        let (text, healthy) = report(
            &env,
            &root,
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(!healthy, "{text}");
        assert!(text.contains("unread"), "{text}");
    }

    #[test]
    fn a_registered_machine_without_live_threads_is_checked() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        write_routing_config(&home.path().join("cfg"));
        let machines = r#"[{"id":"oci-id","label":"oci","target":"me@box","session":"default","enabled":true}]"#;
        let runner = runner_with_machine_list("herdr 0.9.1\n", machines);
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        runner.on("ssh", ok(&box_facts()));
        probe_fakes(&runner);

        let (text, _) = report(
            &env,
            &home.path().join("root-with-no-projects"),
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(
            text.contains("[ok  ] machine oci: ssh target me@box"),
            "{text}"
        );
        assert!(text.contains("[ok  ] box oci capacity"), "{text}");
    }

    #[test]
    fn doctor_lists_an_orphan_box_build_folder_with_its_size() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        write_routing_config(&home.path().join("cfg"));
        let runner = runner_with_machine_list(
            "herdr 0.9.1\n",
            r#"[{"id":"oci-id","label":"oci","target":"me@box","session":"default","enabled":true}]"#,
        );
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        let facts = box_facts().replace(
            "__HERDR_BUILDS_DONE__",
            "2662400\t/home/ubuntu/build/lanes/demo-t-0099\n__HERDR_BUILDS_DONE__",
        );
        runner.on("ssh", ok(&facts));
        probe_fakes(&runner);

        let (text, healthy, checks) = report_with_checks(
            &env,
            &home.path().join("root"),
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );

        assert!(!healthy, "{text}");
        assert!(text.contains("[FAIL] finished worktrees oci"), "{text}");
        assert!(text.contains("demo-t-0099 (2.5 GiB)"), "{text}");
        assert!(checks.iter().any(|check| {
            check.status == "failed"
                && check.label == "finished worktrees oci"
                && check.detail.contains("demo-t-0099 (2.5 GiB)")
        }));
    }

    #[test]
    fn open_box_threads_keep_their_build_folders_out_of_orphan_results() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let thread = crate::thread::allocate(&project, |thread| {
            thread.status = crate::thread::Status::Open;
            thread.machine = "oci".into();
            thread.machine_id = "oci-id".into();
            thread.title = "Review r1".into();
        })
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&format!(
                "__HERDR_BUILDS__\n1\t/home/ubuntu/build/lanes/demo-{}\n__HERDR_BUILDS_DONE__\n",
                thread.id
            )),
        );
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };

        let profile = crate::contracts::MachineProfile {
            id: "oci-id".into(),
            label: "oci".into(),
            target: "me@box".into(),
            session: "default".into(),
        };
        let (leftovers, errors) = finished_build_folders(&ctx, &profile);

        assert!(leftovers.is_empty(), "{leftovers:?}");
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn unreadable_thread_ownership_does_not_turn_a_build_folder_into_an_orphan() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        std::fs::write(project.dir().join("threads/t-0099.toml"), "status = [\n").unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok("__HERDR_BUILDS__\n1\t/home/ubuntu/build/lanes/demo-t-0099\n__HERDR_BUILDS_DONE__\n"),
        );
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };

        let profile = crate::contracts::MachineProfile {
            id: "oci-id".into(),
            label: "oci".into(),
            target: "me@box".into(),
            session: "default".into(),
        };
        let (leftovers, errors) = finished_build_folders(&ctx, &profile);

        assert!(leftovers.is_empty(), "{leftovers:?}");
        assert!(
            errors
                .iter()
                .any(|error| error.contains("build ownership unknown")),
            "{errors:?}"
        );
    }

    #[test]
    fn unavailable_local_free_space_is_typed_as_unknown() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join(crate::harness::BOX_WORKER_MARKER),
            "lane worker\n",
        )
        .unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        runner.on("agent start --help", ok("--parent"));
        runner.on("herdr --version", ok("herdr 0.9.1\n"));
        runner.on("session list --json", ok(r#"{"sessions":[]}"#));
        runner.on("git --version", ok("git version 2.50.0\n"));
        runner.on("df -Pk /", fail(1, "df failed"));
        runner.on("ssh -V", ok(""));
        runner.on("rsync --version", ok("rsync 3\n"));
        runner.on("gh --version", ok("gh version 2\n"));
        runner.on("gh auth status", fail(1, "not logged in"));

        let (text, healthy, checks) = report_with_checks(
            &env,
            &home.path().join("root"),
            &config,
            &SessionFlags::default(),
            &runner,
        );

        assert!(healthy, "{text}");
        assert!(checks.iter().any(|check| {
            check.status == "warning"
                && check.label == "machine local disk"
                && check.detail.contains("free space unknown")
        }));
    }

    #[test]
    fn configured_free_disk_threshold_gates_the_local_machine() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("cfg");
        write_routing_config(&config);
        let mut text = std::fs::read_to_string(config.join("config.toml")).unwrap();
        text.push_str("\n[doctor]\nmin_free_disk_gb = 250\n");
        std::fs::write(config.join("config.toml"), text).unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = runner_with_herdr("herdr 0.9.1\n");
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );

        let (text, healthy) = report(
            &env,
            &home.path().join("root"),
            &config,
            &SessionFlags::default(),
            &runner,
        );

        assert!(!healthy, "{text}");
        assert!(text.contains("[FAIL] machine local disk"), "{text}");
        assert!(text.contains("refuses below 250 GB free"), "{text}");
    }

    #[test]
    fn report_uses_configured_recipe_overrides_for_box_logins() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        let mut recipes = default_recipes();
        // Retiring native Claude rows and moving agy to pi must retire their
        // login checks too, regardless of the unchanged recipe names.
        for recipe in recipes
            .values_mut()
            .filter(|recipe| recipe.kind == "claude")
        {
            recipe.enabled = false;
        }
        recipes.insert(
            "agy_gemini_flash".into(),
            recipes["pi_codex_sol_high"].clone(),
        );
        std::fs::write(
            config.join("config.toml"),
            format!(
                "[routing]\ndefault = \"pi_codex_sol_high\"\nretries = 1\nfallback = []\n\n{}",
                toml::to_string(&BTreeMap::from([("recipes", recipes)])).unwrap()
            ),
        )
        .unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = runner_with_machine_list(
            "herdr 0.9.1\n",
            r#"[{"id":"oci-id","label":"oci","target":"me@box","session":"default","enabled":true}]"#,
        );
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        runner.on("ssh", ok(&box_facts()));
        probe_fakes(&runner);
        let (text, _) = report(
            &env,
            &home.path().join("root"),
            &config,
            &SessionFlags::default(),
            &runner,
        );
        assert!(text.contains("[ok  ] box oci pi openai-codex"), "{text}");
        assert!(!text.contains("box oci login"), "{text}");
    }

    #[test]
    fn a_repo_row_with_a_box_path_brings_its_machine_in() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            "[routing]\ndefault = \"pi_codex_sol_high\"\nretries = 1\nfallback = []\n\n[dispatch]\nmachine = \"dispatch-box\"\n",
        )
        .unwrap();
        let project = project::create(
            &root,
            "demo",
            "",
            vec![crate::project::Repo {
                path: "/repo/on/box".into(),
                machine: Some("repo-box".into()),
                box_path: Some("/box/repo".into()),
                publish_url: None,
                ..crate::project::Repo::default()
            }],
        )
        .unwrap();
        assert!(crate::thread::list(&project).is_empty());
        let runner = FakeRunner::new();
        runner.on("machine list --json", ok("[]"));

        let machines = machines_to_check(&root, &config, &runner, "herdr").unwrap();
        assert!(machines.contains("repo-box"), "{machines:?}");
        assert!(machines.contains("dispatch-box"), "{machines:?}");
    }

    #[test]
    fn an_unreachable_registered_machine_fails_instead_of_disappearing() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        write_routing_config(&home.path().join("cfg"));
        let machines = r#"[{"id":"oci-id","label":"oci","target":"me@box","session":"default","enabled":true}]"#;
        let runner = runner_with_machine_list("herdr 0.9.1\n", machines);
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        runner.on("ssh", fail(255, "ssh: connect timed out"));

        let (text, healthy) = report(
            &env,
            &home.path().join("root-with-no-projects"),
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(!healthy, "{text}");
        assert!(
            text.contains("[FAIL] box oci: unreachable: ssh: connect timed out"),
            "{text}"
        );
    }

    fn default_recipes() -> BTreeMap<String, crate::contracts::Recipe> {
        let dir = tempfile::tempdir().unwrap();
        write_routing_config(dir.path());
        crate::launch::parse_launch_config(dir.path())
            .unwrap()
            .recipes
    }

    #[test]
    fn pi_only_codex_access_is_ready_without_a_codex_binary_or_login() {
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&(box_facts() + "pi_openai-codex/gpt-6-astra\tok\n")
                .replace("login_codex\tok", "login_codex\tmissing")),
        );
        runner.on("pane read", ok("@@pi /home/ubuntu/.local/bin/pi\n@@cmd\n/bin/cargo\n/bin/just\n/bin/node\n@@done\n"));
        probe_fakes(&runner);
        let mut recipes = default_recipes();
        recipes.retain(|_, recipe| recipe.kind == "pi" && recipe.provider == "openai-codex");
        // Every routed model gets its own readiness call.
        for recipe in recipes.values_mut() {
            recipe.enabled = true;
        }
        let rows = box_rows(
            &runner,
            "herdr",
            Path::new(""),
            &box_profile(),
            &recipes,
            12.0,
        );
        assert!(rows.iter().all(|row| row.0 == Some(true)), "{rows:?}");
        assert_eq!(
            rows.iter()
                .filter(|row| row.1.starts_with("box oci pi openai-codex/"))
                .count(),
            2
        );
        assert!(!rows.iter().any(|row| row.1.contains(" login ")));
        let calls = runner.calls.borrow();
        let ssh = calls
            .iter()
            .find(|call| call.program == "ssh")
            .unwrap()
            .display();
        assert_eq!(ssh.matches("check openai-codex").count(), 2, "{ssh}");
        assert!(!ssh.contains("codex login status"), "{ssh}");
        assert!(!ssh.contains("command -v codex"), "{ssh}");
    }

    #[test]
    fn each_native_recipe_requires_its_binary_and_login() {
        for kind in ["claude", "codex", "agy"] {
            let config = machine_config(&[kind]);
            let runner = FakeRunner::new();
            runner.on(
                "ssh",
                ok(&box_facts().replace(
                    &format!("login_{kind}\tok"),
                    &format!("login_{kind}\tmissing"),
                )),
            );
            runner.on("pane read", ok("@@pi /home/ubuntu/.local/bin/pi\n@@cmd\n/bin/cargo\n/bin/just\n/bin/node\n@@done\n"));
            probe_fakes(&runner);
            let recipes = BTreeMap::from([(
                "not_a_runtime_name".into(),
                crate::contracts::Recipe {
                    kind: kind.into(),
                    ..Default::default()
                },
            )]);
            let rows = box_rows(
                &runner,
                "herdr",
                config.path(),
                &box_profile(),
                &recipes,
                12.0,
            );
            let login = rows
                .iter()
                .find(|row| row.1 == format!("box oci login {kind}"))
                .unwrap();
            assert_eq!(login.0, Some(false), "{rows:?}");
            let tools = rows.iter().find(|row| row.1 == "box oci tools").unwrap();
            assert_eq!(tools.0, Some(false));
            assert!(tools.2.contains(kind));
            let calls = runner.calls.borrow();
            let ssh = calls
                .iter()
                .find(|call| call.program == "ssh")
                .unwrap()
                .display();
            assert!(ssh.contains(&format!("command -v {kind}")), "{ssh}");
            assert!(ssh.contains("Reply only OK."), "{ssh}");
        }
    }

    #[test]
    fn no_login_row_outlives_its_enabled_recipe() {
        let mut recipes = default_recipes();
        for recipe in recipes.values_mut() {
            recipe.enabled = false;
        }
        for recipes in [recipes, BTreeMap::new()] {
            let runner = FakeRunner::new();
            runner.on("ssh", ok(&box_facts()));
            probe_fakes(&runner);
            let rows = box_rows(
                &runner,
                "herdr",
                Path::new(""),
                &box_profile(),
                &recipes,
                12.0,
            );
            assert!(
                !rows
                    .iter()
                    .any(|row| row.1.contains(" login ") || row.1.contains(" pi ")),
                "{rows:?}"
            );
            let calls = runner.calls.borrow();
            let ssh = calls
                .iter()
                .find(|call| call.program == "ssh")
                .unwrap()
                .display();
            assert!(!ssh.contains("login_"), "{ssh}");
            assert!(!ssh.contains("herdr-pi"), "{ssh}");
        }
    }

    #[test]
    fn box_readiness_fails_closed_for_unknown_kinds_and_mismatched_providers() {
        let config = machine_config(&["unknown", "pi"]);
        let runner = FakeRunner::new();
        runner.on("ssh", ok(&box_facts()));
        probe_fakes(&runner);
        let recipes = BTreeMap::from([
            (
                "unknown".into(),
                crate::contracts::Recipe {
                    kind: "unknown".into(),
                    ..Default::default()
                },
            ),
            (
                "mismatch".into(),
                crate::contracts::Recipe {
                    kind: "pi".into(),
                    provider: "openai-codex".into(),
                    args: vec!["--provider".into(), "opencode-go".into()],
                    ..Default::default()
                },
            ),
        ]);
        let rows = box_rows(
            &runner,
            "herdr",
            config.path(),
            &box_profile(),
            &recipes,
            12.0,
        );
        for id in ["unknown", "mismatch"] {
            assert_eq!(
                rows.iter()
                    .find(|row| row.1 == format!("box oci recipe {id}"))
                    .unwrap()
                    .0,
                Some(false)
            );
        }
        assert!(
            !rows
                .iter()
                .any(|row| row.1.contains(" login ") || row.1.contains(" pi "))
        );
    }

    fn box_facts() -> String {
        let mut lines: Vec<String> = [
            "host\toci-pi",
            "boot\tenabled",
            "server\therdr 0.9.1",
            "tailscale\t100.91.36.88",
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
        for recipe in default_recipes()
            .into_values()
            .filter(|recipe| recipe.enabled && recipe.kind == "pi")
        {
            let model = crate::pi::launch::flag_value(&recipe.args, "--model").unwrap();
            lines.push(format!("pi_{}/{}\tok", recipe.provider, model));
        }
        lines.extend(["__HERDR_BUILDS__".into(), "__HERDR_BUILDS_DONE__".into()]);
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
    fn box_rows_obey_machine_kinds_and_gate_on_free_disk() {
        let runner = FakeRunner::new();
        runner.on("ssh", ok(&box_facts()));
        probe_fakes(&runner);
        let rows = box_rows(
            &runner,
            "herdr",
            Path::new(""),
            &box_profile(),
            &default_recipes(),
            12.0,
        );
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
        let configured = box_rows(
            &runner,
            "herdr",
            Path::new(""),
            &box_profile(),
            &default_recipes(),
            120.0,
        );
        assert_eq!(
            configured
                .iter()
                .find(|row| row.1 == "box oci capacity")
                .unwrap()
                .0,
            Some(false)
        );
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
        assert!(script.starts_with("sh -c "), "{script}");
        assert!(!ssh.contains("command -v claude"), "{ssh}");
        assert!(!ssh.contains("command -v codex"), "{ssh}");
        assert!(!ssh.contains("command -v agy"), "{ssh}");
        assert!(
            !rows.iter().any(|row| row.1.contains(" login ")),
            "{rows:?}"
        );
        assert!(
            ssh.contains("check openai-codex --model gpt-5.6-sol"),
            "{ssh}"
        );
        drop(calls);

        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&box_facts().replace("df_free\t100000000000", "df_free\t5000000000")),
        );
        probe_fakes(&runner);
        assert_eq!(find_row(&runner, "box oci capacity").0, Some(false));

        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&box_facts().replace("df_free\t100000000000", "df_free\tunknown")),
        );
        probe_fakes(&runner);
        let capacity = find_row(&runner, "box oci capacity");
        assert_eq!(capacity.0, None);
        assert!(capacity.2.contains("unknown GB disk free"), "{capacity:?}");

        let runner = FakeRunner::new();
        runner.on("ssh", fail(255, "ssh: connect timed out"));
        let rows = box_rows(
            &runner,
            "herdr",
            Path::new(""),
            &box_profile(),
            &default_recipes(),
            12.0,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, Some(false));
        assert!(rows[0].2.contains("unreachable"));
    }

    #[test]
    fn box_rows_keep_the_real_lane_readiness_failures() {
        let config = machine_config(&["pi", "claude", "agy"]);
        let runner = FakeRunner::new();
        let facts = box_facts()
            .replace("login_agy\tok", "login_agy\tmissing")
            .replace("pi_pro/pro\tok", "pi_pro/pro\tfail");
        runner.on("ssh", ok(&facts));
        runner.on(
            "workspace create",
            ok(r#"{"result":{"root_pane":{"workspace_id":"w9","tab_id":"w9:t1","pane_id":"w9:p1","cwd":"/home/ubuntu"}}}"#),
        );
        runner.on("pane run", ok(r#"{"result":{}}"#));
        runner.on(
            "pane read",
            ok("@@pi /home/ubuntu/.local/bin/pi\n@@cmd\n/home/ubuntu/.cargo/bin/cargo\n/home/ubuntu/.cargo/bin/just\n/usr/local/bin/node\n@@done\n"),
        );
        runner.on("workspace close", ok(r#"{"result":{}}"#));
        let rows = box_rows(
            &runner,
            "herdr",
            config.path(),
            &box_profile(),
            &default_recipes(),
            12.0,
        );
        let find = |label: &str| {
            rows.iter()
                .find(|(_, name, _)| name == label)
                .unwrap_or_else(|| panic!("no row {label}"))
        };
        assert_eq!(find("box oci login agy").0, Some(false));
        assert_eq!(find("box oci tools").0, Some(false));
        assert!(find("box oci tools").2.contains("agy claude"));
        assert!(!find("box oci tools").2.contains("codex"));
        assert_eq!(find("box oci pi pro/pro").0, Some(false));
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
        box_rows(
            runner,
            "herdr",
            Path::new(""),
            &box_profile(),
            &default_recipes(),
            12.0,
        )
        .into_iter()
        .find(|(_, name, _)| name == label)
        .unwrap_or_else(|| panic!("no row {label}"))
    }

    #[test]
    fn fork_0_9_0_with_parent_is_accepted() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        write_routing_config(&home.path().join("cfg"));
        let runner = runner_with_herdr("herdr 0.9.0\n");
        runner.on(
            "agent start --help",
            ok("usage: herdr agent start <name> --kind KIND --pane ID [--parent PANE_ID]\n[possible values: pi, claude, agy]"),
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
