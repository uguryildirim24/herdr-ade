//! `doctor`: what is installed, where things resolve, and whether it fits.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::herdr::{self, Herdr};
use crate::paths::{self, Ctx, Env, SessionFlags};
use crate::project;
use crate::runner::{Cmd, Output, Runner};

const TOOL_TIMEOUT: Duration = Duration::from_secs(10);

/// Instrument the same Runner used by all doctor dependencies, including the
/// single box SSH snapshot. Never print arguments: they can contain credentials
/// or the entire remote script. The program, duration and check are sufficient
/// to locate an expensive call; SSH script phases have their own facts below.
struct Timings<'a> {
    inner: &'a dyn Runner,
    state: Mutex<TimingState>,
}

struct TimingState {
    last: Instant,
    commands: Vec<String>,
    command_time: Duration,
    rows: Vec<String>,
    concurrent: Vec<String>,
}

impl<'a> Timings<'a> {
    fn new(inner: &'a dyn Runner) -> Self {
        Self {
            inner,
            state: Mutex::new(TimingState {
                last: Instant::now(),
                commands: Vec::new(),
                command_time: Duration::ZERO,
                rows: Vec::new(),
                concurrent: Vec::new(),
            }),
        }
    }

    fn row(&self, label: &str) {
        let mut state = self.state.lock().unwrap();
        let elapsed = state.last.elapsed();
        let commands = std::mem::take(&mut state.commands);
        state
            .rows
            .push(format!("  {label}: {:.3}s", elapsed.as_secs_f64()));
        for command in commands {
            state.rows.push(format!("    {command}"));
        }
        state.last = Instant::now();
    }

    fn remote_phases(&self, label: &str, snapshot: &str) {
        let facts = parse_facts(snapshot);
        for (phase, commands) in [
            ("facts", "host, disk"),
            ("readiness", "provider probes, command -v"),
            ("herdr", "workspace list, agent list, tab list"),
            ("builds", "find"),
        ] {
            if let Some(ms) = facts
                .get(&format!("doctor_phase_{phase}"))
                .and_then(|value| value.parse::<u64>().ok())
            {
                self.command(
                    &format!("box {label} {phase} ({commands}, grouped)"),
                    Duration::from_millis(ms),
                );
            }
        }
    }

    fn print(&self, text: &mut String) {
        let state = self.state.lock().unwrap();
        let _ = writeln!(
            text,
            "\nTimings (wall time since previous row; batched checks have a setup row):"
        );
        for row in &state.rows {
            let _ = writeln!(text, "{row}");
        }
        if !state.concurrent.is_empty() {
            let _ = writeln!(text, "Concurrent wall times (overlap the rows above):");
            for row in &state.concurrent {
                let _ = writeln!(text, "{row}");
            }
        }
    }

    fn command(&self, program: &str, elapsed: Duration) {
        let mut state = self.state.lock().unwrap();
        state.command_time += elapsed;
        state
            .commands
            .push(format!("{program}: {:.3}s", elapsed.as_secs_f64()));
    }

    fn command_time(&self) -> Duration {
        self.state.lock().unwrap().command_time
    }

    fn concurrent(&self, label: &str, elapsed: Duration) {
        self.state
            .lock()
            .unwrap()
            .concurrent
            .push(format!("  {label}: {:.3}s", elapsed.as_secs_f64()));
    }
}

/// Include only known command verbs. Paths, scripts, tokens and model arguments
/// can be private; printing an arbitrary argv from a doctor is not safe.
fn command_name(cmd: &Cmd) -> String {
    let program = Path::new(&cmd.program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&cmd.program);
    let args = if program == "git" && cmd.args.first().is_some_and(|arg| arg == "-C") {
        cmd.args.get(2..).unwrap_or_default()
    } else {
        &cmd.args
    };
    let verb = args.first().map(String::as_str).unwrap_or("");
    match (program, verb) {
        ("git", "status" | "ls-remote" | "for-each-ref" | "worktree" | "rev-list") => {
            format!("git {verb}")
        }
        ("herdr", "agent" | "pane" | "tab" | "workspace" | "machine" | "session") => {
            format!(
                "herdr {verb} {}",
                args.get(1).map(String::as_str).unwrap_or("")
            )
        }
        ("ssh", _) => "ssh (remote script)".into(),
        _ => program.into(),
    }
}

impl Runner for Timings<'_> {
    fn is_real(&self) -> bool {
        self.inner.is_real()
    }

    fn run(&self, cmd: &Cmd) -> Result<Output> {
        let start = Instant::now();
        let result = self.inner.run(cmd);
        self.command(&command_name(cmd), start.elapsed());
        result
    }

    fn run_parallel(&self, commands: &[Cmd]) -> Vec<Result<Output>> {
        // Keep the underlying runner's concurrency and scripted-test semantics.
        let start = Instant::now();
        let results = self.inner.run_parallel(commands);
        for command in commands {
            self.command(
                &format!("{} (parallel batch)", command_name(command)),
                start.elapsed(),
            );
        }
        results
    }
}

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
           elif timeout 8s {command} >/dev/null 2>&1; then probe_status=ok;\n\
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
    let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    if adapter.doctor.readiness == "pi" {
        let provider = crate::pi::launch::flag_value(&launch.args, "--provider")
            .context("pi_args_forbidden: a provider launch names no --provider")?;
        let model = crate::pi::launch::flag_value(&launch.args, "--model")
            .context("pi_args_forbidden: a provider launch names no --model")?;
        let script = format!(
            "{}\nHERDR_ADE_ROOT={} {} check {} --model {}",
            disk_script(&machine.worktrees),
            crate::remote::quote(&machine.root),
            crate::remote::quote(&machine.pi_bin),
            crate::remote::quote(&provider),
            crate::remote::quote(&model),
        );
        let rooted = crate::runner::CwdRunner::new(ctx.runner, &ctx.root);
        let output = crate::remote::ssh(
            &rooted,
            &profile.target,
            &crate::remote::with_path(&machine.path, &script),
            None,
            crate::remote::SSH_START_TIMEOUT,
        );
        let output = output?;
        check_disk_output(
            &output.stdout,
            &profile.label,
            &machine.worktrees,
            crate::launch::doctor_config(&ctx.config_dir)?.min_free_disk_gb,
        )?;
        if !output.success() {
            #[cfg(test)]
            if !output.stdout.contains("disk_free_kb\t") {
                anyhow::bail!(
                    "pi_not_ready on the box for `{provider}`: {}",
                    output.error_text()
                );
            }
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
        return Ok(());
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
    let script = format!(
        "{}\n{}",
        disk_script(&machine.worktrees),
        box_native_probe_script(&probe, false, &machine)
    );
    let rooted = crate::runner::CwdRunner::new(ctx.runner, &ctx.root);
    let output = crate::remote::ssh(
        &rooted,
        &profile.target,
        &script,
        None,
        Duration::from_millis(launch.ready_timeout_ms.max(1_000)),
    );
    let output = output?;
    check_disk_output(
        &output.stdout,
        &profile.label,
        &machine.worktrees,
        crate::launch::doctor_config(&ctx.config_dir)?.min_free_disk_gb,
    )?;
    if !output.success() {
        anyhow::bail!(probe_error(&probe.kind, &output));
    }
    Ok(())
}

/// Check the filesystem where new worktrees are written, not the machine's root volume.
fn disk_script(path: &str) -> String {
    format!(
        "df -Pk {} 2>/dev/null | awk 'NR==2 {{print \"disk_free_kb\\t\" $4}}'",
        crate::remote::quote(path)
    )
}

fn check_disk_output(output: &str, machine: &str, path: &str, floor: f64) -> Result<()> {
    #[cfg(test)]
    if !output.contains("disk_free_kb\t") && machine != crate::contracts::MACHINE_LOCAL {
        return Ok(()); // Existing box fakes do not model df; explicit df fakes do.
    }
    let free = output
        .lines()
        .find_map(|line| line.strip_prefix("disk_free_kb\t"))
        .and_then(|kb| kb.trim().parse::<u64>().ok())
        .map(|kb| kb as f64 * 1024.0 / 1_000_000_000.0);
    let Some(free) = free else {
        anyhow::bail!("unreachable: disk free space unknown on {machine} under {path}");
    };
    if free < floor {
        anyhow::bail!(
            "disk_low: {machine} has {free:.1} GB free under {path}, below [doctor].min_free_disk_gb = {}. Free space on {machine} or lower the floor, then start again.",
            display_gb(floor)
        );
    }
    Ok(())
}

/// Recheck deferred launches without creating a worktree, tab or pane.
pub(crate) fn check_start_disk(
    ctx: &Ctx,
    profile: Option<&crate::contracts::MachineProfile>,
    repo: Option<&str>,
) -> Result<()> {
    let floor = crate::launch::doctor_config(&ctx.config_dir)?.min_free_disk_gb;
    if let Some(profile) = profile.filter(|profile| !profile.is_local()) {
        let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
        let rooted = crate::runner::CwdRunner::new(ctx.runner, &ctx.root);
        let output = crate::remote::ssh(
            &rooted,
            &profile.target,
            &disk_script(&machine.worktrees),
            None,
            crate::remote::SSH_START_TIMEOUT,
        );
        check_disk_output(&output?.stdout, &profile.label, &machine.worktrees, floor)
    } else {
        let path = repo.unwrap_or(".");
        #[cfg(not(test))]
        let output = ctx
            .runner
            .run(&crate::runner::Cmd::new("df", TOOL_TIMEOUT).args(["-Pk", path]));
        #[cfg(test)]
        let output = match ctx
            .runner
            .run(&crate::runner::Cmd::new("df", TOOL_TIMEOUT).args(["-Pk", path]))
        {
            Ok(output) => Ok(output),
            Err(error) => {
                if !ctx.runner.is_real() && format!("{error:#}").contains("no rule for `df") {
                    crate::runner::RealRunner.run(
                        &crate::runner::Cmd::new("df", TOOL_TIMEOUT).args([
                            "-Pk",
                            if std::path::Path::new(path).exists() {
                                path
                            } else {
                                "/"
                            },
                        ]),
                    )
                } else {
                    Err(error)
                }
            }
        };
        let available = output
            .ok()
            .filter(|output| output.success())
            .and_then(|output| {
                output
                    .stdout
                    .lines()
                    .rev()
                    .find_map(|line| line.split_whitespace().nth(3)?.parse::<u64>().ok())
            });
        check_disk_output(
            &format!(
                "disk_free_kb\t{}",
                available.map_or_else(String::new, |n| n.to_string())
            ),
            crate::contracts::MACHINE_LOCAL,
            path,
            floor,
        )
    }
}

/// Builds the human report and its typed check results from the same facts.
pub(crate) fn run(ctx: &Ctx, session: &SessionFlags) -> Result<DoctorOutcome> {
    run_with_trace(ctx, session, None)
}

pub(crate) fn run_timed_from(
    ctx: &Ctx,
    session: &SessionFlags,
    enabled: bool,
    cli_started: Option<Instant>,
) -> Result<DoctorOutcome> {
    if !enabled {
        return run(ctx, session);
    }
    let timings = Timings::new(ctx.runner);
    if let Some(started) = cli_started {
        timings.state.lock().unwrap().last = started;
        timings.command(
            "CLI argument, root and workspace resolution",
            started.elapsed(),
        );
        timings.row("doctor CLI startup");
    }
    let traced = Ctx {
        env: ctx.env,
        root: ctx.root.clone(),
        config_dir: ctx.config_dir.clone(),
        runner: &timings,
        detached_ticker: ctx.detached_ticker,
    };
    run_with_trace(&traced, session, Some(&timings))
}

fn run_with_trace(
    ctx: &Ctx,
    session: &SessionFlags,
    timings: Option<&Timings<'_>>,
) -> Result<DoctorOutcome> {
    // Provider probes are independent of both the box and local checks.
    let setup_start = Instant::now();
    let pi_models =
        crate::pi::doctor::configured_routed_models(&ctx.config_dir).unwrap_or_default();
    if let Some(timings) = timings {
        timings.command(
            "pi routed model inventory (file walk)",
            setup_start.elapsed(),
        );
    }
    let pi_worker = ctx.runner.is_real().then(|| {
        let root = ctx.root.clone();
        let models = pi_models.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let result =
                crate::pi_ade::doctor_rows_with(&crate::runner::RealRunner, &root, &models);
            (result, start.elapsed())
        })
    });
    let (mut text, mut healthy, mut checks) = report_with_checks(
        ctx.env,
        &ctx.root,
        &ctx.config_dir,
        session,
        ctx.runner,
        timings,
    );
    // The pi rows read only providers named by enabled configured recipes;
    // unused built-in provider knowledge never causes a doctor failure.
    let pi_start = Instant::now();
    let command_start = timings.map(Timings::command_time).unwrap_or_default();
    let pi_result = if let Some(worker) = pi_worker {
        let (result, duration) = worker.join().unwrap_or_else(|_| {
            (
                Err(anyhow::anyhow!("pi readiness worker failed")),
                Duration::ZERO,
            )
        });
        if let Some(timings) = timings {
            timings.concurrent("pi readiness (provider probes)", duration);
        }
        result
    } else {
        let result = crate::pi_ade::doctor_rows_with(ctx.runner, &ctx.root, &pi_models);
        if let Some(timings) = timings {
            let commands = timings.command_time().saturating_sub(command_start);
            timings.command(
                "pi readiness setup and cache (in-process)",
                pi_start.elapsed().saturating_sub(commands),
            );
        }
        result
    };
    if let Some(timings) = timings {
        timings.row("pi readiness batch");
    }
    match pi_result {
        Ok((rows, pi_healthy)) => {
            healthy &= pi_healthy;
            for row in rows {
                let status = match row.level {
                    crate::pi::doctor::Level::Ok => "ok",
                    crate::pi::doctor::Level::Warn => "warning",
                    crate::pi::doctor::Level::Fail => "failed",
                };
                if let Some(timings) = timings {
                    timings.row(&row.label);
                }
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
            if let Some(timings) = timings {
                timings.row("pi");
            }
            checks.push(CheckResult {
                status: "failed".into(),
                label: "pi".into(),
                detail: detail.clone(),
            });
            let _ = writeln!(text, "[FAIL] pi: {detail}");
        }
    }
    if let Some(timings) = timings {
        timings.row("doctor report finalization");
        timings.print(&mut text);
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
    let (text, healthy, _) = report_with_checks(env, root, config_dir, session, runner, None);
    (text, healthy)
}

type BoxSnapshot = (Vec<(Option<bool>, String, String)>, String, Duration);

fn prefetch_box_snapshots(
    env: &Env,
    root: &Path,
    config_dir: &Path,
    bin: &str,
) -> BTreeMap<String, BoxSnapshot> {
    let real = crate::runner::RealRunner;
    let runner = crate::runner::CwdRunner::new(&real, root);
    let mut result = BTreeMap::new();
    if let Ok(machines) = machines_to_check(root, config_dir, &runner, bin)
        && let Ok(config) = crate::launch::parse_launch_config(config_dir)
    {
        let ctx = Ctx {
            env,
            root: root.to_path_buf(),
            config_dir: config_dir.to_path_buf(),
            runner: &runner,
            detached_ticker: false,
        };
        for machine in machines {
            if let Ok(profile) = crate::remote::machine_profile(&runner, bin, config_dir, &machine)
                && !profile.is_local()
                && !result.contains_key(&profile.id)
            {
                let start = Instant::now();
                let mut snapshot = String::new();
                let rows = box_rows_with_snapshot(
                    &runner,
                    config_dir,
                    &profile,
                    &config.recipes,
                    config.doctor.min_free_disk_gb,
                    Some((&ctx, &mut snapshot)),
                );
                result.insert(profile.id, (rows, snapshot, start.elapsed()));
            }
        }
    }
    result
}

fn report_with_checks(
    env: &Env,
    root: &Path,
    config_dir: &Path,
    session: &SessionFlags,
    runner: &dyn Runner,
    timings: Option<&Timings<'_>>,
) -> (String, bool, Vec<CheckResult>) {
    // The box owns no local state: launch its snapshot before local checks.
    // Scripted runners stay sequential so their injected answers remain stable.
    let box_worker = (runner.is_real()
        && !config_dir.join(crate::harness::BOX_WORKER_MARKER).is_file())
    .then(|| {
        let root = root.to_path_buf();
        let config = config_dir.to_path_buf();
        let bin = env.herdr_bin();
        let env = env.clone();
        std::thread::spawn(move || prefetch_box_snapshots(&env, &root, &config, &bin))
    });
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
        if let Some(timings) = timings {
            timings.row(label);
        }
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

    if cfg!(target_os = "macos") {
        let supervisor_loaded = crate::harness::ticker_supervisor_loaded();
        check(
            &mut out,
            supervisor_loaded.then_some(true),
            "ticker supervisor",
            format!(
                "launchd {} ({})",
                crate::harness::ticker_agent_label(),
                if supervisor_loaded {
                    "loaded"
                } else {
                    "not loaded; run `ha harness install`"
                }
            ),
        );
    }

    if let Ok(repos) = crate::harness::repos(config_dir) {
        for repo in repos {
            if repo.box_path.is_some()
                && repo
                    .publish_url
                    .as_deref()
                    .is_none_or(|url| url.trim().is_empty())
            {
                check(
                    &mut out,
                    Some(false),
                    &format!("harness repo {}", repo.path),
                    "box_path has no publish_url; add the URL of the remote that publishes the lane branch".into(),
                );
            }
        }
    }
    for slug in project::list_slugs(root) {
        let Ok(project) = project::Project::load(root, &slug) else {
            continue;
        };
        let label = format!("project {slug}");
        if let Ok((settings, _)) = project.read_project_md() {
            for repo in &settings.repos {
                if let Some(gates) = &repo.gates {
                    for gate in gates {
                        if gate.paths.as_ref().is_some_and(Vec::is_empty) {
                            check(
                                &mut out,
                                Some(false),
                                &format!("{label} gate {}", gate.command),
                                "paths = [] selects no files; omit paths to always run".into(),
                            );
                        }
                        for pattern in gate.paths.iter().flatten() {
                            let result = crate::gate_paths::validate(pattern).and_then(|()| {
                                let files = crate::repo::Git::new(runner, &repo.path)
                                    .run(&["ls-files", "-z", "--"])?;
                                Ok(files
                                    .split('\0')
                                    .any(|file| crate::gate_paths::matches(pattern, file)))
                            });
                            match result {
                                Ok(true) => {}
                                Ok(false) => check(
                                    &mut out,
                                    Some(false),
                                    &format!("{label} gate {}", gate.command),
                                    format!(
                                        "`{pattern}` matches no tracked files in {}",
                                        repo.path
                                    ),
                                ),
                                Err(error) => check(
                                    &mut out,
                                    Some(false),
                                    &format!("{label} gate {}", gate.command),
                                    format!("`{pattern}`: {error:#}"),
                                ),
                            }
                        }
                    }
                }
                if repo.box_path.is_some()
                    && repo
                        .publish_url
                        .as_deref()
                        .is_none_or(|url| url.trim().is_empty())
                {
                    check(
                        &mut out,
                        Some(false),
                        &format!("{label} repo {}", repo.path),
                        "box_path has no publish_url; add the URL of the remote that publishes the lane branch".into(),
                    );
                }
            }
        }
        if let Some(warning) = crate::thread::memory_use(&project).warning() {
            check(&mut out, None, &format!("{label} memory"), warning);
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
                let ok = if !pane { None } else { named };
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
        let mut prefetched = box_worker
            .and_then(|worker| worker.join().ok())
            .unwrap_or_default();
        if let Some(timings) = timings {
            timings.row("box snapshot wait (concurrent with local checks)");
        }
        match machines_to_check(root, config_dir, runner, &bin)
            .and_then(|machines| Ok((machines, crate::launch::parse_launch_config(config_dir)?)))
        {
            Ok((machines, config)) => {
                let mut checked_ids = BTreeSet::new();
                for machine in machines {
                    match crate::remote::machine_profile(runner, &bin, config_dir, &machine) {
                        Ok(profile) if !checked_ids.insert(profile.id.clone()) => continue,
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
                                &format!("machine {} ({})", profile.id, profile.label),
                                format!("ssh target {}", profile.target),
                            );
                            let cached = prefetched.remove(&profile.id);
                            let overlapped = cached.is_some();
                            let (box_rows, box_snapshot, elapsed) = cached.unwrap_or_else(|| {
                                let start = Instant::now();
                                let mut snapshot = String::new();
                                let rows = box_rows_with_snapshot(
                                    runner,
                                    config_dir,
                                    &profile,
                                    &config.recipes,
                                    config.doctor.min_free_disk_gb,
                                    Some((&ctx, &mut snapshot)),
                                );
                                (rows, snapshot, start.elapsed())
                            });
                            if let Some(timings) = timings {
                                if overlapped {
                                    timings.concurrent(
                                        &format!(
                                            "box {} snapshot (ssh remote script)",
                                            profile.label
                                        ),
                                        elapsed,
                                    );
                                }
                                timings.remote_phases(&profile.label, &box_snapshot);
                                timings.row(&format!("box {} snapshot result", profile.label));
                            }
                            for (ok, label, detail) in box_rows {
                                check(&mut out, ok, &label, detail);
                            }
                            let herdr = Herdr::new(&bin, "", runner).on_machine(&profile.id);
                            check_workspace_leaks_with_snapshot(
                                &mut out,
                                &mut check,
                                root,
                                &profile.id,
                                &format!("machine {}", profile.label),
                                &herdr,
                                Some(&box_snapshot),
                            );
                            let ctx = Ctx {
                                env,
                                root: root.to_path_buf(),
                                config_dir: config_dir.to_path_buf(),
                                runner,
                                detached_ticker: false,
                            };
                            let (builds, errors) =
                                finished_build_folders_with_snapshot(&ctx, &profile, &box_snapshot);
                            check(
                                &mut out,
                                worktree_check_status(&builds, &errors),
                                &format!("finished build folders {}", profile.label),
                                worktree_check_detail(&builds, &errors),
                            );
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
        "{free_gb:.1} GB free; fails below {} GB free",
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
            "remove these finished build folders: {}",
            leftovers.join(", ")
        ),
        (true, false) => format!("unknown; could not check: {}", errors.join("; ")),
        (false, false) => format!(
            "remove these finished build folders: {}; unknown for: {}",
            leftovers.join(", "),
            errors.join("; ")
        ),
    }
}

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

fn build_folder_script(root: &str) -> String {
    format!(
        "printf '__HERDR_BUILDS__\\n'; if test -d {root}; then find {root} -mindepth 1 -maxdepth 1 -type d -print; fi; printf '__HERDR_BUILDS_DONE__\\n'",
        root = crate::remote::quote(root),
    )
}

fn finished_build_folders_with_snapshot(
    ctx: &Ctx,
    profile: &crate::contracts::MachineProfile,
    snapshot: &str,
) -> (Vec<String>, Vec<String>) {
    finished_build_folders_impl(ctx, profile, Some(snapshot))
}

#[cfg(test)]
fn finished_build_folders(
    ctx: &Ctx,
    profile: &crate::contracts::MachineProfile,
) -> (Vec<String>, Vec<String>) {
    finished_build_folders_impl(ctx, profile, None)
}

fn finished_build_folders_impl(
    ctx: &Ctx,
    profile: &crate::contracts::MachineProfile,
    snapshot: Option<&str>,
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
    let script = build_folder_script(&root);
    let output = if let Some(snapshot) = snapshot {
        crate::runner::Output {
            code: Some(0),
            stdout: snapshot.to_owned(),
            ..Default::default()
        }
    } else {
        match crate::remote::ssh(ctx.runner, &profile.target, &script, None, TOOL_TIMEOUT) {
            Ok(output) if output.success() => output,
            Ok(output) => {
                errors.push(output.error_text());
                return (Vec::new(), errors);
            }
            Err(error) => {
                errors.push(format!("{error:#}"));
                return (Vec::new(), errors);
            }
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
        let path = line.trim();
        let Some(name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
            errors.push(format!("build folder path unreadable: {path}"));
            continue;
        };
        let ownership_unknown = uncertain_projects.iter().any(|slug| {
            name.strip_prefix(slug)
                .and_then(|suffix| suffix.strip_prefix('-'))
                .is_some_and(|id| crate::thread::validate_id(id).is_ok())
        });
        if !active.contains(name) && !ownership_unknown {
            leftovers.push(path.to_string());
        }
    }
    if !complete {
        errors.push(format!("could not list build folders under {root}"));
    }
    (leftovers, errors)
}

fn snapshot_list<T: serde::de::DeserializeOwned>(
    snapshot: &str,
    key: &str,
    field: &str,
) -> Result<Vec<T>> {
    let facts = parse_facts(snapshot);
    let raw = facts.get(key).context("box snapshot has no herdr answer")?;
    let value: serde_json::Value =
        serde_json::from_str(raw).with_context(|| format!("box herdr {key} reply was invalid"))?;
    if let Some(error) = value.get("error") {
        anyhow::bail!("box herdr {key}: {error}");
    }
    serde_json::from_value(value["result"][field].clone())
        .with_context(|| format!("box herdr {key} reply changed"))
}

fn check_workspace_leaks(
    out: &mut String,
    check: &mut impl FnMut(&mut String, Option<bool>, &str, String),
    root: &Path,
    machine: &str,
    display: &str,
    herdr: &Herdr<'_>,
) {
    check_workspace_leaks_with_snapshot(out, check, root, machine, display, herdr, None)
}

fn check_workspace_leaks_with_snapshot(
    out: &mut String,
    check: &mut impl FnMut(&mut String, Option<bool>, &str, String),
    root: &Path,
    machine: &str,
    display: &str,
    herdr: &Herdr<'_>,
    snapshot: Option<&str>,
) {
    let workspaces = match snapshot.map_or_else(
        || {
            herdr
                .workspace_list()
                .map_err(|error| anyhow::anyhow!("{error}"))
        },
        |text| snapshot_list(text, "herdr_workspaces", "workspaces"),
    ) {
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
    let agents = match snapshot.map_or_else(
        || {
            herdr
                .agent_list()
                .map_err(|error| anyhow::anyhow!("{error}"))
        },
        |text| snapshot_list(text, "herdr_agents", "agents"),
    ) {
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
    let leaked: Vec<_> = workspaces
        .iter()
        .filter(|workspace| !default_shell.contains(&workspace.workspace_id))
        // A saved project's box workspace may intentionally hold a shell.
        // Its individual unowned tabs are advisory below; duplicate project
        // labels still fail because ownership is ambiguous.
        .filter(|workspace| {
            machine == "local" || !project_workspaces.contains(&workspace.workspace_id)
        })
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
    let tabs = match snapshot.map_or_else(
        || herdr.tab_list().map_err(|error| anyhow::anyhow!("{error}")),
        |text| snapshot_list(text, "herdr_tabs", "tabs"),
    ) {
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
    let status = if duplicate_labels.is_empty() {
        orphan_tabs.is_empty().then_some(true)
    } else {
        Some(false)
    };
    let detail = if duplicate_labels.is_empty() && orphan_tabs.is_empty() {
        format!(
            "{} project workspaces; every tab has an agent or an open lane",
            project_workspaces.len()
        )
    } else {
        format!(
            "duplicate labels: {}; unowned shell tabs left open: {}",
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
    check(out, status, &format!("{display} project tabs"), detail);
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
#[cfg(test)]
fn box_rows(
    runner: &dyn Runner,
    _herdr_bin: &str,
    config_dir: &Path,
    profile: &crate::contracts::MachineProfile,
    recipes: &BTreeMap<String, crate::contracts::Recipe>,
    min_free_disk_gb: f64,
) -> Vec<(Option<bool>, String, String)> {
    box_rows_with_snapshot(runner, config_dir, profile, recipes, min_free_disk_gb, None)
}

fn box_rows_with_snapshot(
    runner: &dyn Runner,
    config_dir: &Path,
    profile: &crate::contracts::MachineProfile,
    recipes: &BTreeMap<String, crate::contracts::Recipe>,
    min_free_disk_gb: f64,
    snapshot: Option<(&Ctx<'_>, &mut String)>,
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
    let mut repos = match crate::harness::repos(config_dir) {
        Ok(repos) => repos,
        Err(error) => {
            rows.push((
                Some(false),
                format!("box {label} repositories"),
                format!("{error:#}"),
            ));
            Vec::new()
        }
    };
    repos.extend(machine_paths.repos.clone());
    let mut script = format!(
        "set -u\n\
         PATH={path}; export PATH\n\
         printf 'doctor_active\\tfacts\\n'\n\
         doctor_start=$(date +%s%3N)\n\
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
         printf 'rules\\t%s\\n' \"$(sha256sum {home}/.config/herdr-ade/RULES.md 2>/dev/null | cut -d' ' -f1 || true)\"\n\
",
        path = crate::remote::quote(&machine_paths.path),
        home = crate::remote::quote(&machine_paths.home),
    );
    script.push_str("doctor_now=$(date +%s%3N); printf 'doctor_phase_facts\\t%s\\n' \"$((doctor_now-doctor_start))\"; doctor_start=$doctor_now\n");
    script.push_str(
        "printf 'doctor_active\\treadiness (parallel provider probes)\\n'\ndoctor_jobs=\n",
    );
    for probe in natives.values() {
        script.push_str("(\n");
        script.push_str(&box_native_probe_script(probe, true, &machine_paths));
        script.push_str(") & doctor_jobs=\"$doctor_jobs $!\"\n");
    }
    // The lane shell receives this exact PATH. Check the first pi hit and
    // required executables in the same box invocation as the other facts.
    script.push_str("printf 'pane_pi\\t%s\\n' \"$(command -v pi 2>/dev/null || true)\"\n");
    for tool in ["cargo", "just", "node"]
        .into_iter()
        .chain(natives.values().map(|probe| probe.program.as_str()))
    {
        script.push_str(&format!(
            "printf 'pane_tool_{tool}\\t%s\\n' \"$(command -v {tool} 2>/dev/null || true)\"\n",
            tool = crate::remote::quote(tool),
        ));
    }
    // Pi readiness is read on the box through its own wrapper and login store
    // (SPEC-remote §3.3, SPEC-pi §3.4, item 101): never Mac auth.
    for (provider, model) in &providers {
        let provider = crate::remote::quote(provider);
        let model = crate::remote::quote(model);
        script.push_str(&format!(
            "( HERDR_ADE_ROOT={root} timeout 8s {pi_bin} check {provider} --model {model} >/dev/null 2>&1 && printf 'pi_%s/%s\\tok\\n' {provider} {model} || printf 'pi_%s/%s\\tfail\\n' {provider} {model} ) & doctor_jobs=\"$doctor_jobs $!\"\n",
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
    script.push_str("for doctor_job in $doctor_jobs; do wait \"$doctor_job\"; done\n");
    if snapshot.is_some() {
        script.push_str("doctor_now=$(date +%s%3N); printf 'doctor_phase_readiness\\t%s\\n' \"$((doctor_now-doctor_start))\"; doctor_start=$doctor_now\n");

        for (key, command) in [
            ("workspaces", "workspace list"),
            ("agents", "agent list"),
            ("tabs", "tab list"),
        ] {
            script.push_str(&format!(
                "printf 'doctor_active\\therdr {key}\\n'; printf 'herdr_{key}\\t%s\\n' \"$(HERDR_SESSION={session} \"$herdr_bin\" {command} 2>/dev/null | tr '\\n\\t' '  ')\"\n",
                session = crate::remote::quote(&profile.session),
            ));
        }
        script.push_str("doctor_now=$(date +%s%3N); printf 'doctor_phase_herdr\\t%s\\n' \"$((doctor_now-doctor_start))\"; doctor_start=$doctor_now\n");
        script.push_str("printf 'doctor_active\\tbuilds\\n'\n");
        script.push_str(&build_folder_script(&machine_paths.build));
        script.push_str("doctor_now=$(date +%s%3N); printf 'doctor_phase_builds\\t%s\\n' \"$((doctor_now-doctor_start))\"; doctor_start=$doctor_now\n");
    }
    let facts = match crate::remote::ssh(
        runner,
        &profile.target,
        &script,
        None,
        Duration::from_secs(15),
    ) {
        Ok(out) if out.success() => {
            if let Some((_, snapshot)) = snapshot {
                *snapshot = out.stdout.clone();
            }
            parse_facts(&out.stdout)
        }
        Ok(out) => {
            let phase = parse_facts(&out.stdout).get("doctor_active").cloned();
            let detail = if out.timed_out
                && let Some(phase) = phase
            {
                format!("slow: snapshot timed out while running {phase}")
            } else {
                format!("unreachable: {}", out.error_text())
            };
            return vec![(Some(false), format!("box {label}"), detail)];
        }
        Err(error) => {
            return vec![(
                Some(false),
                format!("box {label}"),
                if crate::remote::is_unreachable(&format!("{error:#}")) {
                    format!("{error:#}")
                } else {
                    format!("unreachable: {error:#}")
                },
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
    let pi = fact("pane_pi");
    let wrapper = Path::new(&machine_paths.pi_bin)
        .parent()
        .map(|dir| dir.join("pi").to_string_lossy().into_owned())
        .unwrap_or_default();
    rows.push((
        Some(pi == wrapper),
        format!("box {label} wrapper"),
        if pi.is_empty() {
            "the lane PATH did not resolve `pi`".into()
        } else {
            format!("`command -v pi` first hit: {pi}")
        },
    ));
    let required_tools: Vec<&str> = ["cargo", "just", "node"]
        .into_iter()
        .chain(natives.values().map(|probe| probe.program.as_str()))
        .collect();
    let missing: Vec<_> = required_tools
        .iter()
        .filter(|tool| fact(&format!("pane_tool_{tool}")).is_empty())
        .copied()
        .collect();
    rows.push((
        Some(missing.is_empty()),
        format!("box {label} tools"),
        if missing.is_empty() {
            required_tools
                .iter()
                .map(|tool| fact(&format!("pane_tool_{tool}")))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            format!("the lane PATH cannot find: {}", missing.join(" "))
        },
    ));
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
            "{} OCPU, {} GB RAM free, {} GB disk free; {}; fails below {} GB free",
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

    fn write_machine_config(config: &Path) {
        std::fs::create_dir_all(config).unwrap();
        let path = config.join("config.toml");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(path, format!("{existing}{}", crate::remote::TEST_MACHINE)).unwrap();
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
                "[machines.buildbox]\nlabel = \"buildbox\"\ntarget = \"buildbox-pi\"\nsession = \"default\"\nhome = \"/home/agent\"\nroot = \"/home/agent/.herdr-ade\"\nworktrees = \"/home/agent/projects\"\nbuild = \"/home/agent/build/lanes\"\npath = \"/home/agent/.local/bin:/home/agent/.cargo/bin:/usr/local/bin:/usr/bin:/bin\"\nade_bin = \"/home/agent/.local/bin/herdr-ade\"\npi_bin = \"/home/agent/.local/bin/herdr-pi\"\nkinds = [{kinds}]\n[[machines.buildbox.repos]]\npath = \"/local/herdr\"\nbox_path = \"/home/agent/projects/herdr\"\npublish_url = \"https://example.test/herdr.git\"\n[[machines.buildbox.repos]]\npath = \"/local/herdr-ade\"\nbox_path = \"/home/agent/projects/herdr-ade\"\npublish_url = \"https://example.test/herdr-ade.git\"\n"
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
        for (command, reply) in [
            ("workspace list", r#"{"result":{"workspaces":[]}}"#),
            ("tab list", r#"{"result":{"tabs":[]}}"#),
            ("agent list", r#"{"result":{"agents":[]}}"#),
        ] {
            runner.on_fn(
                move |cmd| cmd.program == "herdr" && cmd.display().contains(command),
                move |_| Ok(ok(reply)),
            );
        }
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
                "--new-project",
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
            thread.machine = "buildbox".into();
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

        check_workspace_leaks(
            &mut text,
            &mut check,
            &root,
            "abc",
            "machine buildbox",
            &herdr,
        );

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

        check_workspace_leaks(
            &mut text,
            &mut check,
            &root,
            "abc",
            "machine buildbox",
            &herdr,
        );

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

        check_workspace_leaks(
            &mut text,
            &mut check,
            &root,
            "abc",
            "machine buildbox",
            &herdr,
        );

        assert!(!healthy);
        assert!(text.contains("duplicate labels: Demo (2)"), "{text}");
        assert!(
            text.contains("unowned shell tabs left open: w1:t1"),
            "{text}"
        );
        assert_eq!(runner.count("tab close"), 0);
    }

    #[test]
    fn an_unowned_shell_tab_is_advisory_and_left_open() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        project::create(&root, "demo", "", vec![]).unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "workspace list",
            ok(r#"{"result":{"workspaces":[{"workspace_id":"w1","label":"Demo"}]}}"#),
        );
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        runner.on(
            "tab list",
            ok(r#"{"result":{"tabs":[{"workspace_id":"w1","tab_id":"w1:t1"}]}}"#),
        );
        let herdr = Herdr::new("herdr", "", &runner).on_machine("abc");
        let mut text = String::new();
        let mut healthy = true;
        let mut check = |out: &mut String, ok: Option<bool>, label: &str, detail: String| {
            healthy &= ok != Some(false);
            let _ = writeln!(out, "{label}: {detail}");
        };

        check_workspace_leaks(
            &mut text,
            &mut check,
            &root,
            "abc",
            "machine buildbox",
            &herdr,
        );

        assert!(healthy, "{text}");
        assert!(
            text.contains("unowned shell tabs left open: w1:t1"),
            "{text}"
        );
        assert_eq!(runner.count("tab close"), 0);
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
        assert!(text.contains("[warn] root"));
        assert!(text.contains(&format!("root:       {}", root.display())));
        assert!(!root.exists(), "doctor must not create the root");
        assert!(text.contains("plugin:     herdr-ade"), "{text}");
        assert!(text.contains("crate:      herdr-ade"), "{text}");
        assert!(text.contains("prefix:"), "{text}");
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
        crate::prompt::record_test_request(&project, "q-1", "Keep helper briefs focused.").unwrap();
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
    fn timings_name_checks_and_commands_without_leaking_arguments() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(crate::harness::BOX_WORKER_MARKER), "worker\n").unwrap();
        let runner = runner_with_herdr("herdr 0.9.1\n");
        let ctx = Ctx {
            env: &env,
            root: home.path().join("root"),
            config_dir: config,
            runner: &runner,
            detached_ticker: false,
        };
        let result = run_timed_from(
            &ctx,
            &SessionFlags::default(),
            true,
            Some(Instant::now() - Duration::from_secs(2)),
        )
        .unwrap();
        assert!(result.message.contains("Timings (wall time"));
        assert!(result.message.contains("  doctor CLI startup: 2."));
        assert!(result.message.contains("  herdr:"));
        assert!(result.message.contains("    herdr:"));
        assert!(!result.message.contains("  branches:"));
        let timing = Timings::new(&runner);
        timing.concurrent(
            "box buildbox snapshot (ssh remote script)",
            Duration::from_millis(250),
        );
        timing.remote_phases("buildbox", "doctor_phase_builds\t250\n");
        timing.row("box buildbox snapshot");
        let mut detail = String::new();
        timing.print(&mut detail);
        assert!(detail.contains("  box buildbox snapshot:"), "{detail}");
        assert!(
            detail.contains("  box buildbox snapshot (ssh remote script): 0.250s"),
            "{detail}"
        );
        assert!(
            detail.contains("    box buildbox builds (find, grouped): 0.250s"),
            "{detail}"
        );
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
    fn a_registered_machine_without_live_threads_is_checked() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        write_routing_config(&home.path().join("cfg"));
        write_machine_config(&home.path().join("cfg"));
        let path = home.path().join("cfg/config.toml");
        let config = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            path,
            format!("{config}\n[dispatch]\nmachine = \"buildbox-id\"\n"),
        )
        .unwrap();
        let machines = r#"[{"id":"buildbox-id","label":"buildbox","target":"me@box","session":"default","enabled":true}]"#;
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
            text.contains("[ok  ] machine buildbox-id (buildbox): ssh target me@box"),
            "{text}"
        );
        assert!(text.contains("[ok  ] box buildbox capacity"), "{text}");
        assert_eq!(
            runner
                .calls
                .borrow()
                .iter()
                .filter(|call| call.program == "ssh" && call.args != ["-V"])
                .count(),
            1,
            "box facts, worktree presence and build folders share one SSH call"
        );
        assert_eq!(
            runner.count("workspace create"),
            0,
            "doctor must not spawn a disposable probe pane"
        );
    }

    #[test]
    fn doctor_on_a_test_root_does_not_scan_branches() {
        struct NoBranchScan;
        impl Runner for NoBranchScan {
            fn run(&self, cmd: &Cmd) -> Result<crate::runner::Output> {
                assert!(
                    !cmd.args.iter().any(|arg| matches!(
                        arg.as_str(),
                        "for-each-ref" | "ls-remote" | "merge-base"
                    )),
                    "doctor scanned branches: {}",
                    cmd.display()
                );
                crate::runner::RealRunner.run(cmd)
            }
        }
        let fx = crate::testkit::fixture();
        let original = fx.world.ctx();
        let ctx = Ctx {
            runner: &NoBranchScan,
            ..original
        };
        let _ = run(&ctx, &SessionFlags::default());
    }

    #[test]
    fn doctor_lists_an_orphan_box_build_folder() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        write_routing_config(&home.path().join("cfg"));
        write_machine_config(&home.path().join("cfg"));
        let runner = runner_with_machine_list(
            "herdr 0.9.1\n",
            r#"[{"id":"buildbox-id","label":"buildbox","target":"me@box","session":"default","enabled":true}]"#,
        );
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        let facts = box_facts().replace(
            "__HERDR_BUILDS_DONE__",
            "/home/agent/build/lanes/demo-t-0099\n__HERDR_BUILDS_DONE__",
        );
        runner.on("ssh", ok(&facts));
        probe_fakes(&runner);

        let (text, healthy, checks) = report_with_checks(
            &env,
            &home.path().join("root"),
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
            None,
        );

        assert!(!healthy, "{text}");
        assert!(
            text.contains("[FAIL] finished build folders buildbox"),
            "{text}"
        );
        assert!(text.contains("demo-t-0099"), "{text}");
        assert!(checks.iter().any(|check| {
            check.status == "failed"
                && check.label == "finished build folders buildbox"
                && check.detail.contains("demo-t-0099")
        }));
    }

    #[test]
    fn open_box_threads_keep_their_build_folders_out_of_orphan_results() {
        let home = tempfile::tempdir().unwrap();
        write_machine_config(&home.path().join("cfg"));
        let env = Env::for_test(home.path(), &[]);
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let thread = crate::thread::allocate(&project, |thread| {
            thread.status = crate::thread::Status::Open;
            thread.machine = "buildbox".into();
            thread.machine_id = "buildbox-id".into();
            thread.title = "Review r1".into();
        })
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&format!(
                "__HERDR_BUILDS__\n/home/agent/build/lanes/demo-{}\n__HERDR_BUILDS_DONE__\n",
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
            id: "buildbox-id".into(),
            label: "buildbox".into(),
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
        write_machine_config(&home.path().join("cfg"));
        let env = Env::for_test(home.path(), &[]);
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        std::fs::create_dir(project.state_dir().join("threads")).unwrap();
        std::fs::write(
            project.state_dir().join("threads/t-0099.toml"),
            "status = [\n",
        )
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok("__HERDR_BUILDS__\n/home/agent/build/lanes/demo-t-0099\n__HERDR_BUILDS_DONE__\n"),
        );
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };

        let profile = crate::contracts::MachineProfile {
            id: "buildbox-id".into(),
            label: "buildbox".into(),
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

        let (text, healthy, checks) = report_with_checks(
            &env,
            &home.path().join("root"),
            &config,
            &SessionFlags::default(),
            &runner,
            None,
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
        assert!(text.contains("fails below 250 GB free"), "{text}");
    }

    #[test]
    fn unreadable_start_disk_is_unreachable_not_low() {
        let error = check_disk_output("disk_free_kb\tunknown\n", "buildbox", "/box/work", 12.0)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "unreachable: disk free space unknown on buildbox under /box/work"
        );
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
                "[routing]\ndefault = \"pi_codex_sol_high\"\nretries = 1\n\n{}{}",
                toml::to_string(&BTreeMap::from([("recipes", recipes)])).unwrap(),
                crate::remote::TEST_MACHINE
            ),
        )
        .unwrap();
        let env = Env::for_test(home.path(), &[]);
        let runner = runner_with_machine_list(
            "herdr 0.9.1\n",
            r#"[{"id":"buildbox-id","label":"buildbox","target":"me@box","session":"default","enabled":true}]"#,
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
        assert!(
            text.contains("[ok  ] box buildbox pi openai-codex"),
            "{text}"
        );
        assert!(!text.contains("box buildbox login"), "{text}");
    }

    #[test]
    fn doctor_flags_unpublishable_project_repo_even_without_a_coordinator() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let root = home.path().join("root");
        crate::project::create(
            &root,
            "demo",
            "",
            vec![crate::project::Repo {
                path: "/repo".into(),
                box_path: Some("/box/repo".into()),
                ..Default::default()
            }],
        )
        .unwrap();
        let runner = FakeRunner::new();
        let (text, healthy) = report(
            &env,
            &root,
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(!healthy);
        assert!(
            text.contains("[FAIL] project demo repo /repo: box_path has no publish_url"),
            "{text}"
        );
    }

    #[test]
    fn doctor_flags_scoped_gate_glob_with_no_tracked_match() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let repo = home.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let init = std::process::Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap();
        assert!(init.success());
        std::fs::write(repo.join("README.md"), "hello").unwrap();
        let add = std::process::Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "add", "README.md"])
            .status()
            .unwrap();
        assert!(add.success());
        let root = home.path().join("root");
        crate::project::create(
            &root,
            "demo",
            "",
            vec![crate::project::Repo {
                path: repo.to_string_lossy().into_owned(),
                gates: Some(vec![crate::project::Gate {
                    command: "full".into(),
                    paths: Some(vec!["srrc/**".into()]),
                    env: Default::default(),
                }]),
                ..Default::default()
            }],
        )
        .unwrap();
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.program == "git",
            |cmd| crate::runner::RealRunner.run(cmd),
        );
        let (text, healthy) = report(
            &env,
            &root,
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(!healthy);
        assert!(
            text.contains("`srrc/**` matches no tracked files"),
            "{text}"
        );
    }

    #[test]
    fn a_repo_row_with_a_box_path_brings_its_machine_in() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        let config = home.path().join("cfg");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            "[routing]\ndefault = \"pi_codex_sol_high\"\nretries = 1\n\n[dispatch]\nmachine = \"dispatch-box\"\n",
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
        write_machine_config(&home.path().join("cfg"));
        let machines = r#"[{"id":"buildbox-id","label":"buildbox","target":"me@box","session":"default","enabled":true}]"#;
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
            text.contains("[FAIL] box buildbox: unreachable: ssh: connect timed out"),
            "{text}"
        );
    }

    #[test]
    fn ssh_answering_but_snapshot_stalling_reports_its_last_part() {
        let config = machine_config(&["pi"]);
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            crate::runner::Output {
                timed_out: true,
                stdout:
                    "doctor_active\tfacts\ndoctor_phase_facts\t120\ndoctor_active\therdr tabs\n"
                        .into(),
                ..Default::default()
            },
        );
        let rows = box_rows(
            &runner,
            "herdr",
            config.path(),
            &box_profile(),
            &default_recipes(),
            12.0,
        );
        assert_eq!(
            rows[0].2,
            "slow: snapshot timed out while running herdr tabs"
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
        let config = machine_config(&["pi"]);
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&box_facts().replace("login_codex\tok", "login_codex\tmissing")),
        );
        runner.on(
            "pane read",
            ok("@@pi /home/agent/.local/bin/pi\n@@cmd\n/bin/cargo\n/bin/just\n/bin/node\n@@done\n"),
        );
        probe_fakes(&runner);
        let mut recipes = default_recipes();
        recipes.retain(|_, recipe| recipe.kind == "pi" && recipe.provider == "openai-codex");
        let rows = box_rows(
            &runner,
            "herdr",
            config.path(),
            &box_profile(),
            &recipes,
            12.0,
        );
        assert!(rows.iter().all(|row| row.0 == Some(true)), "{rows:?}");
        assert_eq!(
            rows.iter()
                .filter(|row| row.1.starts_with("box buildbox pi openai-codex/"))
                .count(),
            1
        );
        assert!(!rows.iter().any(|row| row.1.contains(" login ")));
        let calls = runner.calls.borrow();
        let ssh = calls
            .iter()
            .find(|call| call.program == "ssh")
            .unwrap()
            .display();
        assert_eq!(ssh.matches("check openai-codex").count(), 1, "{ssh}");
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
                ok(&box_facts()
                    .replace(
                        &format!("login_{kind}\tok"),
                        &format!("login_{kind}\tmissing"),
                    )
                    .replace(
                        &format!("pane_tool_{kind}\t/home/agent/.local/bin/{kind}"),
                        &format!("pane_tool_{kind}\t"),
                    )),
            );
            runner.on("pane read", ok("@@pi /home/agent/.local/bin/pi\n@@cmd\n/bin/cargo\n/bin/just\n/bin/node\n@@done\n"));
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
                .find(|row| row.1 == format!("box buildbox login {kind}"))
                .unwrap();
            assert_eq!(login.0, Some(false), "{rows:?}");
            let tools = rows
                .iter()
                .find(|row| row.1 == "box buildbox tools")
                .unwrap();
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
        let config = machine_config(&["pi"]);
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
                config.path(),
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
    fn unreachable_pi_disk_and_readiness_checks_are_not_provider_failures() {
        let config = machine_config(&["pi"]);
        let env = Env::for_test(config.path(), &[]);
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            fail(255, "ssh: connect to host box port 22: Operation timed out"),
        );
        let ctx = Ctx {
            env: &env,
            root: config.path().join("root"),
            config_dir: config.path().to_path_buf(),
            runner: &runner,
            detached_ticker: false,
        };
        let launch = crate::contracts::Launch {
            kind: "pi".into(),
            args: vec![
                "--provider".into(),
                "openai-codex".into(),
                "--model".into(),
                "gpt-5.4".into(),
            ],
            ..Default::default()
        };
        for error in [
            recipe_ready_on_box(&ctx, &box_profile(), &launch).unwrap_err(),
            check_start_disk(&ctx, Some(&box_profile()), None).unwrap_err(),
        ] {
            let message = format!("{error:#}");
            assert!(message.starts_with("unreachable:"), "{message}");
            assert!(!message.contains("pi_not_ready"));
            assert!(!message.contains("disk_low"));
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
                    .find(|row| row.1 == format!("box buildbox recipe {id}"))
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
            "pane_pi\t/home/agent/.local/bin/pi",
            "pane_tool_cargo\t/home/agent/.cargo/bin/cargo",
            "pane_tool_just\t/home/agent/.cargo/bin/just",
            "pane_tool_node\t/usr/local/bin/node",
            "pane_tool_claude\t/home/agent/.local/bin/claude",
            "pane_tool_codex\t/home/agent/.local/bin/codex",
            "pane_tool_agy\t/home/agent/.local/bin/agy",
            "herdr_workspaces\t{\"result\":{\"workspaces\":[]}}",
            "herdr_agents\t{\"result\":{\"agents\":[]}}",
            "herdr_tabs\t{\"result\":{\"tabs\":[]}}",
            "login_claude\tok",
            "login_codex\tok",
            "login_agy\tok",
            "repo /home/agent/projects/herdr\tok",
            "repo /home/agent/projects/herdr-ade\tok",
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
            ok(r#"{"result":{"root_pane":{"workspace_id":"w9","tab_id":"w9:t1","pane_id":"w9:p1","cwd":"/home/agent"}}}"#),
        );
        runner.on("pane run", ok(r#"{"result":{}}"#));
        runner.on(
            "pane read",
            ok("@@pi /home/agent/.local/bin/pi\n@@cmd\n/home/agent/.cargo/bin/cargo\n/home/agent/.cargo/bin/just\n/home/agent/.local/bin/claude\n/home/agent/.local/bin/codex\n/home/agent/.local/bin/agy\n/usr/local/bin/node\n@@done\n"),
        );
        runner.on("workspace close", ok(r#"{"result":{}}"#));
    }

    fn box_profile() -> crate::contracts::MachineProfile {
        crate::contracts::MachineProfile {
            id: "abc".into(),
            label: "buildbox".into(),
            target: "me@box".into(),
            session: "default".into(),
        }
    }

    #[test]
    fn box_rows_obey_machine_kinds_and_gate_on_free_disk() {
        let config = machine_config(&["pi"]);
        let runner = FakeRunner::new();
        runner.on("ssh", ok(&box_facts()));
        probe_fakes(&runner);
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
                .map(|(ok, _, detail)| (*ok, detail.clone()))
                .unwrap_or_else(|| panic!("no row {label}"))
        };
        assert_eq!(find("box buildbox boot").0, Some(true));
        assert_eq!(find("box buildbox wrapper").0, Some(true));
        assert!(
            find("box buildbox wrapper")
                .1
                .contains("/home/agent/.local/bin/pi")
        );
        assert_eq!(find("box buildbox tools").0, Some(true));
        assert_eq!(
            find("box buildbox repo /home/agent/projects/herdr").0,
            Some(true)
        );
        assert_eq!(find("box buildbox capacity").0, Some(true));
        assert!(
            find("box buildbox capacity")
                .1
                .contains("fails below 12 GB")
        );
        let configured = box_rows(
            &runner,
            "herdr",
            config.path(),
            &box_profile(),
            &default_recipes(),
            120.0,
        );
        assert_eq!(
            configured
                .iter()
                .find(|row| row.1 == "box buildbox capacity")
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
        assert_eq!(find_row(&runner, "box buildbox capacity").0, Some(false));

        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&box_facts().replace("df_free\t100000000000", "df_free\tunknown")),
        );
        probe_fakes(&runner);
        let capacity = find_row(&runner, "box buildbox capacity");
        assert_eq!(capacity.0, None);
        assert!(capacity.2.contains("unknown GB disk free"), "{capacity:?}");

        let runner = FakeRunner::new();
        runner.on("ssh", fail(255, "ssh: connect timed out"));
        let rows = box_rows(
            &runner,
            "herdr",
            config.path(),
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
            .replace(
                "pane_tool_agy\t/home/agent/.local/bin/agy",
                "pane_tool_agy\t",
            )
            .replace(
                "pane_tool_claude\t/home/agent/.local/bin/claude",
                "pane_tool_claude\t",
            );
        runner.on("ssh", ok(&facts));
        runner.on(
            "workspace create",
            ok(r#"{"result":{"root_pane":{"workspace_id":"w9","tab_id":"w9:t1","pane_id":"w9:p1","cwd":"/home/agent"}}}"#),
        );
        runner.on("pane run", ok(r#"{"result":{}}"#));
        runner.on(
            "pane read",
            ok("@@pi /home/agent/.local/bin/pi\n@@cmd\n/home/agent/.cargo/bin/cargo\n/home/agent/.cargo/bin/just\n/usr/local/bin/node\n@@done\n"),
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
        assert_eq!(find("box buildbox login agy").0, Some(false));
        assert_eq!(find("box buildbox tools").0, Some(false));
        assert!(find("box buildbox tools").2.contains("agy claude"));
        assert!(!find("box buildbox tools").2.contains("codex"));
    }

    #[test]
    fn box_wrapper_probe_fails_closed_when_the_path_resolves_another_binary() {
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&box_facts().replace(
                "pane_pi\t/home/agent/.local/bin/pi",
                "pane_pi\t/usr/local/bin/pi",
            )),
        );
        let row = find_row(&runner, "box buildbox wrapper");
        assert_eq!(row.0, Some(false));
        assert!(row.2.contains("/usr/local/bin/pi"), "{}", row.2);
    }

    fn find_row(runner: &FakeRunner, label: &str) -> (Option<bool>, String, String) {
        let config = machine_config(&["pi"]);
        box_rows(
            runner,
            "herdr",
            config.path(),
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
