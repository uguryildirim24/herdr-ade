//! `doctor`: readiness and retained bindings for ADE-owned work.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::herdr::{self, Herdr};
use crate::paths::{self, Ctx, Env, SessionFlags};
use crate::project;
use crate::runner::{Cmd, Output, Runner};

const TOOL_TIMEOUT: Duration = Duration::from_secs(10);

/// Instrument the same Runner used by all doctor dependencies, including the
/// box SSH transport. Never print arguments: they can contain credentials.
/// The program, duration and check locate an expensive call.
struct Timings<'a> {
    inner: &'a dyn Runner,
    state: Mutex<TimingState>,
}

struct TimingState {
    last: Instant,
    commands: Vec<String>,
    rows: Vec<String>,
}

impl<'a> Timings<'a> {
    fn new(inner: &'a dyn Runner) -> Self {
        Self {
            inner,
            state: Mutex::new(TimingState {
                last: Instant::now(),
                commands: Vec::new(),
                rows: Vec::new(),
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

    fn print(&self, text: &mut String) {
        let state = self.state.lock().unwrap();
        let _ = writeln!(
            text,
            "\nTimings (wall time since previous row; batched checks have a setup row):"
        );
        for row in &state.rows {
            let _ = writeln!(text, "{row}");
        }
    }

    fn command(&self, program: &str, elapsed: Duration) {
        let mut state = self.state.lock().unwrap();
        state
            .commands
            .push(format!("{program}: {:.3}s", elapsed.as_secs_f64()));
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

    fn capture(
        &self,
        cmd: &Cmd,
        logs: Option<&crate::runner::OutputLogs>,
    ) -> Result<crate::runner::Capture> {
        let start = Instant::now();
        let result = self.inner.capture(cmd, logs);
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

/// The terminal command and plugin action publish the same checks and exit status.
pub(crate) fn finish(ctx: &Ctx, result: &DoctorOutcome) -> Result<()> {
    crate::output::success(
        Some(if result.healthy {
            "healthy"
        } else {
            "unhealthy"
        }),
        result,
        &result.message,
        "",
    )?;
    if !result.healthy {
        return Err(crate::refusal::error(
            "some checks failed",
            format!(
                "{} doctor --timings (inspect failed checks, fix them, then rerun)",
                crate::coordinator::current_prefix(&ctx.root)?
            ),
        ));
    }
    Ok(())
}

/// One agent runtime's existing doctor probe. Recipes identify the runtime by
/// `kind`; placement never carries a separate allowlist of recipe ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NativeProbe {
    kind: String,
    cache_key: String,
    program: String,
    args: Vec<String>,
}

/// Selected inputs only: no routing policy or credential store crosses SSH.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct ProbePlan {
    natives: Vec<(String, NativeProbe, u64)>,
    models: Vec<(String, String)>,
    pi_ids: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    boundaries: Vec<(String, String, String)>,
    #[serde(default)]
    publication_target: Option<(String, String)>,
    #[serde(default)]
    execution_tools_only: bool,
    disk: Option<(String, f64)>,
    snapshot: Option<SnapshotInput>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotInput {
    home: String,
    build: String,
    session: String,
    repos: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct ProbeReport {
    rows: Vec<crate::pi::doctor::Row>,
    snapshot: SnapshotObservation,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SnapshotObservation {
    panes: Option<Vec<herdr::Pane>>,
    agents: Option<Vec<herdr::Agent>>,
    builds: Option<Vec<String>>,
    build_error: Option<String>,
}

impl ProbePlan {
    fn launch(ctx: &Ctx, launch: &crate::contracts::Launch) -> Result<Self> {
        let adapter = crate::adapters::declaration(&ctx.config_dir, &launch.kind)?;
        if adapter.doctor.readiness == "pi" {
            let provider = crate::pi::launch::flag_value(&launch.args, "--provider")
                .context("pi_args_forbidden: a provider launch names no --provider")?;
            let model = crate::pi::launch::flag_value(&launch.args, "--model")
                .context("pi_args_forbidden: a provider launch names no --model")?;
            // Publication uses the repository selected at terminal binding,
            // not another lane's card during provider readiness.
            let mut plan = Self {
                execution_tools_only: true,
                ..Default::default()
            };
            if !launch.recipe_id.is_empty() {
                plan.pi_ids.insert(
                    format!("provider {provider}/{model}"),
                    vec![launch.recipe_id.clone()],
                );
            }
            plan.models.push((provider, model));
            if crate::launch::execution_requested(launch) {
                plan.boundaries.push((
                    launch.recipe_id.clone(),
                    crate::launch::EXECUTION_BACKEND.into(),
                    crate::launch::execution_network(launch).into(),
                ));
            }
            return Ok(plan);
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
        Ok(Self {
            natives: vec![(
                if launch.recipe_id.is_empty() {
                    launch.kind.clone()
                } else {
                    launch.recipe_id.clone()
                },
                probe,
                launch.ready_timeout_ms.max(1_000),
            )],
            ..Default::default()
        })
    }
}

fn selected_plan(
    config: &crate::launch::LaunchConfig,
    machine: Option<&crate::remote::MachineDeclaration>,
) -> Result<ProbePlan> {
    let mut plan = ProbePlan::default();
    for (id, recipe) in &config.recipes {
        if recipe.enabled && machine.is_none_or(|m| m.runs_kind(&recipe.kind)) {
            let adapter = config
                .adapters
                .get(&recipe.kind)
                .with_context(|| format!("no adapter exists for agent kind `{}`", recipe.kind))?;
            plan.boundaries.push((
                id.clone(),
                crate::adapters::recipe_execution(adapter, recipe).into(),
                recipe.network.clone(),
            ));
        }
    }
    for id in config.routing.recipe_ids() {
        let recipe = config
            .recipes
            .get(id)
            .with_context(|| format!("recipe `{id}` is missing"))?;
        if machine.is_some_and(|machine| !machine.runs_kind(&recipe.kind)) {
            continue;
        }
        let adapter = config
            .adapters
            .get(&recipe.kind)
            .with_context(|| format!("no adapter exists for agent kind `{}`", recipe.kind))?;
        crate::adapters::validate_recipe(adapter, id, recipe)?;
        if adapter.doctor.readiness == "pi" {
            crate::pi::launch::validate_provider_column(&recipe.provider, &recipe.args)?;
            let model = crate::pi::launch::flag_value(&recipe.args, "--model")
                .context("pi recipe names no model")?;
            let label = format!("provider {}/{}", recipe.provider, model);
            plan.pi_ids.entry(label).or_default().push(id.to_string());
            let pair = (recipe.provider.clone(), model);
            if !plan.models.contains(&pair) {
                plan.models.push(pair);
            }
        } else {
            let probe = native_probe(adapter, recipe).with_context(|| {
                format!(
                    "no doctor readiness probe exists for agent kind `{}`",
                    recipe.kind
                )
            })?;
            let timeout = if recipe.ready_timeout_ms == 0 {
                adapter.ready_timeout_ms
            } else {
                recipe.ready_timeout_ms
            };
            plan.natives
                .push((id.to_string(), probe, timeout.max(1_000)));
        }
    }
    Ok(plan)
}

fn pi_label(plan: &ProbePlan, label: &str) -> String {
    plan.pi_ids.get(label).map_or_else(
        || label.to_string(),
        |ids| format!("recipe {} ({label})", ids.join(", ")),
    )
}

fn progress(phase: &str, rows: &[crate::pi::doctor::Row]) {
    if std::env::var_os("HERDR_ADE_BOX_INPUT").is_some() {
        crate::output::write_stdout(format_args!(
            "{}\n",
            serde_json::json!({"active": phase, "observations": rows})
        ));
    }
}

/// Executed unchanged at home and on a saved machine. SSH only transports it.
pub(crate) fn execute_plan(ctx: &Ctx, plan: &ProbePlan) -> Result<ProbeReport> {
    use crate::pi::doctor::Row;
    let mut report = ProbeReport::default();
    for (id, backend, network) in &plan.boundaries {
        report.rows.push(execution_row_with_publication(
            ctx,
            id,
            backend,
            network,
            plan.publication_target.as_ref(),
            plan.execution_tools_only,
        ));
    }
    progress("disk", &report.rows);
    if let Some((path, floor)) = &plan.disk {
        report.rows.push(disk_row(ctx.runner, path, *floor));
    }
    progress("readiness", &report.rows);
    report.rows.extend(run_native_probes(ctx, &plan.natives));
    if !plan.models.is_empty() {
        let env = crate::pi::Env::from_process()?;
        let rooted = crate::runner::CwdRunner::new(ctx.runner, &ctx.root);
        let models: Vec<_> = plan
            .models
            .iter()
            .map(|(p, m)| (p.as_str(), m.as_str()))
            .collect();
        report.rows.extend(
            crate::pi::doctor::doctor_rows_with_models(
                &env,
                &crate::pi_ade::layout(&ctx.root),
                &rooted,
                &models,
            )
            .into_iter()
            .map(|mut row| {
                row.label = pi_label(plan, &row.label);
                row
            }),
        );
    }
    if let Some(input) = &plan.snapshot {
        progress("facts", &report.rows);
        let bin = ctx.env.herdr_bin();
        report.rows.push(
            match ctx
                .runner
                .run(&Cmd::new(&bin, TOOL_TIMEOUT).arg("--version"))
            {
                Ok(output) if output.success() => Row::ok("server", output.stdout.trim()),
                Ok(output) => Row::warn("server", output.error_text()),
                Err(error) => Row::warn("server", format!("{error:#}")),
            },
        );
        for path in &input.repos {
            report.rows.push(if Path::new(path).join(".git").is_dir() {
                Row::ok(format!("repo {path}"), "clone ok")
            } else {
                Row::fail(format!("repo {path}"), "clone missing")
            });
        }
        let rules = Path::new(&input.home).join(".config/herdr-ade/RULES.md");
        report.rows.push(match std::fs::read(rules) {
            Ok(bytes) => Row::ok(
                "rules",
                format!("RULES.md sha256 {}", crate::thread::sha256_hex(&bytes)),
            ),
            Err(_) => Row::warn("rules", "no generated RULES.md recorded"),
        });
        progress("herdr", &report.rows);
        report.snapshot.panes =
            observe_list(ctx.runner, &bin, "pane", "panes", &input.session).ok();
        report.snapshot.agents =
            observe_list(ctx.runner, &bin, "agent", "agents", &input.session).ok();
        progress("builds", &report.rows);
        match build_folders(&input.build) {
            Ok(builds) => report.snapshot.builds = Some(builds),
            Err(error) => {
                report.snapshot.build_error = Some(format!(
                    "could not list build folders under {}: {error:#}",
                    input.build
                ))
            }
        }
    }
    Ok(report)
}

pub(crate) fn observe_list<T: serde::de::DeserializeOwned>(
    runner: &dyn Runner,
    bin: &str,
    verb: &str,
    field: &str,
    session: &str,
) -> Result<Vec<T>> {
    let out = runner.run(
        &Cmd::new(bin, TOOL_TIMEOUT)
            .own_group()
            .env("HERDR_SESSION", session)
            .args([verb, "list"]),
    )?;
    anyhow::ensure!(out.success(), "{}", out.error_text());
    let value: serde_json::Value = serde_json::from_str(&out.stdout)?;
    Ok(serde_json::from_value(value["result"][field].clone())?)
}

fn build_folders(root: &str) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut folders = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            folders.push(entry.path().to_string_lossy().into_owned());
        }
    }
    folders.sort();
    Ok(folders)
}

fn remote_plan(
    runner: &dyn Runner,
    profile: &crate::contracts::MachineProfile,
    machine: &crate::remote::MachineDeclaration,
    plan: &ProbePlan,
) -> Result<ProbeReport> {
    // Pi has a 10s auth check plus a 30s live call, and setup/tool checks.
    let timeout = Duration::from_secs(150)
        + Duration::from_millis(plan.natives.iter().map(|(_, _, ms)| *ms).max().unwrap_or(0));
    let report: ProbeReport = crate::box_helper::call(
        runner,
        &profile.target,
        machine,
        crate::box_helper::Request::Doctor(Box::new(serde_json::from_value(
            serde_json::to_value(plan)?,
        )?)),
        timeout,
        None,
    )?;
    let mut expected: Vec<String> = plan
        .natives
        .iter()
        .map(|(id, _, _)| format!("recipe {id}"))
        .collect();
    expected.extend(
        plan.models
            .iter()
            .map(|(p, m)| pi_label(plan, &format!("provider {p}/{m}"))),
    );
    if plan.disk.is_some() {
        expected.push("disk".into());
    }
    expected.extend(
        plan.boundaries
            .iter()
            .map(|(id, _, _)| format!("recipe {id} execution")),
    );
    for label in expected {
        anyhow::ensure!(
            report.rows.iter().filter(|row| row.label == label).count() == 1,
            "readiness unknown: machine returned no unique observation for {label}"
        );
    }
    Ok(report)
}

/// Probe namespaces on the execution machine, never infer enforcement from
/// an installed binary or a permission flag. No silent host fallback.
pub(crate) fn execution_probe_command(network: &str) -> Cmd {
    let mut cmd = Cmd::new("/usr/bin/bwrap", TOOL_TIMEOUT).args([
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--cap-drop",
        "ALL",
        "--clearenv",
        "--ro-bind",
        "/usr",
        "/usr",
        "--ro-bind",
        "/lib",
        "/lib",
        "--ro-bind-try",
        "/lib64",
        "/lib64",
        "/usr/bin/true",
    ]);
    if network == "allowed" {
        cmd.args.insert(1, "--share-net".into());
    }
    cmd
}

#[cfg(test)]
pub(crate) fn pi_execution_fixture() -> Output {
    let tools: Vec<_> = ["bash", "read", "write", "edit", "ade"].iter().map(|name|
        serde_json::json!({"name": name, "source": {"path": "/runtime/ade-boundary-probe.mjs"}})
    ).collect();
    crate::runner::fake::ok(&format!(
        "ADE_BOUNDARY_TOOLS={}\nADE_AUTHORIZED_PUBLICATION=passed\n",
        serde_json::to_string(&tools).unwrap()
    ))
}

/// Use the doctor's actual probe on the target, not presence/OS heuristics.
pub(crate) fn lane_execution(
    ctx: &Ctx,
    machine: Option<&crate::remote::MachineDeclaration>,
    network: &str,
    publication_target: Option<(String, String)>,
) -> Result<crate::pi::doctor::Row> {
    let plan = ProbePlan {
        boundaries: vec![(
            "lane".into(),
            crate::launch::EXECUTION_BACKEND.into(),
            network.into(),
        )],
        // Local seals do not publish. Remote bindings supply their exact
        // trusted target before the first lane card has been provisioned.
        execution_tools_only: machine.is_none() && publication_target.is_none(),
        publication_target,
        ..Default::default()
    };
    let report = if let Some(machine) = machine {
        remote_plan(
            ctx.runner,
            &crate::contracts::MachineProfile {
                target: machine.target.clone(),
                ..Default::default()
            },
            machine,
            &plan,
        )?
    } else {
        execute_plan(ctx, &plan)?
    };
    report
        .rows
        .into_iter()
        .find(|row| row.label == "recipe lane execution")
        .context("execution_boundary_unavailable: no namespace observation")
}

/// Execute the installed Pi CLI, not the SDK: the CLI's allowlist semantics
/// caused D40. No model/provider request or namespace operation is needed here.
#[cfg(test)]
pub(crate) fn pi_execution_probe_command(root: &Path) -> Cmd {
    pi_probe_command(root, false, None)
}

/// Uses a trusted host lane card, never a target chosen by lane tools. A
/// machine without an authorized publication target remains advisory.
pub(crate) fn pi_authorized_publication_probe_command(root: &Path) -> Cmd {
    pi_probe_command(root, true, None)
}

pub(crate) fn pi_publication_probe_for(root: &Path, repo: &str, url: &str) -> Cmd {
    pi_probe_command(root, true, Some((repo.to_string(), url.to_string())))
}

fn pi_probe_command(root: &Path, publication: bool, target: Option<(String, String)>) -> Cmd {
    let target = target.or_else(|| {
        crate::project::list_slugs(root)
            .into_iter()
            .find_map(|slug| {
                let cards = root.join(slug).join(".state/lanes");
                std::fs::read_dir(cards)
                    .ok()?
                    .filter_map(|entry| entry.ok())
                    .find_map(|entry| {
                        let card: crate::contracts::LaneCard =
                            toml::from_str(&std::fs::read_to_string(entry.path()).ok()?).ok()?;
                        (Path::new(&card.box_repo).join(".git").exists()
                            && !card.publish_url.is_empty())
                        .then_some((card.box_repo, card.publish_url))
                    })
            })
    });
    let root = root.display().to_string();
    let policy = serde_json::json!({"root": root, "ade": "/usr/bin/true", "cwd": target.as_ref().map_or("/tmp", |t| t.0.as_str()), "publication": target.as_ref().map(|t| &t.1), "branch": "probe", "role": "lane", "state": "/tmp", "network": "denied"});
    let source = include_str!("../assets/pi-execution-boundary.mjs")
        .replace("__ADE_EXECUTION_POLICY__", &policy.to_string());
    let args =
        crate::launch::bounded_pi_args(&root, "__STATE__", "__STATE__/ade-boundary-probe.mjs")
            .iter()
            .map(|arg| match arg.strip_prefix("__STATE__") {
                Some(suffix) => format!("\"$state\"{}", crate::remote::quote(suffix)),
                None => crate::remote::quote(arg),
            })
            .collect::<Vec<_>>()
            .join(" ");
    let script = format!(
        "set -eu\nmkdir -p {runtime}\nstate=$(mktemp -d {template})\ntrap 'rm -rf -- \"$state\"' EXIT\ncat > \"$state/ade-boundary-probe.mjs\"\npi --no-skills {args} --mode rpc --ade-execution-probe {publication_flag} </dev/null\n",
        publication_flag = if publication {
            "--ade-publication-probe"
        } else {
            ""
        },
        runtime = crate::remote::quote(&format!("{root}/.execution")),
        template = crate::remote::quote(&format!("{root}/.execution/probe.XXXXXX")),
    );
    Cmd::new("bash", Duration::from_secs(30))
        .args(["-c", &script])
        .env("PI_OFFLINE", "1")
        .stdin(source)
        .cwd("/tmp")
        .own_group()
}

pub(crate) fn pi_authorized_publication_probe_result(output: &Output) -> Result<()> {
    pi_execution_probe_result(output)?;
    anyhow::ensure!(
        output
            .stdout
            .lines()
            .chain(output.stderr.lines())
            .any(|line| line == "ADE_AUTHORIZED_PUBLICATION=passed"),
        "bounded authorized publication dry run failed or missing: {}",
        output.error_text()
    );
    Ok(())
}

pub(crate) fn pi_execution_probe_result(output: &Output) -> Result<()> {
    let expected = ["bash", "read", "write", "edit", "ade"];
    let tools: Vec<serde_json::Value> = output
        .stdout
        .lines()
        .chain(output.stderr.lines())
        .find_map(|line| line.strip_prefix("ADE_BOUNDARY_TOOLS="))
        .and_then(|line| serde_json::from_str(line).ok())
        .unwrap_or_default();
    let missing: Vec<_> = expected
        .iter()
        .filter(|name| {
            !tools.iter().any(|tool| {
                tool["name"].as_str() == Some(**name)
                    && tool["source"]["path"]
                        .as_str()
                        .is_some_and(|path| path.ends_with("/ade-boundary-probe.mjs"))
            })
        })
        .copied()
        .collect();
    anyhow::ensure!(
        missing.is_empty(),
        "bounded Pi CLI missing boundary tools: {}; {}",
        missing.join(", "),
        output.error_text()
    );
    anyhow::ensure!(
        output.success() && tools.len() == expected.len(),
        "bounded Pi CLI exposed unexpected tools or failed: {}",
        output.error_text()
    );
    anyhow::ensure!(
        !output.stderr.contains("Failed to load extension"),
        "bounded Pi CLI extension load error: {}",
        output.stderr
    );
    Ok(())
}

fn execution_row(ctx: &Ctx, id: &str, backend: &str, network: &str) -> crate::pi::doctor::Row {
    execution_row_with_publication(ctx, id, backend, network, None, false)
}

fn execution_row_with_publication(
    ctx: &Ctx,
    id: &str,
    backend: &str,
    network: &str,
    publication: Option<&(String, String)>,
    tools_only: bool,
) -> crate::pi::doctor::Row {
    use crate::pi::doctor::Row;
    let label = format!("recipe {id} execution");
    let detail = crate::launch::execution_description(backend, std::env::consts::OS, network);
    if backend != crate::launch::EXECUTION_BACKEND || !cfg!(target_os = "linux") {
        return Row::warn(label, detail);
    }
    let output = ctx.runner.run(&execution_probe_command(network));
    match output {
        Ok(output) if output.success() => {
            let command = if tools_only {
                pi_probe_command(&ctx.root, false, None)
            } else if let Some((repo, url)) = publication {
                pi_publication_probe_for(&ctx.root, repo, url)
            } else {
                pi_authorized_publication_probe_command(&ctx.root)
            };
            let probe = ctx.runner.run(&command).and_then(|output| {
                if tools_only {
                    pi_execution_probe_result(&output)
                } else {
                    pi_authorized_publication_probe_result(&output)
                }
            });
            match probe {
                Ok(()) => Row::ok(
                    label,
                    detail.replace(
                        "(availability probed separately)",
                        if tools_only {
                            "(namespace and bounded Pi CLI tool probes passed)"
                        } else {
                            "(namespace, bounded Pi CLI tool and authorized publication probes passed)"
                        },
                    ),
                ),
                Err(error) => Row::fail(
                    label,
                    format!(
                        "advisory: execution_boundary_unavailable: {error:#}; new lanes use advisory execution; already bounded launches fail closed"
                    ),
                ),
            }
        }
        Ok(output) => Row::fail(
            label,
            format!(
                "advisory: execution_boundary_unavailable: {}; new lanes use advisory execution; already bounded launches fail closed",
                output.error_text()
            ),
        ),
        Err(error) => Row::fail(
            label,
            format!(
                "advisory: execution_boundary_unavailable: {error:#}; new lanes use advisory execution; already bounded launches fail closed"
            ),
        ),
    }
}

#[cfg(test)]
pub(crate) fn boundary_diagnostic_output(cmd: &Cmd, free_kb: u64, refusal: Option<&str>) -> Output {
    let mut output = crate::testkit::diagnostic_output(cmd, free_kb, refusal);
    let input: serde_json::Value = serde_json::from_str(cmd.stdin.as_deref().unwrap()).unwrap();
    let mut reply: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    if let Some(boundaries) = input["request"]["Doctor"]["boundaries"].as_array() {
        for boundary in boundaries {
            reply["result"]["rows"].as_array_mut().unwrap().push(
                serde_json::to_value(crate::pi::doctor::Row::ok(
                    format!("recipe {} execution", boundary[0].as_str().unwrap()),
                    "fixture namespace probe passed",
                ))
                .unwrap(),
            );
        }
    }
    output.stdout = serde_json::to_string(&reply).unwrap();
    output
}

fn require_ready(report: &ProbeReport) -> Result<()> {
    anyhow::ensure!(
        !report.rows.is_empty(),
        "readiness unknown: no observations returned"
    );
    let failures: Vec<_> = report
        .rows
        .iter()
        .filter(|row| {
            row.level == crate::pi::doctor::Level::Fail
                || (row.label == "disk" && row.level != crate::pi::doctor::Level::Ok)
        })
        .collect();
    if !failures.is_empty() {
        return Err(crate::pi_ade::ReadinessError {
            class: match crate::pi::doctor::failure_evidence(failures.iter().copied()) {
                crate::pi::doctor::FailureEvidence::Provider => {
                    crate::contracts::FailureClass::Provider
                }
                crate::pi::doctor::FailureEvidence::Unknown => {
                    crate::contracts::FailureClass::Unknown
                }
            },
            message: failures
                .iter()
                .map(|row| {
                    if row.label == "disk" {
                        row.detail.clone()
                    } else {
                        format!("{}: {}", row.label, row.detail)
                    }
                })
                .collect::<Vec<_>>()
                .join("; "),
        }
        .into());
    }
    Ok(())
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
    let digest =
        crate::thread::sha256_hex(format!("{}\0{}", adapter.binary, args.join("\0")).as_bytes());
    Some(NativeProbe {
        kind: recipe.kind.clone(),
        cache_key: format!("{}-{}", recipe.kind, &digest[..12]),
        program: adapter.binary.clone(),
        args,
    })
}

fn probe_error(kind: &str, output: &crate::runner::Output) -> String {
    let detail = output.error_text();
    if output.timed_out || output.code.is_none() {
        return format!(
            "{kind} readiness probe timed out or was terminated; provider status is unknown"
        );
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

fn run_native_probes(
    ctx: &Ctx,
    inputs: &[(String, NativeProbe, u64)],
) -> Vec<crate::pi::doctor::Row> {
    use crate::pi::doctor::{FailureEvidence, Row, cache_row, cached_row};
    let caches: Vec<_> = inputs
        .iter()
        .map(|(_, probe, _)| {
            ctx.root
                .join(".readiness")
                .join(format!("native-{}.json", probe.cache_key))
        })
        .collect();
    let mut outputs: Vec<_> = inputs
        .iter()
        .zip(&caches)
        .map(|((id, _, _), cache)| cached_row(cache, format!("recipe {id}")))
        .collect();
    let pending: Vec<_> = outputs
        .iter()
        .enumerate()
        .filter_map(|(i, output)| output.is_none().then_some(i))
        .collect();
    let commands: Vec<_> = pending
        .iter()
        .map(|&i| {
            let (_, probe, timeout) = &inputs[i];
            Cmd::new(&probe.program, Duration::from_millis(*timeout))
                .args(probe.args.iter().map(String::as_str))
                .cwd(&ctx.root)
                .own_group()
        })
        .collect();
    for (i, output) in pending.into_iter().zip(ctx.runner.run_parallel(&commands)) {
        let (id, probe, _) = &inputs[i];
        let label = format!("recipe {id}");
        let row = match output {
            Ok(output) if output.success() => {
                Row::ok(label, format!("{} reached its selected model", probe.kind))
            }
            Ok(output) => {
                let mut row = Row::fail(label, probe_error(&probe.kind, &output));
                if !output.timed_out
                    && output.code.is_some()
                    && crate::pi::doctor::positive_sign_in_evidence(&output.error_text())
                {
                    row.evidence = FailureEvidence::Provider;
                }
                row
            }
            Err(error) => Row::fail(
                label,
                format!("{} readiness could not run: {error:#}", probe.program),
            ),
        };
        if ctx.root.is_dir() {
            cache_row(&caches[i], row.clone());
        }
        outputs[i] = Some(row);
    }
    outputs
        .into_iter()
        .map(|output| output.expect("probe result"))
        .collect()
}

/// Whether the chosen recipe can run on this Mac. This is the same provider
/// or login probe the doctor owns, not a placement-specific capability table.
pub(crate) fn recipe_ready_local(ctx: &Ctx, launch: &crate::contracts::Launch) -> Result<()> {
    crate::adapters::dependency_ready(&ctx.root, crate::contracts::MACHINE_LOCAL, launch, || {
        recipe_ready_local_probe(ctx, launch)
    })
}

fn require_launch_ready(mut report: ProbeReport, launch: &crate::contracts::Launch) -> Result<()> {
    if !launch.args.iter().any(|arg| arg == "--no-extensions") {
        // A fresh launch chooses bounded/advisory at binding. Namespace failure
        // is still FAIL in doctor, but must not masquerade as provider failure
        // or move a ready lane away from its requested machine.
        let label = format!("recipe {} execution", launch.recipe_id);
        report.rows.retain(|row| row.label != label);
    }
    require_ready(&report)
}

fn recipe_ready_local_probe(ctx: &Ctx, launch: &crate::contracts::Launch) -> Result<()> {
    require_launch_ready(execute_plan(ctx, &ProbePlan::launch(ctx, launch)?)?, launch)
}

/// Whether the chosen recipe can run on a saved box. SSH injects the exact
/// lane PATH, so the selected probes measure the environment a fresh lane
/// receives. The target runs the same plan executor as local doctor.
pub(crate) fn recipe_ready_on_box(
    ctx: &Ctx,
    profile: &crate::contracts::MachineProfile,
    launch: &crate::contracts::Launch,
) -> Result<()> {
    let machine = if profile.id.is_empty() {
        &profile.label
    } else {
        &profile.id
    };
    crate::adapters::dependency_ready(&ctx.root, machine, launch, || {
        recipe_ready_on_box_probe(ctx, profile, launch)
    })
}

fn recipe_ready_on_box_probe(
    ctx: &Ctx,
    profile: &crate::contracts::MachineProfile,
    launch: &crate::contracts::Launch,
) -> Result<()> {
    let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    let mut plan = ProbePlan::launch(ctx, launch)?;
    plan.disk = Some((
        machine.worktrees.clone(),
        crate::launch::doctor_config(&ctx.config_dir)?.min_free_disk_gb,
    ));
    let rooted = crate::runner::CwdRunner::new(ctx.runner, &ctx.root);
    require_launch_ready(remote_plan(&rooted, profile, &machine, &plan)?, launch)
}

/// Recheck deferred launches without creating a worktree, tab or pane.
pub(crate) fn check_start_disk(
    ctx: &Ctx,
    profile: Option<&crate::contracts::MachineProfile>,
    repo: Option<&str>,
) -> Result<()> {
    let floor = crate::launch::doctor_config(&ctx.config_dir)?.min_free_disk_gb;
    let report = if let Some(profile) = profile.filter(|profile| !profile.is_local()) {
        let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label)?;
        let rooted = crate::runner::CwdRunner::new(ctx.runner, &ctx.root);
        remote_plan(
            &rooted,
            profile,
            &machine,
            &ProbePlan {
                disk: Some((machine.worktrees.clone(), floor)),
                ..Default::default()
            },
        )?
    } else {
        execute_plan(
            ctx,
            &ProbePlan {
                disk: Some((repo.unwrap_or(".").to_string(), floor)),
                ..Default::default()
            },
        )?
    };
    require_ready(&report)
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
    let (mut text, healthy, checks) = report_with_checks(
        ctx.env,
        &ctx.root,
        &ctx.config_dir,
        session,
        ctx.runner,
        timings,
    );
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

fn report_with_checks(
    env: &Env,
    root: &Path,
    config_dir: &Path,
    session: &SessionFlags,
    runner: &dyn Runner,
    timings: Option<&Timings<'_>>,
) -> (String, bool, Vec<CheckResult>) {
    let worker = config_dir.join(crate::harness::BOX_WORKER_MARKER).is_file();
    let config = if worker {
        crate::launch::recipe_catalog(config_dir)
    } else {
        crate::launch::parse_launch_config(config_dir)
    };
    let repos = crate::harness::repos(config_dir);
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
        }
        Err(error) => check(&mut out, Some(false), "session", format!("{error:#}")),
    }

    // Git is part of ADE's branch/publish contract. SSH and provider tools
    // are exercised only by the selected placements and adapter probes below.
    match runner.run(&Cmd::new("git", TOOL_TIMEOUT).args(["--version"])) {
        Ok(output) if output.success() => {
            check(&mut out, Some(true), "git", output.stdout.trim().into())
        }
        Ok(output) => check(&mut out, Some(false), "git", output.error_text()),
        Err(error) => check(&mut out, Some(false), "git", format!("{error:#}")),
    }
    let doctor_config = match &config {
        Ok(config) => config.doctor.clone(),
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
    let disk = disk_row(
        runner,
        &root.display().to_string(),
        doctor_config.min_free_disk_gb,
    );
    check(
        &mut out,
        row_status(disk.level),
        "machine local disk",
        disk.detail,
    );

    let (slugs, discovery_errors) = project::list_slugs_with_errors(root);
    let projects: Projects = slugs
        .iter()
        .map(|slug| (slug.clone(), project::Project::load(root, slug)))
        .collect();
    if !discovery_errors.is_empty() {
        check(
            &mut out,
            Some(false),
            "root",
            discovery_errors
                .iter()
                .map(|error| format!("{error:#}"))
                .collect::<Vec<_>>()
                .join("; "),
        );
    } else if root.is_dir() {
        check(
            &mut out,
            Some(true),
            "root",
            format!("{} project(s)", slugs.len()),
        );
    } else {
        check(
            &mut out,
            None,
            "root",
            "does not exist yet; `new` creates it".into(),
        );
    }

    let (mut status, mut detail) = crate::ticker::health_report(root);
    if let crate::ticker::LockState::Held(info) = crate::ticker::lock_state(root) {
        let (folder_ok, folder_detail) = ticker_folder_check(&info);
        if folder_ok == Some(false) {
            status = Some(false);
        }
        detail.push_str(&format!("; {folder_detail}"));
    }
    check(&mut out, status, "ticker", detail);

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
                    "not loaded; run the harness install command"
                }
            ),
        );
    }

    if let Ok(repos) = &repos {
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
    check_bindings(
        &mut out,
        &mut check,
        &projects,
        ("local", "local"),
        (&bin, runner),
        None,
    );
    for (slug, project) in &projects {
        let Ok(project) = project else {
            continue;
        };
        let label = format!("project {slug}");
        if let Ok((settings, _)) = project.read_project_md() {
            for repo in &settings.repos {
                if let Some(machine) = &repo.review_machine {
                    let result = if machine == crate::contracts::MACHINE_LOCAL {
                        Ok(())
                    } else {
                        crate::remote::machine_declaration(config_dir, machine).map(|_| ())
                    };
                    check(
                        &mut out,
                        Some(result.is_ok()),
                        &format!("{label} repo {} review_machine", repo.path),
                        match result {
                            Ok(()) => format!("explicit review machine `{machine}`"),
                            Err(error) => {
                                format!("unknown or invalid review_machine `{machine}`: {error:#}")
                            }
                        },
                    );
                }
                if repo.machine.is_none()
                    && repo
                        .push_remote
                        .as_deref()
                        .is_none_or(|s| s.trim().is_empty())
                    && repo
                        .publish_url
                        .as_deref()
                        .is_none_or(|s| s.trim().is_empty())
                    && let Ok(remotes) = crate::repo::Git::new(runner, &repo.path).run(&["remote"])
                    && !remotes.trim().is_empty()
                {
                    let choices = remotes
                        .lines()
                        .map(|remote| format!("push_remote = {remote:?}"))
                        .collect::<Vec<_>>();
                    check(
                        &mut out,
                        None,
                        &format!("{label} repo {}", repo.path),
                        format!(
                            "no push target; remotes: {}; add {} to this [[repos]] row in {}{}",
                            remotes.lines().collect::<Vec<_>>().join(", "),
                            choices.join(" or "),
                            project.project_md().display(),
                            if choices.len() > 1 {
                                " (choose the integration destination)"
                            } else {
                                ""
                            }
                        ),
                    );
                }
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
        if let Some(warning) = crate::thread::memory_use(project).warning() {
            check(&mut out, None, &format!("{label} memory"), warning);
        }
        let record = match crate::ticker::coordinator_binding(project) {
            Ok(Some(record)) => record,
            Ok(None) => {
                check(
                    &mut out,
                    Some(true),
                    &label,
                    format!(
                        "{}; coordinator closed; reviews skipped until open",
                        project.status()
                    ),
                );
                continue;
            }
            Err(error) => {
                check(&mut out, Some(false), &label, format!("{error:#}"));
                continue;
            }
        };
        if !Path::new(&record.socket).exists() {
            check(
                &mut out,
                Some(false),
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
                Some(false),
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

    if worker {
        if let Ok(config) = &config {
            let ctx = Ctx {
                env,
                root: root.to_path_buf(),
                config_dir: config_dir.to_path_buf(),
                runner,
                detached_ticker: false,
            };
            for (id, recipe) in &config.recipes {
                if recipe.enabled
                    && let Some(adapter) = config.adapters.get(&recipe.kind)
                {
                    let row = execution_row(
                        &ctx,
                        id,
                        crate::adapters::recipe_execution(adapter, recipe),
                        &recipe.network,
                    );
                    check(&mut out, row_status(row.level), &row.label, row.detail);
                }
            }
        }
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
        match config
            .as_ref()
            .map_err(|error| anyhow::anyhow!("recipe selection unknown: {error:#}"))
            .and_then(|config| crate::launch::doctor_rows(&ctx, config))
        {
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
        let selected = config
            .as_ref()
            .map_err(|error| anyhow::anyhow!("pi recipe selection unknown: {error:#}"))
            .and_then(|config| selected_plan(config, None))
            .and_then(|plan| execute_plan(&ctx, &plan));
        match selected {
            Ok(report) => {
                for row in report.rows {
                    check(&mut out, row_status(row.level), &row.label, row.detail);
                }
            }
            Err(error) => check(&mut out, Some(false), "readiness", format!("{error:#}")),
        }
        match config
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error:#}"))
            .and_then(|config| {
                let repos = repos
                    .as_ref()
                    .map_err(|error| anyhow::anyhow!("{error:#}"))?;
                Ok((
                    machines_to_check(&projects, config_dir, runner, &bin, config, repos)?,
                    config,
                    repos,
                ))
            }) {
            Ok((machines, config, repos)) => {
                let mut checked_ids = BTreeSet::new();
                for (machine, profile) in machines {
                    match profile {
                        Ok(profile) if !checked_ids.insert(profile.id.clone()) => continue,
                        Ok(profile) if profile.is_local() => check(
                            &mut out,
                            Some(true),
                            &format!("machine {machine}"),
                            "on this machine".into(),
                        ),
                        Ok(profile) => {
                            check(
                                &mut out,
                                Some(true),
                                &format!("machine {} ({})", profile.id, profile.label),
                                format!("ssh target {}", profile.target),
                            );
                            let observed = probe_box(
                                runner,
                                config_dir,
                                &profile,
                                config,
                                config.doctor.min_free_disk_gb,
                                repos,
                            )
                            .unwrap_or_else(|error| {
                                check(
                                    &mut out,
                                    Some(false),
                                    &format!("box {}", profile.label),
                                    format!("{error:#}"),
                                );
                                ProbeReport::default()
                            });
                            let box_snapshot = observed.snapshot;
                            if let Some(timings) = timings {
                                timings.row(&format!("box {} snapshot result", profile.label));
                            }
                            for row in observed.rows {
                                check(
                                    &mut out,
                                    row_status(row.level),
                                    &format!("box {} {}", profile.label, row.label),
                                    row.detail,
                                );
                            }
                            check_bindings(
                                &mut out,
                                &mut check,
                                &projects,
                                (&profile.id, &profile.label),
                                (&bin, runner),
                                Some(&box_snapshot),
                            );
                            let (builds, errors) =
                                finished_build_folders_impl(&profile, &box_snapshot, &projects);
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

fn disk_row(runner: &dyn Runner, path: &str, floor: f64) -> crate::pi::doctor::Row {
    use crate::pi::doctor::Row;
    match local_free_disk_gb(runner, path) {
        Ok(free) if free >= floor => Row::ok("disk", disk_detail(free, floor)),
        Ok(free) => Row::fail(
            "disk",
            format!("disk_low: {} under {path}", disk_detail(free, floor)),
        ),
        Err(error) => Row::warn(
            "disk",
            format!("unreachable: disk free space unknown under {path}: {error:#}"),
        ),
    }
}

fn local_free_disk_gb(runner: &dyn Runner, path: &str) -> Result<f64> {
    let output = runner.run(&Cmd::new("df", TOOL_TIMEOUT).args(["-Pk", path]))?;
    if !output.success() {
        anyhow::bail!("{}", output.error_text());
    }
    let available_kb = output
        .stdout
        .lines()
        .rev()
        .find_map(|line| line.split_whitespace().nth(3)?.parse::<u64>().ok())
        .with_context(|| format!("`df -Pk {path}` did not report available blocks"))?;
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

type Projects = BTreeMap<String, Result<project::Project>>;

#[cfg(test)]
fn load_projects(root: &Path) -> Projects {
    project::list_slugs(root)
        .into_iter()
        .map(|slug| {
            let project = project::Project::load(root, &slug);
            (slug, project)
        })
        .collect()
}

#[cfg(test)]
fn finished_build_folders(
    ctx: &Ctx,
    profile: &crate::contracts::MachineProfile,
) -> (Vec<String>, Vec<String>) {
    let machine = crate::remote::machine_declaration(&ctx.config_dir, &profile.label).unwrap();
    let plan = ProbePlan {
        snapshot: Some(SnapshotInput {
            home: machine.home.clone(),
            build: machine.build.clone(),
            session: profile.session.clone(),
            repos: Vec::new(),
        }),
        ..Default::default()
    };
    match remote_plan(ctx.runner, profile, &machine, &plan) {
        Ok(report) => {
            finished_build_folders_impl(profile, &report.snapshot, &load_projects(&ctx.root))
        }
        Err(error) => (Vec::new(), vec![format!("{error:#}")]),
    }
}

fn finished_build_folders_impl(
    profile: &crate::contracts::MachineProfile,
    snapshot: &SnapshotObservation,
    projects: &Projects,
) -> (Vec<String>, Vec<String>) {
    let mut active = BTreeSet::new();
    let mut uncertain_projects = BTreeSet::new();
    let mut errors = Vec::new();
    for (slug, project) in projects {
        let project = match project {
            Ok(project) => project,
            Err(error) => {
                uncertain_projects.insert(slug.clone());
                errors.push(format!("{slug}: build ownership unknown: {error:#}"));
                continue;
            }
        };
        let (threads, unreadable) = crate::thread::list_with_errors(project);
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

    let Some(builds) = &snapshot.builds else {
        errors.push(
            snapshot
                .build_error
                .clone()
                .unwrap_or_else(|| "build folder observation unavailable".into()),
        );
        return (Vec::new(), errors);
    };
    let mut leftovers = Vec::new();
    for path in builds {
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
    (leftovers, errors)
}

#[cfg(test)]
fn check_lane_bindings(
    out: &mut String,
    check: &mut impl FnMut(&mut String, Option<bool>, &str, String),
    root: &Path,
    machine: (&str, &str),
    endpoint: (&str, &dyn Runner),
    snapshot: Option<&SnapshotObservation>,
) {
    check_bindings(
        out,
        check,
        &load_projects(root),
        machine,
        endpoint,
        snapshot,
    );
}

/// Records, not labels or an absent agent, establish ADE ownership.
fn check_bindings(
    out: &mut String,
    check: &mut impl FnMut(&mut String, Option<bool>, &str, String),
    projects: &Projects,
    machine: (&str, &str),
    endpoint: (&str, &dyn Runner),
    snapshot: Option<&SnapshotObservation>,
) {
    let (bin, runner) = endpoint;
    let (machine, machine_label) = machine;
    for (slug, project) in projects {
        let Ok(project) = project else {
            continue;
        };
        let (threads, errors) = crate::thread::list_with_errors(project);
        for error in errors {
            check(
                out,
                None,
                &format!("project {slug} lanes"),
                format!("ownership unknown: {error:#}"),
            );
        }
        let record = project.coordinator();
        let mut by_socket: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for lane in threads.iter().filter(|lane| {
            !lane.pane_id.is_empty()
                && if machine == "local" {
                    !lane.is_remote()
                } else {
                    lane.is_remote()
                        && (lane.machine_route() == machine
                            || (lane.machine_id.is_empty() && lane.machine == machine_label))
                }
        }) {
            // Historical lanes have no identity socket; the retained project
            // binding supplies it. A closed coordinator cannot hide modern lanes.
            let socket = if machine != "local" {
                String::new()
            } else if !lane.identity.socket.is_empty() {
                lane.identity.socket.clone()
            } else {
                record
                    .as_ref()
                    .map(|record| record.socket.clone())
                    .unwrap_or_default()
            };
            by_socket.entry(socket).or_default().push(lane);
        }
        for (socket, lanes) in by_socket {
            if machine == "local" && socket.is_empty() {
                check(
                    out,
                    None,
                    &format!("project {slug} lanes"),
                    "retained lane socket unavailable; bindings unknown".into(),
                );
                continue;
            }
            let herdr = Herdr::new(bin, socket, runner).on_machine(if machine == "local" {
                ""
            } else {
                machine
            });
            let panes = snapshot.map_or_else(
                || herdr.pane_list().map_err(anyhow::Error::from),
                |snapshot| {
                    snapshot
                        .panes
                        .clone()
                        .context("box pane observation unavailable")
                },
            );
            let agents = snapshot.map_or_else(
                || herdr.agent_list().map_err(anyhow::Error::from),
                |snapshot| {
                    snapshot
                        .agents
                        .clone()
                        .context("box agent observation unavailable")
                },
            );
            for lane in lanes {
                let label = format!("project {slug} lane {}", lane.id);
                let (Ok(panes), Ok(agents)) = (&panes, &agents) else {
                    check(
                        out,
                        None,
                        &label,
                        format!(
                            "binding unknown; panes: {:?}; agents: {:?}",
                            panes.as_ref().err(),
                            agents.as_ref().err()
                        ),
                    );
                    continue;
                };
                let pane = panes
                    .iter()
                    .any(|pane| crate::thread::pane_matches(lane, pane));
                let agent = agents
                    .iter()
                    .find(|agent| crate::thread::agent_matches(lane, agent));
                let (mut status, mut detail) = if lane
                    .retirement
                    .as_ref()
                    .is_some_and(|request| request.keep_pane)
                {
                    (None, "pane intentionally retained")
                } else if lane.status == crate::thread::Status::Resolved {
                    if agent.is_none() {
                        (Some(true), "resolved; no bound agent remains")
                    } else if lane.cleanup_pending && lane.retirement.is_some() {
                        (
                            Some(false),
                            "resolved lane still has its bound agent pending cleanup",
                        )
                    } else {
                        // Completed retirement clears the request, including
                        // --keep-pane. A retained agent alone cannot prove a leak.
                        (
                            None,
                            "resolved lane retains its bound agent; retention intent unknown",
                        )
                    }
                } else if !crate::thread::can_check_gone(lane, jiff::Timestamp::now()) {
                    (
                        None,
                        "placement or intentional pane closure; binding not required yet",
                    )
                } else {
                    (
                        Some(pane && agent.is_some()),
                        if !pane {
                            "recorded pane binding is gone or changed"
                        } else if agent.is_none() {
                            "recorded agent binding is gone or changed"
                        } else {
                            "pane and agent bindings match"
                        },
                    )
                };
                if status == Some(true)
                    && lane.status != crate::thread::Status::Resolved
                    && crate::thread::process_bound_to_pane(lane)
                {
                    match herdr.pane_process_info(&lane.pane_id) {
                        Ok(info)
                            if info.pane_id == lane.pane_id
                                && crate::thread::identity_verifies(
                                    lane,
                                    agent.unwrap(),
                                    &info.identities(),
                                ) => {}
                        Ok(info) if info.agent_gone(&lane.pane_id) => {
                            status = Some(false);
                            detail = "bound agent process has exited";
                        }
                        _ => {
                            status = None;
                            detail = "pane and agent match; bound process identity unknown";
                        }
                    }
                }
                check(out, status, &label, detail.into());
            }
        }
    }
}

fn machines_to_check(
    projects: &Projects,
    config_dir: &Path,
    runner: &dyn Runner,
    herdr_bin: &str,
    config: &crate::launch::LaunchConfig,
    repos: &[crate::project::Repo],
) -> Result<BTreeMap<String, Result<crate::contracts::MachineProfile>>> {
    let dispatch = &config.dispatch.machine;
    let mut machines = std::collections::BTreeSet::new();
    if !dispatch.is_empty() {
        machines.insert(dispatch.clone());
    }
    let add_repos = |machines: &mut std::collections::BTreeSet<String>,
                     repos: &[crate::project::Repo]| {
        for repo in repos {
            if let Some(machine) = repo.machine.as_ref().filter(|machine| !machine.is_empty()) {
                machines.insert(machine.clone());
            }
            if let Some(machine) = &repo.review_machine
                && machine != crate::contracts::MACHINE_LOCAL
            {
                machines.insert(machine.clone());
            }
            // A box_path is what makes the dispatch default eligible for this
            // repository. Keep that relationship explicit even though the
            // dispatch row itself is also checked when no project is open.
            if repo.box_path.is_some() && !dispatch.is_empty() {
                machines.insert(dispatch.clone());
            }
        }
    };
    add_repos(&mut machines, repos);
    for project in projects
        .values()
        .filter_map(|project| project.as_ref().ok())
    {
        if let Ok((settings, _)) = project.read_project_md() {
            add_repos(&mut machines, &settings.repos);
        }
        machines.extend(
            crate::thread::list(project)
                .into_iter()
                .filter(|thread| thread.is_remote() && !thread.worktree_path.is_empty())
                .map(|thread| thread.machine_route().to_string()),
        );
    }
    crate::remote::doctor_profiles(runner, herdr_bin, config_dir, &machines)
}

/// ADE repository mapping, selected adapter readiness and the configured disk
/// floor. One read-only SSH snapshot.
fn probe_box(
    runner: &dyn Runner,
    config_dir: &Path,
    profile: &crate::contracts::MachineProfile,
    config: &crate::launch::LaunchConfig,
    min_free_disk_gb: f64,
    repos: &[crate::project::Repo],
) -> Result<ProbeReport> {
    anyhow::ensure!(!profile.target.is_empty(), "has no SSH target");
    let machine_paths = crate::remote::machine_declaration(config_dir, &profile.label)
        .context("machine path declaration is missing")?;
    let mut plan = selected_plan(config, Some(&machine_paths))?;
    plan.disk = Some((machine_paths.worktrees.clone(), min_free_disk_gb));
    plan.snapshot = Some(SnapshotInput {
        home: machine_paths.home.clone(),
        build: machine_paths.build.clone(),
        session: profile.session.clone(),
        repos: repos
            .iter()
            .chain(&machine_paths.repos)
            .filter_map(|repo| repo.box_path.clone())
            .collect(),
    });
    remote_plan(runner, profile, &machine_paths, &plan)
}

fn row_status(level: crate::pi::doctor::Level) -> Option<bool> {
    match level {
        crate::pi::doctor::Level::Ok => Some(true),
        crate::pi::doctor::Level::Warn => None,
        crate::pi::doctor::Level::Fail => Some(false),
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

    #[test]
    fn selected_and_launch_plans_keep_recipe_identity_and_deadline_overrides() {
        let world = crate::scenarios::World::new();
        let ctx = world.ctx();
        let mut config = crate::launch::parse_launch_config(&ctx.config_dir).unwrap();
        let id = "test_claude";
        for (timeout, expected) in [(0, 300_000), (500, 1_000), (72_000, 72_000)] {
            config.recipes.get_mut(id).unwrap().ready_timeout_ms = timeout;
            let selected = selected_plan(&config, None).unwrap();
            assert_eq!(selected.natives[0].2, expected);
            let launch = crate::contracts::Launch {
                kind: "claude".into(),
                recipe_id: id.into(),
                args: config.recipes[id].args.clone(),
                ready_timeout_ms: expected,
                ..Default::default()
            };
            let launched = ProbePlan::launch(&ctx, &launch).unwrap();
            assert_eq!(launched.natives[0].0, selected.natives[0].0);
        }
    }

    #[test]
    fn local_and_remote_execute_the_same_selected_inputs_and_deadlines() {
        let world = std::rc::Rc::new(crate::scenarios::World::new());
        let runner = std::rc::Rc::new(FakeRunner::new());
        runner.on("df", ok("Filesystem blocks used available capacity path\nfixture 100000000 0 99999999 1% /worktrees\n"));
        runner.on("fixture-provider", fail(1, "unrecognized local diagnostic"));
        let ctx = Ctx {
            runner: runner.as_ref(),
            ..world.ctx()
        };
        let probe = |model: &str| NativeProbe {
            kind: "same-kind".into(),
            cache_key: model.into(),
            program: "fixture-provider".into(),
            args: vec![model.into()],
        };
        let plan = ProbePlan {
            natives: vec![
                ("recipe-one".into(), probe("model-one"), 41_000),
                ("recipe-two".into(), probe("model-two"), 72_000),
            ],
            disk: Some(("/worktrees".into(), 12.0)),
            ..Default::default()
        };
        let local = execute_plan(&ctx, &plan).unwrap();
        let remote = FakeRunner::new();
        let target_world = world.clone();
        let target_runner = runner.clone();
        remote.on_fn(
            |cmd| cmd.program == "ssh",
            move |cmd| {
                let input = crate::box_helper::tests::doctor_input(cmd.stdin.as_ref().unwrap());
                let bytes = cmd.stdin.as_ref().unwrap();
                for excluded in ["routing", "credentials", "auth.json", "config.toml"] {
                    assert!(!bytes.contains(excluded), "{bytes}");
                }
                let ctx = Ctx {
                    runner: target_runner.as_ref(),
                    ..target_world.ctx()
                };
                let report = execute_plan(&ctx, &input).unwrap();
                Ok(ok(&crate::box_helper::tests::ready(&report)))
            },
        );
        let report = remote_plan(
            &remote,
            &box_profile(),
            &crate::remote::MachineDeclaration {
                ade_bin: "/box/herdr-ade".into(),
                root: "/box/root".into(),
                path: "/box/bin:/usr/bin".into(),
                ..Default::default()
            },
            &plan,
        )
        .unwrap();
        assert_eq!(local.rows, report.rows);
        assert_eq!(remote.count("ssh"), 1);
        let calls = runner.calls.borrow();
        let probes: Vec<_> = calls
            .iter()
            .filter(|cmd| cmd.program == "fixture-provider")
            .collect();
        assert_eq!(probes.len(), 4);
        for pair in probes.chunks(2) {
            assert_eq!(pair[0].timeout, Duration::from_secs(41));
            assert_eq!(pair[1].timeout, Duration::from_secs(72));
            assert_eq!(pair[0].args, ["model-one"]);
            assert_eq!(pair[1].args, ["model-two"]);
            assert!(pair.iter().all(|cmd| cmd.own_group));
        }
        assert!(
            calls
                .iter()
                .filter(|cmd| cmd.program == "df")
                .all(|cmd| cmd.args == ["-Pk", "/worktrees"])
        );
        assert_eq!(
            report
                .rows
                .iter()
                .filter(|row| row.label.starts_with("recipe "))
                .count(),
            2
        );
    }

    #[test]
    fn one_remote_failure_carries_its_evidence_without_a_second_model_call() {
        for evidence in [
            crate::pi::doctor::FailureEvidence::Provider,
            crate::pi::doctor::FailureEvidence::Unknown,
        ] {
            let world = crate::scenarios::World::new();
            let runner = FakeRunner::new();
            runner.on_fn(
                |cmd| cmd.program == "ssh",
                move |cmd| {
                    let value: serde_json::Value = serde_json::from_str(
                        &crate::doctor::boundary_diagnostic_output(cmd, 99_999_999, None).stdout,
                    )
                    .unwrap();
                    let mut report: ProbeReport =
                        serde_json::from_value(value["result"].clone()).unwrap();
                    let row = report
                        .rows
                        .iter_mut()
                        .find(|row| row.label != "disk")
                        .unwrap();
                    row.level = crate::pi::doctor::Level::Fail;
                    row.evidence = evidence;
                    row.detail = "original target diagnostic".into();
                    Ok(ok(&crate::box_helper::tests::ready(&report)))
                },
            );
            let config = machine_config(&["pi"]);
            let ctx = Ctx {
                config_dir: config.path().to_path_buf(),
                runner: &runner,
                ..world.ctx()
            };
            let launch = crate::contracts::Launch {
                kind: "pi".into(),
                recipe_id: "selected-pi".into(),
                args: vec![
                    "--provider".into(),
                    "openai-codex".into(),
                    "--model".into(),
                    "gpt-6.1-sol".into(),
                ],
                ..Default::default()
            };
            let error = recipe_ready_on_box_probe(&ctx, &box_profile(), &launch).unwrap_err();
            assert!(error.to_string().contains("original target diagnostic"));
            assert!(error.to_string().contains("recipe selected-pi"));
            assert_eq!(
                crate::pi_ade::failure_class(&error),
                match evidence {
                    crate::pi::doctor::FailureEvidence::Provider =>
                        crate::contracts::FailureClass::Provider,
                    crate::pi::doctor::FailureEvidence::Unknown =>
                        crate::contracts::FailureClass::Unknown,
                }
            );
            assert_eq!(runner.count("ssh"), 1);
        }
    }

    #[test]
    fn incomplete_remote_observations_never_establish_readiness() {
        let world = crate::scenarios::World::new();
        let machine = crate::remote::MachineDeclaration::default();
        let plan = ProbePlan {
            disk: Some(("/worktrees".into(), 12.0)),
            ..Default::default()
        };
        for output in [ok("{}"), ok(&serde_json::to_string(&ProbeReport::default()).unwrap()), Output {
            timed_out: true, stdout: "{\"active\":\"readiness\",\"observations\":[{\"detail\":\"disk already measured\"}]}\n".into(), ..Default::default()
        }, Output { code: Some(255), stdout: serde_json::to_string(&box_report()).unwrap(), ..Default::default() }] {
            let runner = FakeRunner::new();
            runner.on("ssh", output);
            let error = remote_plan(&runner, &box_profile(), &machine, &plan).unwrap_err();
            assert_eq!(crate::pi_ade::failure_class(&error), crate::contracts::FailureClass::Unknown);
            if error.to_string().contains("slow:") { assert!(error.to_string().contains("disk already measured")); }
            assert_eq!(runner.count("ssh"), 1);
        }
        assert!(!world.root.join(".readiness").exists());
    }

    #[test]
    fn disk_observations_use_only_supplied_worktree_filesystem_facts() {
        let world = crate::scenarios::World::new();
        for (facts, level) in [
            (
                "fixture 10000000 0 1000000 1% /worktrees",
                crate::pi::doctor::Level::Fail,
            ),
            ("unknown", crate::pi::doctor::Level::Warn),
        ] {
            let runner = FakeRunner::new();
            runner.on("df", ok(facts));
            let ctx = Ctx {
                runner: &runner,
                ..world.ctx()
            };
            let report = execute_plan(
                &ctx,
                &ProbePlan {
                    disk: Some(("/worktrees".into(), 12.0)),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(report.rows[0].level, level);
            assert!(require_ready(&report).is_err());
            assert_eq!(runner.calls.borrow()[0].args, ["-Pk", "/worktrees"]);
        }
        let ctx = Ctx {
            runner: &FakeRunner::new(),
            ..world.ctx()
        };
        assert!(check_start_disk(&ctx, None, Some("/missing-fixture")).is_err());
    }

    #[test]
    fn doctor_refuses_unknown_review_machine() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        world.add_repo(&project, "/code/demo");
        let (mut settings, body) = project.read_project_md().unwrap();
        settings.repos[0].review_machine = Some("missing-box".into());
        std::fs::write(
            project.project_md(),
            format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        let (_, _, checks) = report_with_checks(
            &world.env,
            &world.root,
            &world.ctx().config_dir,
            &SessionFlags::default(),
            &world.runner,
            None,
        );
        let check = checks
            .iter()
            .find(|c| c.label == "project demo repo /code/demo review_machine")
            .unwrap();
        assert_eq!(check.status, "failed");
        assert!(
            check
                .detail
                .contains("unknown or invalid review_machine `missing-box`")
        );
    }

    #[test]
    fn doctor_warns_for_a_path_only_repo_with_a_remote() {
        let world = crate::scenarios::World::new();
        let project = world.project("chainlm", "a.sock");
        let (mut settings, body) = project.read_project_md().unwrap();
        settings.repos.push(crate::project::Repo {
            path: "/code/chainlm".into(),
            ..Default::default()
        });
        std::fs::write(
            project.project_md(),
            format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        world.runner.on("-C /code/chainlm remote", ok("origin\n"));
        let config = world.home.path().join("cfg");
        let (_, _, checks) = report_with_checks(
            &world.env,
            &world.root,
            &config,
            &SessionFlags::default(),
            &world.runner,
            None,
        );
        let warning = checks
            .iter()
            .find(|c| c.label == "project chainlm repo /code/chainlm")
            .unwrap();
        assert_eq!(warning.status, "warning");
        assert!(warning.detail.contains("origin"));
        assert!(warning.detail.contains("push_remote = \"origin\""));
        assert!(
            warning
                .detail
                .contains(&project.project_md().display().to_string())
        );
        settings.repos[0].push_remote = Some("origin".into());
        std::fs::write(
            project.project_md(),
            format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        let (_, _, checks) = report_with_checks(
            &world.env,
            &world.root,
            &config,
            &SessionFlags::default(),
            &world.runner,
            None,
        );
        assert!(!checks.iter().any(|c| c.label == warning.label));
    }

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
                "{ROUTING_CONFIG}\n[machines.buildbox]\nlabel = \"buildbox\"\ntarget = \"buildbox-pi\"\nsession = \"default\"\nhome = \"/home/agent\"\nroot = \"/home/agent/.herdr-ade\"\nworktrees = \"/home/agent/projects\"\nbuild = \"/home/agent/build/lanes\"\npath = \"/home/agent/.local/bin:/home/agent/.cargo/bin:/usr/local/bin:/usr/bin:/bin\"\nade_bin = \"/home/agent/.local/bin/herdr-ade\"\npi_bin = \"/home/agent/.local/bin/herdr-pi\"\nkinds = [{kinds}]\n[[machines.buildbox.repos]]\npath = \"/local/herdr\"\nbox_path = \"/home/agent/projects/herdr\"\npublish_url = \"https://example.test/herdr.git\"\n[[machines.buildbox.repos]]\npath = \"/local/herdr-ade\"\nbox_path = \"/home/agent/projects/herdr-ade\"\npublish_url = \"https://example.test/herdr-ade.git\"\n"
            ),
        )
        .unwrap();
        config
    }

    fn box_rows_with_snapshot(
        runner: &dyn Runner,
        config_dir: &Path,
        profile: &crate::contracts::MachineProfile,
        recipes: &BTreeMap<String, crate::contracts::Recipe>,
        floor: f64,
        snapshot: Option<&mut SnapshotObservation>,
    ) -> Vec<(Option<bool>, String, String)> {
        let mut config = crate::launch::parse_launch_config(config_dir).unwrap();
        config.recipes = recipes.clone();
        match probe_box(
            runner,
            config_dir,
            profile,
            &config,
            floor,
            &crate::harness::repos(config_dir).unwrap(),
        ) {
            Ok(report) => {
                if let Some(snapshot) = snapshot {
                    *snapshot = report.snapshot;
                }
                report
                    .rows
                    .into_iter()
                    .map(|row| {
                        (
                            row_status(row.level),
                            format!("box {} {}", profile.label, row.label),
                            row.detail,
                        )
                    })
                    .collect()
            }
            Err(error) => vec![(
                Some(false),
                format!("box {}", profile.label),
                format!("{error:#}"),
            )],
        }
    }

    fn runner_with_machine_list(version: &str, machines: &str) -> FakeRunner {
        let runner = FakeRunner::new();
        runner.on("herdr --version", ok(version));
        runner.on("session list --json", ok(r#"{"sessions":[]}"#));
        runner.on("git --version", ok("git version 2.50.0\n"));
        runner.on("df -Pk", ok("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk 200000000 1000000 199000000 1% /\n"));
        runner.on("claude", ok("OK"));
        runner.on("agy", ok("OK"));
        runner.on("machine list --json", ok(machines));
        runner
    }

    fn runner_with_herdr(version: &str) -> FakeRunner {
        runner_with_machine_list(version, "[]")
    }

    /// Like `runner_with_herdr`, but the local session answers `pane list` and
    /// `agent list` with the given JSON, so a project row can be read.
    fn runner_with_project(version: &str, panes: &str, agents: &str) -> FakeRunner {
        let runner = runner_with_herdr(version);
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
    fn project_checks_distinguish_closed_coordinators_from_observation_gaps() {
        for state in ["unbound", "empty", "missing", "unreadable", "unreachable"] {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().join("root");
            let config = home.path().join("cfg");
            write_routing_config(&config);
            let project = opened_project(home.path(), &root, "w1:p1", "coordinator");
            let socket = project.coordinator().unwrap().socket;
            let binding = project.state_dir().join("coordinator.json");
            match state {
                "unbound" => std::fs::remove_file(&binding).unwrap(),
                "empty" => {
                    project
                        .update_coordinator(|record| record.socket.clear())
                        .unwrap();
                }
                "missing" => std::fs::remove_file(&socket).unwrap(),
                "unreadable" => std::fs::write(&binding, "{bad").unwrap(),
                _ => {}
            }
            let env = Env::for_test(home.path(), &[]);
            let runner = runner_with_herdr("herdr 0.9.0");
            runner.on(
                "pane list",
                if state == "unreachable" {
                    fail(1, "socket connection refused")
                } else {
                    ok(r#"{"result":{"panes":[]}}"#)
                },
            );
            let (_, _, checks) = report_with_checks(
                &env,
                &root,
                &config,
                &SessionFlags::default(),
                &runner,
                None,
            );
            let rows: Vec<_> = checks
                .iter()
                .filter(|check| check.label == "project demo")
                .collect();
            assert_eq!(rows.len(), 1, "{state}: {rows:?}");
            let row = rows[0];
            if matches!(state, "unbound" | "empty") {
                assert_eq!(row.status, "ok", "{state}: {row:?}");
                assert!(row.detail.contains("coordinator closed"), "{row:?}");
                assert!(row.detail.contains("reviews skipped"), "{row:?}");
            } else {
                assert_eq!(row.status, "failed", "{state}: {row:?}");
                assert!(!row.detail.contains("coordinator closed"), "{row:?}");
                let expected_path = if state == "unreadable" {
                    binding.to_string_lossy().into_owned()
                } else {
                    socket
                };
                assert!(row.detail.contains(&expected_path), "{row:?}");
            }
        }
    }

    #[test]
    fn a_cached_native_failure_preserves_the_adapter_evidence() {
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
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("native-")
            })
            .unwrap();
        let cache = std::fs::read_to_string(cache_path).unwrap();
        assert!(cache.contains("subscription expired"), "{cache}");

        let cached = recipe_ready_local(&ctx, &launch).unwrap_err().to_string();
        let cache: serde_json::Value = serde_json::from_str(&cache).unwrap();
        assert_eq!(cache["row"]["evidence"], "Provider");
        assert_eq!(cache["row"]["level"], "Fail");
        assert_eq!(cached, first);
        assert_eq!(runner.count("claude"), 1);
        let calls = runner.calls.borrow();
        let probe = calls.iter().find(|call| call.program == "claude").unwrap();
        assert_eq!(probe.cwd.as_deref(), Some(ctx.root.as_path()));
    }

    #[test]
    fn a_timed_out_native_probe_shares_unknown_without_claiming_authentication_failure() {
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
            assert!(error.contains("provider status is unknown"));
            assert!(!error.contains("authentication failed"));
        }
        assert_eq!(runner.count("claude"), 1);
        crate::adapters::expire_dependency_probe(
            &ctx.root,
            crate::contracts::MACHINE_LOCAL,
            &launch,
        );
        assert!(recipe_ready_local(&ctx, &launch).is_err());
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
        assert!(!crate::pi::doctor::positive_sign_in_evidence(
            &output.error_text()
        ));
    }

    const LANE_PANES: &str = r#"{"result":{"panes":[{"workspace_id":"w3","tab_id":"w3:t1","pane_id":"w3:p1","cwd":"/lane"}]}}"#;
    const LANE_AGENTS: &str = r#"{"result":{"agents":[{"workspace_id":"w3","tab_id":"w3:t1","pane_id":"w3:p1","cwd":"/lane","name":"hp-demo-t-0001"}]}}"#;

    fn bound_lane(project: &project::Project) -> crate::thread::Thread {
        crate::thread::allocate(project, |lane| {
            lane.status = crate::thread::Status::Open;
            lane.machine = "buildbox".into();
            lane.machine_id = "abc".into();
            lane.workspace_id = "w3".into();
            lane.tab_id = "w3:t1".into();
            lane.pane_id = "w3:p1".into();
            lane.cwd = "/lane".into();
            lane.agent_name = "hp-demo-t-0001".into();
        })
        .unwrap()
    }

    fn lane_rows(
        root: &Path,
        runner: &FakeRunner,
        snapshot: Option<&SnapshotObservation>,
    ) -> Vec<(Option<bool>, String)> {
        let mut rows = Vec::new();
        check_lane_bindings(
            &mut String::new(),
            &mut |_, status, _, detail| rows.push((status, detail)),
            root,
            ("abc", "buildbox"),
            ("herdr", runner),
            snapshot,
        );
        assert_eq!(runner.count("workspace list"), 0);
        assert_eq!(runner.count("tab list"), 0);
        assert_eq!(runner.count("workspace close"), 0);
        assert_eq!(runner.count("tab close"), 0);
        rows
    }

    #[test]
    fn unrelated_agentless_shells_and_duplicate_labels_are_not_diagnosed() {
        let home = tempfile::tempdir().unwrap();
        let project = project::create(home.path(), "demo", "", vec![]).unwrap();
        bound_lane(&project);
        let runner = FakeRunner::new();
        runner.on("workspace list", ok(r#"{"result":{"workspaces":[{"workspace_id":"w1","label":"~"},{"workspace_id":"w2","label":"Demo"},{"workspace_id":"w3","label":"Demo"}]}}"#));
        runner.on(
            "tab list",
            ok(r#"{"result":{"tabs":[{"workspace_id":"w3","tab_id":"w3:shell"}]}}"#),
        );
        runner.on("pane list", ok(LANE_PANES));
        runner.on("agent list", ok(LANE_AGENTS));
        let rows = lane_rows(home.path(), &runner, None);
        assert_eq!(rows[0].0, Some(true), "{rows:?}");
        crate::thread::update(&project, "t-0001", |lane| lane.machine_id.clear()).unwrap();
        assert_eq!(
            lane_rows(home.path(), &runner, None)[0].0,
            Some(true),
            "old label-only machine records still bind"
        );
    }

    #[test]
    fn owned_lane_binding_failures_stay_loud_even_with_a_closed_coordinator() {
        for (panes, agents) in [
            (LANE_PANES.replace("/lane", "/other"), LANE_AGENTS.into()),
            (
                LANE_PANES.into(),
                LANE_AGENTS.replace("hp-demo-t-0001", "someone-else"),
            ),
            (r#"{"result":{"panes":[]}}"#.into(), LANE_AGENTS.into()),
        ] {
            let home = tempfile::tempdir().unwrap();
            let project = project::create(home.path(), "demo", "", vec![]).unwrap();
            bound_lane(&project);
            let runner = FakeRunner::new();
            let snapshot = SnapshotObservation {
                panes: serde_json::from_str::<serde_json::Value>(&panes)
                    .ok()
                    .and_then(|value| {
                        serde_json::from_value(value["result"]["panes"].clone()).ok()
                    }),
                agents: serde_json::from_str::<serde_json::Value>(&agents)
                    .ok()
                    .and_then(|value| {
                        serde_json::from_value(value["result"]["agents"].clone()).ok()
                    }),
                ..Default::default()
            };
            assert_eq!(
                lane_rows(home.path(), &runner, Some(&snapshot))[0].0,
                Some(false)
            );
        }
    }

    #[test]
    fn only_a_resolved_lanes_exact_bound_agent_is_a_leftover() {
        let home = tempfile::tempdir().unwrap();
        let project = project::create(home.path(), "demo", "", vec![]).unwrap();
        let lane = bound_lane(&project);
        crate::thread::update(&project, &lane.id, |lane| {
            lane.status = crate::thread::Status::Resolved;
            lane.cleanup_pending = true;
            lane.retirement = Some(Default::default());
        })
        .unwrap();
        let runner = FakeRunner::new();
        runner.on("pane list", ok(LANE_PANES));
        runner.on("agent list", ok(LANE_AGENTS));
        assert_eq!(lane_rows(home.path(), &runner, None)[0].0, Some(false));
        let runner = FakeRunner::new();
        runner.on("pane list", ok(LANE_PANES));
        runner.on(
            "agent list",
            ok(&LANE_AGENTS.replace("hp-demo-t-0001", "someone-else")),
        );
        assert_eq!(lane_rows(home.path(), &runner, None)[0].0, Some(true));
    }

    #[test]
    fn completed_keep_pane_retirement_does_not_become_a_leftover_failure() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let lane = world.thread(&project, world.home.path(), |lane| {
            lane.kind = crate::thread::Kind::Tab;
            lane.worktree_path.clear();
            lane.repo.clear();
        });
        *world.panes.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::pane_json("w2", "w2:t1", "w2:p1", &lane.cwd)
        );
        *world.agents.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::agent_json(
                "w2",
                "w2:t1",
                "w2:p1",
                &lane.cwd,
                &lane.agent_name,
                "idle"
            )
        );
        crate::threads::resolve(
            &world.ctx(),
            "demo",
            &lane.id,
            &crate::threads::ResolveArgs {
                keep_pane: true,
                ..Default::default()
            },
        )
        .unwrap();
        let resolved = crate::thread::load(&project, &lane.id).unwrap();
        assert_eq!(resolved.status, crate::thread::Status::Resolved);
        assert!(resolved.retirement.is_none());
        assert!(!resolved.cleanup_pending);
        let mut rows = Vec::new();
        check_lane_bindings(
            &mut String::new(),
            &mut |_, status, _, detail| rows.push((status, detail)),
            &world.root,
            ("local", "local"),
            ("herdr", &world.runner),
            None,
        );
        assert_eq!(rows[0].0, None, "{rows:?}");
        assert!(rows[0].1.contains("retention intent unknown"), "{rows:?}");
        assert_eq!(world.runner.count("workspace close"), 0);
        assert_eq!(world.runner.count("tab close"), 0);
    }

    #[test]
    fn missing_lane_observations_and_intentional_closure_are_unknown_not_success() {
        let home = tempfile::tempdir().unwrap();
        let project = project::create(home.path(), "demo", "", vec![]).unwrap();
        let lane = bound_lane(&project);
        let runner = FakeRunner::new();
        assert_eq!(
            lane_rows(home.path(), &runner, Some(&SnapshotObservation::default()))[0].0,
            None
        );
        crate::thread::update(&project, &lane.id, |lane| lane.parked = true).unwrap();
        runner.on("pane list", ok(r#"{"result":{"panes":[]}}"#));
        runner.on("agent list", ok(r#"{"result":{"agents":[]}}"#));
        assert_eq!(lane_rows(home.path(), &runner, None)[0].0, None);
        std::fs::write(
            project.state_dir().join("threads/t-0099.toml"),
            "status = [",
        )
        .unwrap();
        let rows = lane_rows(home.path(), &runner, None);
        assert!(
            rows.iter()
                .any(|row| row.0.is_none() && row.1.contains("ownership unknown")),
            "{rows:?}"
        );
    }

    #[test]
    fn local_lanes_use_the_retained_socket_and_known_process_evidence() {
        for (process, expected) in [
            (
                r#"{"pane_id":"w3:p1","foreground_processes":[{"pid":42,"name":"pi"}]}"#,
                Some(true),
            ),
            (
                r#"{"pane_id":"w3:p1","foreground_processes":[{"pid":9,"name":"bash"}]}"#,
                Some(false),
            ),
            (
                r#"{"pane_id":"w3:p1","foreground_processes":[{"pid":9,"name":"tool"}]}"#,
                None,
            ),
            (r#"{"pane_id":"other","foreground_processes":[]}"#, None),
        ] {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().join("root");
            let project = opened_project(home.path(), &root, "w1:p1", "coordinator");
            let lane = bound_lane(&project);
            let socket = home
                .path()
                .join("retained.sock")
                .to_string_lossy()
                .into_owned();
            crate::thread::update(&project, &lane.id, |lane| {
                lane.machine.clear();
                lane.machine_id.clear();
                lane.identity = crate::contracts::IdentityBinding {
                    socket: socket.clone(),
                    pane_id: lane.pane_id.clone(),
                    tab_id: lane.tab_id.clone(),
                    workspace_id: lane.workspace_id.clone(),
                    process: Some(crate::contracts::ProcessIdentity {
                        pid: 42,
                        argv0: "pi".into(),
                    }),
                    ..Default::default()
                };
            })
            .unwrap();
            let runner = FakeRunner::new();
            runner.on("pane list", ok(LANE_PANES));
            runner.on("agent list", ok(LANE_AGENTS));
            runner.on(
                "pane process-info",
                ok(&format!(r#"{{"result":{{"process_info":{process}}}}}"#)),
            );
            let observe = || {
                let mut rows = Vec::new();
                check_lane_bindings(
                    &mut String::new(),
                    &mut |_, status, _, detail| rows.push((status, detail)),
                    &root,
                    ("local", "local"),
                    ("herdr", &runner),
                    None,
                );
                rows
            };
            assert_eq!(observe()[0].0, expected);
            assert!(runner.calls.borrow().iter().all(|call| {
                call.env
                    .contains(&("HERDR_SOCKET_PATH".into(), socket.clone()))
            }));
            project
                .update_coordinator(|record| record.socket.clear())
                .unwrap();
            assert_eq!(
                observe()[0].0,
                expected,
                "closed coordinator still observes its lane"
            );
        }
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
        assert!(
            detail.contains(folder.to_string_lossy().as_ref()),
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
        runner.on("ade-boundary-probe.mjs", pi_execution_fixture());
        runner.on("/usr/bin/bwrap", ok(""));

        let ctx = Ctx {
            env: &env,
            root: home.path().join("root"),
            config_dir: config,
            runner: &runner,
            detached_ticker: false,
        };
        let outcome = run(&ctx, &SessionFlags::default()).unwrap();
        let text = &outcome.message;

        assert!(outcome.healthy, "{text}");
        assert!(!outcome.checks.iter().any(|row| row.label == "pi"));
        assert!(text.contains("[ok  ] lane worker:"), "{text}");
        assert!(!text.contains("routing_recipe_missing"), "{text}");
        assert!(!text.contains("[FAIL] recipes:"), "{text}");
        assert_eq!(
            runner.count("agent start --help"),
            1,
            "only the parent CLI check runs; recipe validation is skipped"
        );
        assert_eq!(runner.count("machine list"), 0);
        for command in ["ssh -V", "rsync --version", "gh --version"] {
            assert_eq!(runner.count(command), 0, "unused tool {command}");
        }
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

        let (text, _) = report(
            &env,
            &home.path().join("root-with-no-projects"),
            &home.path().join("cfg"),
            &SessionFlags::default(),
            &runner,
        );
        assert!(text.contains("buildbox-id"), "{text}");
        assert!(text.contains("[ok  ] box buildbox disk"), "{text}");
        assert_eq!(runner.count("machine list --json"), 1);
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
    fn open_box_threads_keep_their_build_folders_out_of_orphan_results() {
        let home = tempfile::tempdir().unwrap();
        write_machine_config(&home.path().join("cfg"));
        let env = Env::for_test(home.path(), &[]);
        let root = home.path().join("root");
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let thread = crate::thread::allocate(&project, |thread| {
            thread.status = crate::thread::Status::Open;
            thread.machine = "buildbox".into();
            thread.machine_id = "abc".into();
            thread.title = "Review r1".into();
        })
        .unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&crate::box_helper::tests::ready(ProbeReport {
                snapshot: SnapshotObservation {
                    builds: Some(vec![format!("/home/agent/build/lanes/demo-{}", thread.id)]),
                    ..Default::default()
                },
                ..Default::default()
            })),
        );
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };

        let (leftovers, errors) = finished_build_folders(&ctx, &box_profile());

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
            ok(&crate::box_helper::tests::ready(ProbeReport {
                snapshot: SnapshotObservation {
                    builds: Some(vec!["/home/agent/build/lanes/demo-t-0099".into()]),
                    ..Default::default()
                },
                ..Default::default()
            })),
        );
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: home.path().join("cfg"),
            runner: &runner,
            detached_ticker: false,
        };

        let (leftovers, errors) = finished_build_folders(&ctx, &box_profile());

        assert!(leftovers.is_empty(), "{leftovers:?}");
        assert!(!errors.is_empty(), "{errors:?}");
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
        runner.on("df -Pk", fail(1, "df failed"));
        runner.on("ade-boundary-probe.mjs", pi_execution_fixture());
        runner.on("/usr/bin/bwrap", ok(""));

        let (text, healthy, checks) = report_with_checks(
            &env,
            &home.path().join("root"),
            &config,
            &SessionFlags::default(),
            &runner,
            None,
        );

        assert!(healthy, "{text}");
        assert!(
            checks
                .iter()
                .any(|check| { check.status == "warning" && check.label == "machine local disk" })
        );
    }

    #[test]
    fn unreadable_start_disk_is_unreachable_not_low() {
        let world = crate::scenarios::World::new();
        let runner = FakeRunner::new();
        runner.on("df", ok("no disk facts"));
        let ctx = Ctx {
            runner: &runner,
            ..world.ctx()
        };
        let error = check_start_disk(&ctx, None, Some("/box/work"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unreachable: disk free space unknown under /box/work"),
            "{error}"
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
        let (text, _) = report(
            &env,
            &home.path().join("root"),
            &config,
            &SessionFlags::default(),
            &runner,
        );
        assert!(text.contains("provider openai-codex/"), "{text}");
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
        assert!(text.contains("[FAIL] project demo repo /repo:"), "{text}");
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
        assert!(text.contains("[FAIL] box buildbox: unreachable:"), "{text}");
    }

    #[test]
    fn ssh_answering_but_snapshot_stalling_reports_its_last_part() {
        let config = machine_config(&["pi"]);
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            crate::runner::Output {
                timed_out: true,
                stdout: "{\"active\":\"facts\"}\n{\"active\":\"herdr tabs\"}\n".into(),
                ..Default::default()
            },
        );
        let rows = box_rows_with_snapshot(
            &runner,
            config.path(),
            &box_profile(),
            &default_recipes(),
            12.0,
            None,
        );
        assert!(
            rows[0]
                .2
                .starts_with("slow: snapshot timed out while running herdr tabs")
        );
        assert!(rows[0].2.contains("partial observations"));
    }

    fn default_recipes() -> BTreeMap<String, crate::contracts::Recipe> {
        let dir = tempfile::tempdir().unwrap();
        write_routing_config(dir.path());
        crate::launch::parse_launch_config(dir.path())
            .unwrap()
            .recipes
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
        let recipes: BTreeMap<String, _> = BTreeMap::from([
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
        for (id, recipe) in recipes {
            let mut selected = crate::launch::parse_launch_config(config.path()).unwrap();
            selected.recipes.insert(id.clone(), recipe);
            selected.routing.default = id;
            selected.routing.rules.clear();
            assert!(
                probe_box(&runner, config.path(), &box_profile(), &selected, 12.0, &[]).is_err()
            );
        }
        assert_eq!(runner.count("ssh"), 0);
    }

    fn box_report() -> ProbeReport {
        use crate::pi::doctor::Row;
        let mut rows = vec![
            Row::ok("server", "herdr 0.9.1"),
            Row::ok("disk", "100 GB free"),
            Row::ok("rules", "RULES.md sha256 abc"),
            Row::ok("wrapper on PATH", "/home/agent/.local/bin/pi"),
            Row::ok("repo /home/agent/projects/herdr", "clone ok"),
            Row::ok("repo /home/agent/projects/herdr-ade", "clone ok"),
        ];
        for (id, recipe) in default_recipes()
            .into_iter()
            .filter(|(_, recipe)| recipe.enabled)
        {
            let label = if recipe.kind == "pi" {
                let model = crate::pi::launch::flag_value(&recipe.args, "--model").unwrap();
                format!("recipe {id} (provider {}/{model})", recipe.provider)
            } else {
                format!("recipe {id}")
            };
            rows.push(Row::ok(label, "selected model ready"));
            rows.push(Row::ok(
                format!("recipe {id} execution"),
                "fixture namespace probe passed",
            ));
        }
        ProbeReport {
            rows,
            snapshot: SnapshotObservation {
                panes: Some(Vec::new()),
                agents: Some(Vec::new()),
                builds: Some(Vec::new()),
                build_error: None,
            },
        }
    }

    fn box_facts() -> String {
        crate::box_helper::tests::ready(box_report())
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
    fn box_rows_keep_the_real_lane_readiness_failures() {
        let config = machine_config(&["pi", "claude", "agy"]);
        let runner = FakeRunner::new();
        let mut report = box_report();
        let row = report
            .rows
            .iter_mut()
            .find(|row| row.label == "recipe agy_gemini_flash")
            .unwrap();
        row.level = crate::pi::doctor::Level::Fail;
        row.detail =
            "agy readiness could not run: binary missing; provider status is unknown".into();
        runner.on("ssh", ok(&crate::box_helper::tests::ready(&report)));
        let rows = box_rows_with_snapshot(
            &runner,
            config.path(),
            &box_profile(),
            &default_recipes(),
            12.0,
            None,
        );
        let find = |label: &str| {
            rows.iter()
                .find(|(_, name, _)| name == label)
                .unwrap_or_else(|| panic!("no row {label}"))
        };
        assert_eq!(find("box buildbox recipe agy_gemini_flash").0, Some(false));
        assert!(
            find("box buildbox recipe agy_gemini_flash")
                .2
                .contains("binary missing")
        );
        assert!(
            find("box buildbox recipe agy_gemini_flash")
                .2
                .contains("unknown")
        );
    }

    #[test]
    fn recipe_advisory_is_reported_instead_of_adapter_default_and_network_is_named() {
        let world = crate::scenarios::World::new();
        std::fs::write(world.ctx().config_dir.join("config.toml"), "[routing]\ndefault = 'host'\n[recipes.host]\nkind = 'pi'\nprovider = 'p'\nexecution = 'advisory'\nargs = ['--provider', 'p', '--model', 'm', '--thinking', 'high', '--no-skills']\n[recipes.untrusted]\nkind = 'pi'\nprovider = 'p'\nnetwork = 'denied'\nargs = ['--provider', 'p', '--model', 'm', '--thinking', 'high', '--no-skills']\n").unwrap();
        let config = crate::launch::parse_launch_config(&world.ctx().config_dir).unwrap();
        let plan = selected_plan(&config, None).unwrap();
        let (id, backend, network) = plan
            .boundaries
            .iter()
            .find(|(id, _, _)| id == "host")
            .unwrap();
        assert_eq!(backend, "advisory");
        let row = execution_row(&world.ctx(), id, backend, network);
        assert_eq!(row.level, crate::pi::doctor::Level::Warn);
        assert!(row.detail.starts_with("advisory:"));
        assert_eq!(world.runner.count("/usr/bin/bwrap"), 0);
        let (_, backend, network) = plan
            .boundaries
            .iter()
            .find(|(id, _, _)| id == "untrusted")
            .unwrap();
        assert_eq!(network, "denied");
        assert!(
            crate::launch::execution_description(backend, "linux", network)
                .contains("network denied")
        );
    }

    #[test]
    fn remote_namespace_failure_keeps_ready_first_launch_on_its_box() {
        let world = crate::scenarios::World::new();
        world.runner.on_fn(
            |cmd| cmd.display().contains("HERDR_ADE_BOX_INPUT"),
            |cmd| {
                let output = boundary_diagnostic_output(cmd, 100_000_000, None);
                let mut reply: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
                for row in reply["result"]["rows"].as_array_mut().unwrap() {
                    if row["label"].as_str().unwrap().ends_with(" execution") {
                        *row = serde_json::to_value(crate::pi::doctor::Row::fail(
                            row["label"].as_str().unwrap(),
                            "advisory: namespace denied",
                        ))
                        .unwrap();
                    }
                }
                Ok(crate::runner::fake::ok(&reply.to_string()))
            },
        );
        let profile = crate::contracts::MachineProfile {
            label: "box".into(),
            target: "box".into(),
            ..Default::default()
        };
        let mut launch = crate::contracts::Launch {
            kind: "pi".into(),
            recipe_id: "pi".into(),
            args: vec![
                "--provider".into(),
                "p".into(),
                "--model".into(),
                "m".into(),
            ],
            env: vec![format!(
                "HERDR_ADE_EXECUTION={}",
                crate::launch::EXECUTION_BACKEND
            )],
            ..Default::default()
        };
        assert!(recipe_ready_on_box_probe(&world.ctx(), &profile, &launch).is_ok());
        launch.args.push("--no-extensions".into());
        assert!(
            recipe_ready_on_box_probe(&world.ctx(), &profile, &launch)
                .unwrap_err()
                .to_string()
                .contains("namespace denied")
        );
    }

    #[test]
    fn first_launch_boundary_failure_is_not_a_provider_stop_but_pinned_launches_refuse_it() {
        use crate::pi::doctor::Row;
        let report = || ProbeReport {
            rows: vec![
                Row::ok("provider p/m", "ready"),
                Row::fail("recipe pi execution", "advisory: namespace denied"),
            ],
            ..Default::default()
        };
        let mut launch = crate::contracts::Launch {
            recipe_id: "pi".into(),
            ..Default::default()
        };
        assert!(require_launch_ready(report(), &launch).is_ok());
        let mut provider_failure = report();
        provider_failure.rows[0] = Row::fail("provider p/m", "provider unreachable");
        assert!(require_launch_ready(provider_failure, &launch).is_err());
        launch.args.push("--no-extensions".into());
        assert!(
            require_launch_ready(report(), &launch)
                .unwrap_err()
                .to_string()
                .contains("namespace denied")
        );
    }

    #[test]
    fn namespace_readiness_reports_advisory_fallback_without_claiming_it_is_secure() {
        let world = crate::scenarios::World::new();
        let backend = crate::launch::EXECUTION_BACKEND;
        let advisory = execution_row(&world.ctx(), "claude", "", "allowed");
        assert_eq!(advisory.level, crate::pi::doctor::Level::Warn);
        assert!(advisory.detail.starts_with("advisory:"));
        if cfg!(target_os = "linux") {
            world.runner.on(
                "/usr/bin/bwrap",
                crate::runner::fake::fail(1, "namespaces disabled"),
            );
            let denied = execution_row(&world.ctx(), "pi", backend, "allowed");
            assert_eq!(denied.level, crate::pi::doctor::Level::Fail);
            assert!(denied.detail.contains("execution_boundary_unavailable:"));
            assert!(denied.detail.contains("new lanes use advisory execution"));
            let ready_world = crate::scenarios::World::new();
            ready_world
                .runner
                .on("/usr/bin/bwrap", crate::runner::fake::ok(""));
            ready_world
                .runner
                .on("ade-boundary-probe.mjs", pi_execution_fixture());
            let ready = execution_row(&ready_world.ctx(), "pi", backend, "allowed");
            assert_eq!(ready.level, crate::pi::doctor::Level::Ok);
            assert!(ready.detail.contains(
                "namespace, bounded Pi CLI tool and authorized publication probes passed"
            ));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn doctor_fails_and_names_missing_or_host_backed_boundary_tools() {
        for name in ["bash", "read", "write", "edit", "ade"] {
            let world = crate::scenarios::World::new();
            world.runner.on("/usr/bin/bwrap", ok(""));
            let mut output = pi_execution_fixture();
            // Same name is insufficient: built-in file/shell tools are unsafe.
            output.stdout = output.stdout.replace(
                &format!("\"name\":\"{name}\",\"source\":{{\"path\":\"/runtime/ade-boundary-probe.mjs\"}}"),
                &format!("\"name\":\"{name}\",\"source\":{{\"path\":\"builtin:{name}\"}}"),
            );
            world.runner.on("ade-boundary-probe.mjs", output);
            let row = execution_row(
                &world.ctx(),
                "pi",
                crate::launch::EXECUTION_BACKEND,
                "allowed",
            );
            assert_eq!(row.level, crate::pi::doctor::Level::Fail);
            assert!(
                row.detail
                    .contains(&format!("missing boundary tools: {name};")),
                "{}",
                row.detail
            );
        }
        let mut extra = pi_execution_fixture();
        extra.stdout = extra.stdout.replace(
            "]",
            ",{\"name\":\"find\",\"source\":{\"path\":\"builtin:find\"}}]",
        );
        assert!(
            pi_execution_probe_result(&extra)
                .unwrap_err()
                .to_string()
                .contains("unexpected tools")
        );
    }

    #[test]
    fn native_only_routes_do_not_run_local_pi_setup_checks() {
        let fx = crate::testkit::fixture();
        let config = fx.world.ctx().config_dir.clone();
        write_routing_config(&config);
        let path = config.join("config.toml");
        let mut settings: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        settings["routing"] = toml::Value::Table(toml::Table::from_iter([
            (
                "default".into(),
                toml::Value::String("agy_gemini_flash".into()),
            ),
            ("retries".into(), toml::Value::Integer(1)),
        ]));
        std::fs::write(&path, toml::to_string(&settings).unwrap()).unwrap();
        let outcome = run(&fx.world.ctx(), &SessionFlags::default()).unwrap();
        for label in [
            "node",
            "npm",
            "pi version",
            "wrapper on PATH",
            "pi folder",
            "providers",
        ] {
            assert!(
                !outcome.checks.iter().any(|row| row.label == label),
                "{label}: {}",
                outcome.message
            );
        }
        assert!(
            !fx.world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|call| call.program == "/bin/bash")
        );
        std::fs::write(path, "[routing]\ndefault = 'absent'\n").unwrap();
        let outcome = run(&fx.world.ctx(), &SessionFlags::default()).unwrap();
        assert!(!outcome.healthy);
        assert!(outcome.checks.iter().any(
            |row| row.status == "failed" && row.detail.contains("pi recipe selection unknown")
        ));
    }

    #[test]
    fn selected_adapters_need_only_their_own_executables_not_host_inventory() {
        for kinds in [vec!["claude"], vec!["pi"]] {
            let config = machine_config(&kinds);
            let runner = FakeRunner::new();
            runner.on("ssh", ok(&box_facts()));
            let rows = box_rows_with_snapshot(
                &runner,
                config.path(),
                &box_profile(),
                &default_recipes(),
                12.0,
                None,
            );
            assert!(
                rows.iter().all(|row| row.0 != Some(false)),
                "{kinds:?}: {rows:?}"
            );
            let request = crate::box_helper::tests::doctor_input(
                runner.calls.borrow()[0].stdin.as_ref().unwrap(),
            );
            assert_eq!(!request.models.is_empty(), kinds == ["pi"]);
            assert_eq!(!request.natives.is_empty(), kinds == ["claude"]);
            let calls = runner.calls.borrow();
            let call = calls.iter().find(|call| call.program == "ssh").unwrap();
            let script = call.args.join(" ");
            assert!(script.contains("HERDR_ADE_BOX_INPUT"));
            assert!(!script.contains("command -v"));
            for excluded in ["routing", "credentials", "auth.json", "config.toml"] {
                assert!(!call.stdin.as_ref().unwrap().contains(excluded));
            }
        }
    }

    #[test]
    fn box_disk_floor_still_fails_and_missing_space_stays_unknown() {
        for (level, expected) in [
            (crate::pi::doctor::Level::Fail, Some(false)),
            (crate::pi::doctor::Level::Ok, Some(true)),
            (crate::pi::doctor::Level::Warn, None),
        ] {
            let runner = FakeRunner::new();
            let mut report = box_report();
            report
                .rows
                .iter_mut()
                .find(|row| row.label == "disk")
                .unwrap()
                .level = level;
            runner.on("ssh", ok(&crate::box_helper::tests::ready(&report)));
            let row = find_row(&runner, "box buildbox disk");
            assert_eq!(row.0, expected, "{row:?}");
            assert!(!row.2.contains("lane(s) fit"));
        }
    }

    #[test]
    fn box_wrapper_probe_fails_closed_when_the_path_resolves_another_binary() {
        let runner = FakeRunner::new();
        let mut report = box_report();
        let row = report
            .rows
            .iter_mut()
            .find(|row| row.label == "wrapper on PATH")
            .unwrap();
        row.level = crate::pi::doctor::Level::Fail;
        row.detail = "first hit /usr/local/bin/pi".into();
        runner.on("ssh", ok(&crate::box_helper::tests::ready(&report)));
        let row = find_row(&runner, "box buildbox wrapper on PATH");
        assert_eq!(row.0, Some(false));
        assert!(row.2.contains("/usr/local/bin/pi"), "{}", row.2);
    }

    fn find_row(runner: &FakeRunner, label: &str) -> (Option<bool>, String, String) {
        let config = machine_config(&["pi"]);
        box_rows_with_snapshot(
            runner,
            config.path(),
            &box_profile(),
            &default_recipes(),
            12.0,
            None,
        )
        .into_iter()
        .find(|(_, name, _)| name == label)
        .unwrap_or_else(|| panic!("no row {label}"))
    }
}
