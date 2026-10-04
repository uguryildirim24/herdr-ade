//! Dispatch resolves work through the editable recipe table.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::contracts::{Launch, Recipe};
use crate::paths::{Ctx, Env};
use crate::project::{self, Project};
use crate::runner::{Cmd, Runner};

pub const HELP_TIMEOUT: Duration = Duration::from_secs(10);
pub const TEMPLATE_PINNED: &str = "You chose {plain} for {job}.";
pub const TEMPLATE_USUAL: &str = "{job} runs on {plain}, chosen for this work.";
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DispatchConfig {
    pub machine: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DoctorConfig {
    pub min_free_disk_gb: f64,
}

impl Default for DoctorConfig {
    fn default() -> Self {
        Self {
            min_free_disk_gb: 12.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchConfig {
    pub recipes: BTreeMap<String, Recipe>,
    pub adapters: BTreeMap<String, crate::adapters::Adapter>,
    pub dispatch: DispatchConfig,
    pub routing: crate::routing::Routing,
    pub doctor: DoctorConfig,
    pub policy_hash: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    recipes: BTreeMap<String, Recipe>,
    dispatch: DispatchConfig,
    routing: crate::routing::Routing,
    doctor: DoctorConfig,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct DoctorOnlyConfig {
    doctor: DoctorConfig,
}

fn validate_doctor_config(config: &DoctorConfig) -> Result<()> {
    if !config.min_free_disk_gb.is_finite() || config.min_free_disk_gb < 0.0 {
        bail!("doctor_min_free_disk_invalid: [doctor].min_free_disk_gb must be zero or greater");
    }
    Ok(())
}

pub fn doctor_config(config_dir: &Path) -> Result<DoctorConfig> {
    let document = crate::config::Document::read(config_dir)?;
    let raw: DoctorOnlyConfig = document.decode()?;
    validate_doctor_config(&raw.doctor)?;
    Ok(raw.doctor)
}

pub fn parse_launch_config(config_dir: &Path) -> Result<LaunchConfig> {
    let config = recipe_catalog(config_dir)?;
    config.routing.validate(&config.recipes)?;
    Ok(config)
}

/// Canonical declarations, also used during pi setup before routes are set.
pub(crate) fn recipe_catalog(config_dir: &Path) -> Result<LaunchConfig> {
    let document = crate::config::Document::read(config_dir)?;
    let raw: RawConfig = document.decode()?;
    validate_doctor_config(&raw.doctor)?;
    let defaults: RawConfig = toml::from_str(include_str!("../assets/default-recipes.toml"))
        .context("shipped recipe declarations do not parse")?;
    let mut recipes = defaults.recipes;
    // A configured row is complete and replaces the shipped data row.
    recipes.extend(raw.recipes);
    let adapters = crate::adapters::declarations_from(&document)?;
    let policy_hash = crate::thread::sha256_hex(
        serde_json::to_vec(&(&recipes, &adapters, &raw.dispatch, &raw.routing))?.as_slice(),
    );
    Ok(LaunchConfig {
        recipes,
        adapters,
        dispatch: raw.dispatch,
        routing: raw.routing,
        doctor: raw.doctor,
        policy_hash,
    })
}

pub fn validate_config(config: &LaunchConfig, kinds: &BTreeSet<String>) -> Result<()> {
    for (id, recipe) in &config.recipes {
        if !kinds.contains(recipe.kind.trim()) {
            bail!("recipe_kind_unknown: {id}: {}", recipe.kind);
        }
        let adapter = config
            .adapters
            .get(&recipe.kind)
            .with_context(|| format!("adapter_unknown: recipe `{id}` uses `{}`", recipe.kind))?;
        crate::adapters::validate_recipe(adapter, id, recipe)?;
    }
    Ok(())
}

/// One compact context row per configured recipe. Command syntax lives once in
/// the coordinator skill; this view only says what the recipe is and what
/// selects it.
pub fn context_recipe_lines(config: &LaunchConfig) -> Vec<String> {
    config
        .recipes
        .iter()
        .map(|(id, recipe)| {
            let capabilities = if recipe.capabilities.is_empty() {
                "none".to_string()
            } else {
                recipe.capabilities.join(",")
            };
            if !recipe.enabled {
                return format!(
                    "- {id} [disabled] {} — capabilities={capabilities}",
                    recipe.plain
                );
            }
            let mut reach = Vec::new();
            if config.routing.default == *id {
                reach.push("default".to_string());
            }
            for (index, rule) in config.routing.rules.iter().enumerate() {
                if rule.recipe != *id {
                    continue;
                }
                let mut trigger = Vec::new();
                if let Some(workflow) = &rule.workflow {
                    trigger.push(format!("--workflow {workflow}"));
                }
                let mut front = Vec::new();
                if let Some(product) = &rule.product {
                    front.push(format!("product = \"{product}\""));
                }
                if let Some(capability) = &rule.capability {
                    front.push(format!("capability = \"{capability}\""));
                }
                if !front.is_empty() {
                    trigger.push(format!("brief front matter: {}", front.join(", ")));
                }
                reach.push(format!("rule[{index}] {}", trigger.join("; ")));
            }
            reach.push("coordinator's one-off lane choice".into());
            format!(
                "- {id} [enabled] {} — capabilities={capabilities}; reach: {}",
                recipe.plain,
                reach.join(" | ")
            )
        })
        .collect()
}

/// Resolve `request:<id>` or `request:<project>/<id>` for a project-level
/// coordinator recipe choice. The portable, project-qualified form is retained
/// on the coordinator launch record.
pub fn authorize_coordinator_recipe(project: &Project, basis: &str) -> Result<String> {
    crate::prompt::resolve_request(project, basis)
        .map(|request| request.qualified_basis())
        .map_err(|error| {
            crate::refusal::error(
                error.to_string(),
                format!(
                    "ha open {} --recipe <id> --basis request:<id>",
                    project.slug
                ),
            )
        })
}

/// Optional task front matter describes the deliverable or a hard runtime
/// requirement, not a model preference. URLs in the body never trigger a rule.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TaskContract {
    product: String,
    capability: Option<String>,
    once: bool,
}

pub fn work_contract(task: &str, workflow: &str) -> Result<crate::routing::WorkContract> {
    let contract = if workflow == "coordinator" {
        TaskContract::default()
    } else if let Some(rest) = task.strip_prefix("+++\n") {
        let (front, _) = rest
            .split_once("\n+++\n")
            .context("task_contract: unclosed front matter")?;
        toml::from_str::<TaskContract>(front).context("task_contract: describe product and capability; model/recipe/role overrides are forbidden")?
    } else {
        TaskContract::default()
    };
    if !matches!(
        contract.product.as_str(),
        "" | "code" | "web-research" | "spec"
    ) {
        bail!("task_contract: product must be code, web-research or spec");
    }
    Ok(crate::routing::WorkContract {
        workflow: workflow.to_string(),
        product: contract.product,
        capability: contract.capability,
        once: contract.once,
    })
}

#[derive(Debug, Default)]
pub struct ResolveInput<'a> {
    pub task: &'a str,
    /// Stable job identity for specific initial-selection refusals.
    pub task_id: Option<&'a str>,
    /// Selects skill text and an ordered routing rule.
    pub workflow: &'a str,
    /// One recipe the coordinator chose for this lane. Ordinary starts leave this empty.
    pub recipe: Option<&'a str>,
    /// A recipe selected for the project's coordinator. Unlike a lane's
    /// one-off choice, it is retained on the coordinator binding itself.
    pub project_recipe: Option<&'a str>,
    /// Coordinator recipe basis and request, persisted on its launch record.
    /// Lanes leave both empty.
    pub recipe_basis: Option<&'a str>,
    pub recipe_request: Option<&'a str>,
    pub previous: Option<&'a Launch>,
    pub failure: Option<&'a str>,
    /// Evidence omitted by an upstream task builder, already disclosed in the
    /// brief; copied into the launch record and dispatch journal.
    pub source_truncation: Option<&'a Value>,
}

pub fn resolve_launch(ctx: &Ctx, project: &Project, input: &ResolveInput) -> Result<Launch> {
    let result = resolve(ctx, project, input);
    if let Err(error) = &result {
        dispatch(
            project,
            json!({"kind":"dispatch-refused", "brief_hash":crate::thread::sha256_hex(input.task.as_bytes()),
            "failure":input.failure, "source_truncation":input.source_truncation,
            "error":format!("{error:#}")}),
        )?;
    }
    result
}

/// Apply the recovery policy without turning infrastructure evidence into
/// failed work. Provider and connection failures get bounded same-recipe
/// retries; a gone process restarts. Failed work retries its original recipe.
pub fn resolve_failure(
    ctx: &Ctx,
    project: &Project,
    input: &ResolveInput<'_>,
    class: crate::contracts::FailureClass,
) -> Result<Launch> {
    same_recipe_retry(ctx, project, input, class, "recovery", true)
}

/// A coordinator's explicit retry keeps the recipe and records the reason,
/// even after automatic retries are exhausted. Unknown evidence never chooses
/// another recipe; automatic recovery still stops at its configured limit.
pub fn resolve_coordinator_retry(
    ctx: &Ctx,
    project: &Project,
    input: &ResolveInput<'_>,
    class: crate::contracts::FailureClass,
) -> Result<Launch> {
    same_recipe_retry(ctx, project, input, class, "coordinator-retry", false)
}

fn same_recipe_retry(
    ctx: &Ctx,
    project: &Project,
    input: &ResolveInput<'_>,
    class: crate::contracts::FailureClass,
    dispatch_kind: &str,
    automatic: bool,
) -> Result<Launch> {
    use crate::contracts::FailureClass;
    let previous = input.previous.context("recovery_previous_missing")?;
    let work = work_contract(input.task, input.workflow)?;
    if automatic && work.once {
        return Err(crate::refusal::error(
            format!(
                "recovery_exhausted: {} runs once; attempt {} ended ({}) and is not retried automatically",
                job_noun(input.workflow),
                previous.attempt.max(1),
                class.plain()
            ),
            crate::threads::retry_command(&project.slug, "<thread>"),
        ));
    }
    if automatic && class == FailureClass::Unknown {
        return Err(crate::refusal::error(
            "recovery_unknown: waiting for the coordinator",
            "wait for the coordinator to classify the failure; a classified failure event clears this refusal",
        ));
    }
    if input
        .failure
        .is_none_or(|failure| failure.trim().is_empty())
    {
        bail!("recovery_failure_missing");
    }
    // Recovery uses only the editable retry policy, not today's recipe selection
    // or this machine's kinds. Placement checks the stored launch where it runs.
    let config = recipe_catalog(&ctx.config_dir)?;
    let mut same = previous.clone();
    let work_failed = class == FailureClass::WorkFailed;
    let counter = if work_failed {
        &mut same.work_retries
    } else {
        &mut same.same_recipe_retries
    };
    let recovery = counter.saturating_add(1);
    let retries = if work_failed && previous.routing_rule == "explicit" {
        config.routing.retries
    } else {
        config.routing.retry_limit(&work)
    };
    if automatic && recovery > retries {
        return Err(crate::refusal::error(
            format!(
                "recovery_exhausted: {} allowed {retries} same-recipe retries; waiting for the coordinator",
                if work_failed {
                    previous.routing_rule.as_str()
                } else {
                    class.plain()
                }
            ),
            crate::threads::retry_command(&project.slug, "<thread>"),
        ));
    }
    *counter = recovery;
    dispatch(
        project,
        json!({"kind":dispatch_kind, "class":class, "recipe":same.recipe_id,
            "work_retry":same.work_retries, "same_recipe_retry":same.same_recipe_retries,
            "rule":same.routing_rule, "failure":input.failure,
            "policy_hash":config.policy_hash}),
    )?;
    Ok(same)
}

fn resolve(ctx: &Ctx, project: &Project, input: &ResolveInput) -> Result<Launch> {
    if input.task.trim().is_empty() {
        return Err(crate::refusal::error(
            "dispatch_brief_missing: supply the full task file",
            format!(
                "ha thread start {} --job <job> --task-file <complete-task-file>",
                project.slug
            ),
        ));
    }
    let config = parse_launch_config(&ctx.config_dir)?;
    validate_config(&config, &agent_kinds(ctx.env, ctx.runner)?)?;
    let work = work_contract(input.task, input.workflow)?;
    let hash = crate::thread::sha256_hex(input.task.as_bytes());
    let selected = match (input.project_recipe, input.recipe) {
        (Some(recipe), None) => crate::routing::Selection {
            recipe: recipe.to_string(),
            rule: "project".into(),
            pinned: true,
        },
        (None, Some(recipe)) => crate::routing::Selection {
            recipe: recipe.to_string(),
            rule: "explicit".into(),
            pinned: true,
        },
        (None, None) => config.routing.select(&hash, &work, 0)?,
        (Some(_), Some(_)) => bail!("recipe_choice_ambiguous"),
    };
    let recipe = config
        .recipes
        .get(&selected.recipe)
        .with_context(|| format!("routing_recipe_unknown: {}", selected.recipe))?;
    let retry_start = format!(
        "ha thread start {} --job {} --task-file <file>",
        project.slug,
        input.task_id.unwrap_or("<job>")
    );
    if !recipe.enabled {
        return Err(crate::refusal::error(
            format!("routing_recipe_disabled: {}", selected.recipe),
            retry_start,
        ));
    }
    if let Some(capability) = &work.capability
        && !recipe.capabilities.contains(capability)
    {
        return Err(crate::refusal::error(
            format!(
                "routing_capability_missing: recipe `{}` does not declare `{capability}`",
                selected.recipe
            ),
            retry_start,
        ));
    }
    if parse_launch_config(&ctx.config_dir)?.policy_hash != config.policy_hash {
        return Err(crate::refusal::error(
            "dispatch_policy_changed: config changed during selection; dispatch again",
            retry_start,
        ));
    }
    dispatch(
        project,
        json!({"kind":"pick",
        "brief_hash": hash, "recipe": selected.recipe, "rule": selected.rule,
        "workflow": input.workflow, "basis": input.recipe_basis,
        "request": input.recipe_request,
        "source_truncation": input.source_truncation,
        "policy_hash": config.policy_hash}),
    )?;
    Ok(Launch {
        kind: recipe.kind.clone(),
        args: if config.adapters[&recipe.kind].doctor.readiness == "pi" {
            crate::pi::launch::parse_args(&recipe.args)?.argv()
        } else {
            crate::adapters::launch_args(
                config
                    .adapters
                    .get(&recipe.kind)
                    .context("adapter_unknown")?,
                recipe,
            )
        },
        env: {
            let mut env: Vec<_> = recipe
                .env
                .iter()
                .filter(|value| !value.starts_with("HERDR_ADE_EXECUTION="))
                .cloned()
                .collect();
            // Pin new lane launches only. Recovery preserves the stored env;
            // installing this build does not retrofit a running process.
            if input.workflow != "coordinator" {
                let backend =
                    crate::adapters::recipe_execution(&config.adapters[&recipe.kind], recipe);
                if backend == EXECUTION_BACKEND || recipe.execution == "advisory" {
                    env.push(format!("HERDR_ADE_EXECUTION={backend}"));
                }
            }
            env
        },
        execution: recipe.execution.clone(),
        network: recipe.network.clone(),
        ready_timeout_ms: if recipe.ready_timeout_ms == 0 {
            config.adapters[&recipe.kind].ready_timeout_ms
        } else {
            recipe.ready_timeout_ms
        },
        policy_hash: config.policy_hash,
        attempt: 1,
        recipe_id: selected.recipe,
        routing_rule: selected.rule,
        recipe_basis: input.recipe_basis.unwrap_or_default().to_string(),
        recipe_request: input.recipe_request.unwrap_or_default().to_string(),
        reason: if selected.pinned {
            pinned_reason(input.workflow, &recipe.plain)
        } else {
            usual_reason(input.workflow, &recipe.plain)
        },
        source_truncation: input.source_truncation.cloned(),
        machine: config.dispatch.machine,
        ..Launch::default()
    })
}

pub(crate) const EXECUTION_BACKEND: &str = "linux-bwrap-tools-v1";

pub(crate) fn execution_requested(launch: &Launch) -> bool {
    launch
        .env
        .iter()
        .any(|value| value == &format!("HERDR_ADE_EXECUTION={EXECUTION_BACKEND}"))
}

pub(crate) fn execution_network(launch: &Launch) -> &str {
    if launch.network == "allowed" {
        "allowed"
    } else {
        "denied"
    }
}

pub(crate) fn execution_description(backend: &str, os: &str, network: &str) -> String {
    if backend == EXECUTION_BACKEND && os == "linux" {
        format!(
            "Linux bubblewrap tool boundary (availability probed separately): worktree/private Git/build/cache writable; tool network {network}; provider and authorized seal/publication outside. Trusted Pi and installed extensions are not contained."
        )
    } else {
        format!(
            "advisory: no ADE filesystem/network/credential boundary; permission bypass, hooks and worktrees do not protect the host{}",
            if network == "denied" {
                "; requested network denial cannot be enforced here"
            } else {
                ""
            }
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExecutionNotice {
    None,
    Machine,
    Lane,
}

#[derive(Debug)]
pub(crate) struct ExecutionBinding {
    pub(crate) args: Vec<String>,
    pub(crate) advisory: Option<String>,
    pub(crate) notice: ExecutionNotice,
}

/// Apply inside the thread update's project lock. Existing durable lane notices
/// are the outbox: retain one warning per project/machine, even after receipt
/// or across concurrent first launches, rather than flooding every new lane.
pub(crate) fn apply_execution(
    project: &Project,
    lane: &mut crate::thread::Thread,
    binding: &ExecutionBinding,
) {
    lane.launch.args = binding.args.clone();
    if let Some(reason) = &binding.advisory {
        lane.launch
            .env
            .retain(|value| !value.starts_with("HERDR_ADE_EXECUTION="));
        lane.launch.env.push("HERDR_ADE_EXECUTION=advisory".into());
        let annotation = format!("; execution advisory: {reason}");
        if !lane.placement_reason.ends_with(&annotation) {
            lane.placement_reason.push_str(&annotation);
        }
        if binding.notice == ExecutionNotice::None {
            return;
        }
        let key = if lane.machine_route().is_empty() {
            "local"
        } else {
            lane.machine_route()
        };
        let prefix = if binding.notice == ExecutionNotice::Lane {
            format!("EXECUTION {}/{} advisory:", lane.id, lane.attempt.max(1))
        } else {
            format!("EXECUTION {key} advisory:")
        };
        let heard = crate::thread::list(project).iter().any(|other| {
            other
                .start_notices
                .iter()
                .any(|notice| notice.line.starts_with(&prefix))
        });
        if !heard {
            lane.start_notices.push(crate::steps::Notice {
                line: format!(
                    "{prefix} {}: {reason}",
                    if lane.machine.is_empty() {
                        "local"
                    } else {
                        &lane.machine
                    }
                ),
                submitted: false,
            });
        }
    }
}

/// Freeze the backend outside writable work. Remote writes use the same
/// hash-verified runtime-file transport as briefs, not a credential transfer.
/// Arguments are stored on the launch, so retries retain their chosen mode.
#[cfg(test)]
pub(crate) fn bind_execution(
    ctx: &Ctx,
    record: &crate::thread::Thread,
    machine: Option<&crate::remote::MachineDeclaration>,
) -> Result<ExecutionBinding> {
    bind_execution_with_evidence(ctx, record, machine, &[])
}

/// Only packet-pinned member reports and this lane's frozen inputs are exposed.
/// Remote report paths match the packet's root substitution in freeze_start.
pub(crate) fn execution_evidence(
    project: &Project,
    record: &crate::thread::Thread,
    root: &str,
) -> Result<Vec<Value>> {
    let mut files = BTreeMap::new();
    if !record.launch.brief_hash.is_empty() {
        files.insert(
            format!("{}/brief.md", record.thread_dir),
            record.launch.brief_hash.clone(),
        );
    }
    for (name, hash) in &record.attachments {
        files.insert(
            format!("{}/attachments/{name}", record.thread_dir),
            hash.clone(),
        );
    }
    if record.role == "reviewer" && !record.review_id.is_empty() {
        for member in crate::review::load(project, &record.review_id)?.members {
            // Verify producer-side immutable bytes before freezing the allowlist.
            crate::thread::artifact(project, &member.artifact)?;
            files.insert(
                format!(
                    "{root}/{}/.state/artifacts/{}",
                    project.slug, member.artifact
                ),
                member.artifact,
            );
        }
    }
    Ok(files
        .into_iter()
        .map(|(path, hash)| json!({"path": path, "hash": hash}))
        .collect())
}

pub(crate) fn bind_execution_with_evidence(
    ctx: &Ctx,
    record: &crate::thread::Thread,
    machine: Option<&crate::remote::MachineDeclaration>,
    evidence: &[Value],
) -> Result<ExecutionBinding> {
    if record.launch.execution == "advisory" {
        anyhow::ensure!(
            !record
                .launch
                .args
                .iter()
                .any(|arg| arg == "--no-extensions")
                || !execution_requested(&record.launch),
            "execution_boundary_unavailable: a stored bounded launch cannot downgrade to advisory"
        );
        return Ok(ExecutionBinding {
            args: record.launch.args.clone(),
            advisory: Some(format!(
                "recipe {} explicitly requests host execution; no filesystem/network/credential boundary",
                record.launch.recipe_id
            )),
            notice: ExecutionNotice::Lane,
        });
    }
    if !execution_requested(&record.launch) {
        return Ok(ExecutionBinding {
            args: record.launch.args.clone(),
            advisory: None,
            notice: ExecutionNotice::None,
        });
    }
    let observation =
        crate::doctor::lane_execution(ctx, machine, execution_network(&record.launch))?;
    if observation.level != crate::pi::doctor::Level::Ok {
        anyhow::ensure!(
            !record
                .launch
                .args
                .iter()
                .any(|arg| arg == "--no-extensions"),
            "execution_boundary_unavailable: a stored bounded launch cannot downgrade to an advisory host: {}",
            observation.detail
        );
        return Ok(ExecutionBinding {
            args: record.launch.args.clone(),
            advisory: Some(observation.detail),
            notice: if observation.level == crate::pi::doctor::Level::Fail {
                ExecutionNotice::Machine
            } else {
                ExecutionNotice::None
            },
        });
    }
    let root = machine.map_or_else(
        || std::path::absolute(&ctx.root).map(|root| root.display().to_string()),
        |m| Ok(m.root.clone()),
    )?;
    let ade = machine.map_or_else(
        || std::env::current_exe().map(|p| p.display().to_string()),
        |m| Ok(m.ade_bin.clone()),
    )?;
    let identity = crate::thread::sha256_hex(
        format!(
            "{}\0{}\0{}\0{}",
            root,
            record.id,
            record.attempt.max(1),
            record.worktree_path
        )
        .as_bytes(),
    );
    let state = format!("{root}/.execution/{identity}");
    anyhow::ensure!(
        !Path::new(&state).starts_with(&record.worktree_path),
        "execution_root_exposed: runtime state must be outside the writable worktree"
    );
    let policy = json!({"root": root, "ade": ade, "cwd": record.worktree_path, "branch": record.branch, "role": record.role, "state": state, "network": execution_network(&record.launch), "evidence": evidence});
    let source = include_str!("../assets/pi-execution-boundary.mjs")
        .replace("__ADE_EXECUTION_POLICY__", &serde_json::to_string(&policy)?);
    let hash = crate::thread::sha256_hex(source.as_bytes());
    let path = format!("{state}/{hash}.mjs");
    if let Some(machine) = machine {
        crate::remote::write_runtime_file(
            ctx.runner,
            &machine.target,
            &path,
            source.as_bytes(),
            &hash,
        )?;
    } else {
        std::fs::create_dir_all(&state)?;
        std::fs::write(&path, source)?;
    }
    // These flags cannot originate in a Pi recipe. A retry replaces only the
    // prior backend suffix with this attempt's immutable backend and state.
    let mut args = record.launch.args.clone();
    if let Some(index) = args.iter().position(|arg| arg == "--no-extensions") {
        args.truncate(index);
    }
    args.extend(bounded_pi_args(&root, &state, &path));
    Ok(ExecutionBinding {
        args,
        advisory: None,
        notice: ExecutionNotice::None,
    })
}

/// Shared by real launches and the real-CLI doctor/contract probe. In Pi 0.99.1
/// --no-tools is an EMPTY ALLOWLIST, not merely an inactive initial loadout.
/// --no-builtin-tools leaves extension tools activatable and host tools inactive.
pub(crate) fn bounded_pi_args(root: &str, state: &str, path: &str) -> Vec<String> {
    let mut args = vec![
        "--no-extensions".into(),
        "--no-builtin-tools".into(),
        "--no-approve".into(),
        "--no-prompt-templates".into(),
        "--no-themes".into(),
        "--session-dir".into(),
        format!("{state}/sessions"),
        "--extension".into(),
        path.into(),
    ];
    for extension in ["herdr-agent-state.ts", "herdr-pi-guard.ts"] {
        args.extend([
            "--extension".into(),
            format!("{root}/pi/agent/extensions/{extension}"),
        ]);
    }
    args
}

/// Append-only dispatch journal. No network call is made while holding its lock.
pub fn dispatch(project: &Project, mut row: Value) -> Result<()> {
    let _lock = project.lock()?;
    row["at"] = json!(project::now());
    std::fs::create_dir_all(project.state_dir())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(project.state_dir().join("dispatch.jsonl"))?;
    writeln!(file, "{row}")?;
    file.sync_all()?;
    Ok(())
}

/// Workflow labels may match routing rules. Selection happens only once
/// each complete side's brief exists; no model pin is printed by this command.
pub struct DoctorRow {
    pub ok: Option<bool>,
    pub label: String,
    pub detail: String,
}
pub fn doctor_rows(ctx: &Ctx, config: &LaunchConfig) -> Result<Vec<DoctorRow>> {
    let valid = validate_recipe_reachability(config)
        .and_then(|_| validate_config(config, &agent_kinds(ctx.env, ctx.runner)?));
    Ok(vec![DoctorRow {
        ok: Some(valid.is_ok()),
        label: "recipes and routing".into(),
        detail: valid
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_else(|| {
                format!(
                    "{} recipes; editable routing table and recipe commands are valid",
                    config.recipes.len()
                )
            }),
    }])
}

fn validate_recipe_reachability(config: &LaunchConfig) -> Result<()> {
    for (id, recipe) in config.recipes.iter().filter(|(_, recipe)| recipe.enabled) {
        let routed = config.routing.default == *id
            || config.routing.rules.iter().any(|rule| rule.recipe == *id);
        // Every declared adapter can be reached by Rolf's guarded one-off
        // `thread start --recipe` command.
        let commanded = config.adapters.contains_key(&recipe.kind);
        if !routed && !commanded {
            bail!(
                "recipe_unreachable: enabled recipe `{id}` has no default or rule and no command can reach it"
            );
        }
    }
    Ok(())
}
/// The kinds `herdr agent start` accepts, read from its `--help`.
pub fn agent_kinds(env: &Env, runner: &dyn Runner) -> Result<BTreeSet<String>> {
    let bin = env.herdr_bin();
    let output = runner
        .run(&Cmd::new(&bin, HELP_TIMEOUT).args(["agent", "start", "--help"]))
        .with_context(|| format!("could not run `{bin} agent start --help`"))?;
    parse_kinds(&output.stdout).context("`herdr agent start --help` did not list kinds")
}

/// Which kinds `herdr agent start --help` lists.
pub fn parse_kinds(help: &str) -> Option<BTreeSet<String>> {
    let marker = "[possible values:";
    let start = help.find(marker)? + marker.len();
    let rest = &help[start..];
    let end = rest.find(']')?;
    let kinds: BTreeSet<String> = rest[..end]
        .split(',')
        .map(|kind| kind.trim().to_string())
        .filter(|kind| !kind.is_empty())
        .collect();
    if kinds.is_empty() { None } else { Some(kinds) }
}

pub fn job_noun(role: &str) -> &'static str {
    match role {
        "reviewer" => "this review",
        "critic" => "this second opinion",
        "drafter" => "this draft",
        "research" => "this lookup",
        "planner" => "this plan",
        _ => "this task",
    }
}

fn render(template: &str, job: &str, plain: &str) -> String {
    template.replace("{job}", job).replace("{plain}", plain)
}

pub fn pinned_reason(role: &str, plain: &str) -> String {
    render(TEMPLATE_PINNED, job_noun(role), plain)
}

pub fn usual_reason(role: &str, plain: &str) -> String {
    render(TEMPLATE_USUAL, job_noun(role), plain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::FailureClass;
    use crate::scenarios::World;

    fn recipe_launch(extra: &str) -> (World, Project, Launch) {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        std::fs::write(world.ctx().config_dir.join("config.toml"), format!(
            "[routing]\ndefault = 'pi_test'\nretries = 1\n[recipes.pi_test]\nkind = 'pi'\nprovider = 'p'\nargs = ['--provider', 'p', '--model', 'm', '--thinking', 'high', '--no-skills']\n{extra}\n")).unwrap();
        let launch = resolve(
            &world.ctx(),
            &project,
            &ResolveInput {
                task: "Build the requested artifact.",
                workflow: "lane",
                ..Default::default()
            },
        )
        .unwrap();
        (world, project, launch)
    }

    #[test]
    fn recipe_network_defaults_allowed_denied_is_frozen_and_legacy_boundaries_stay_denied() {
        let (_, _, online) = recipe_launch("");
        assert_eq!(online.network, "allowed");
        assert_eq!(execution_network(&online), "allowed");
        let (world, project, denied) = recipe_launch("network = 'denied'");
        assert!(execution_requested(&denied));
        assert_eq!(execution_network(&denied), "denied");
        let path = world.ctx().config_dir.join("config.toml");
        let changed = std::fs::read_to_string(&path)
            .unwrap()
            .replace("network = 'denied'", "network = 'allowed'");
        std::fs::write(path, changed).unwrap();
        let resumed = resolve_failure(
            &world.ctx(),
            &project,
            &ResolveInput {
                previous: Some(&denied),
                failure: Some("lost connection"),
                workflow: "lane",
                ..Default::default()
            },
            FailureClass::LostConnection,
        )
        .unwrap();
        assert_eq!(resumed.network, "denied");
        let legacy: Launch = serde_json::from_str("{}").unwrap();
        assert_eq!(execution_network(&legacy), "denied");
    }

    #[test]
    fn recipe_advisory_overrides_adapter_and_has_one_start_notice_without_a_probe() {
        let (world, project, launch) = recipe_launch("execution = 'advisory'");
        assert!(!execution_requested(&launch));
        assert!(launch.env.contains(&"HERDR_ADE_EXECUTION=advisory".into()));
        let record =
            crate::thread::allocate(&project, |lane| lane.launch = launch.clone()).unwrap();
        for _ in 0..2 {
            let binding = bind_execution(&world.ctx(), &record, None).unwrap();
            assert_eq!(binding.args, launch.args);
            crate::thread::update(&project, &record.id, |lane| {
                apply_execution(&project, lane, &binding)
            })
            .unwrap();
        }
        let saved = crate::thread::load(&project, &record.id).unwrap();
        assert_eq!(saved.start_notices.len(), 1);
        assert!(
            saved.start_notices[0]
                .line
                .contains("explicitly requests host execution")
        );
        assert_eq!(world.runner.count("/usr/bin/bwrap"), 0);
        let (_, _, mut bounded) = recipe_launch("");
        bounded.args.push("--no-extensions".into());
        // A newly configured opt-out cannot rewrite a stored bounded retry.
        let resumed = resolve_failure(
            &world.ctx(),
            &project,
            &ResolveInput {
                previous: Some(&bounded),
                failure: Some("lost connection"),
                workflow: "lane",
                ..Default::default()
            },
            FailureClass::LostConnection,
        )
        .unwrap();
        assert!(execution_requested(&resumed));
        assert_eq!(resumed.execution, "");
    }

    #[test]
    fn boundary_launch_is_pinned_outside_work_and_old_launches_stay_unchanged() {
        let world = World::new();
        let mut record = crate::thread::Thread {
            id: "t-1".into(),
            attempt: 1,
            worktree_path: world.home.path().join("work").display().to_string(),
            branch: "lane/one".into(),
            role: "reviewer".into(),
            launch: Launch {
                kind: "pi".into(),
                args: vec!["--provider".into(), "same".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let before = record.launch.clone();
        assert_eq!(
            bind_execution(&world.ctx(), &record, None).unwrap().args,
            before.args
        );
        assert_eq!(record.launch, before);
        record
            .launch
            .env
            .push(format!("HERDR_ADE_EXECUTION={EXECUTION_BACKEND}"));
        if cfg!(target_os = "linux") {
            world
                .runner
                .on("/usr/bin/bwrap", crate::runner::fake::ok(""));
            world.runner.on(
                "ade-boundary-probe.mjs",
                crate::doctor::pi_execution_fixture(),
            );
            let args = bind_execution(&world.ctx(), &record, None).unwrap().args;
            assert!(args.contains(&"--no-builtin-tools".into()));
            assert!(!args.contains(&"--tools".into()));
            assert!(args.contains(&"--no-extensions".into()));
            assert!(args.contains(&"--no-approve".into()));
            let index = args.iter().position(|arg| arg == "--extension").unwrap();
            let path = Path::new(&args[index + 1]);
            assert!(!path.starts_with(&record.worktree_path));
            let source = std::fs::read_to_string(path).unwrap();
            assert!(!source.contains("__ADE_EXECUTION_POLICY__"));
            assert!(source.contains("\"role\":\"reviewer\""));
            record.launch.args = args.clone();
            assert_eq!(
                bind_execution(&world.ctx(), &record, None).unwrap().args,
                args
            );
            record.attempt += 1;
            let retry = bind_execution(&world.ctx(), &record, None).unwrap().args;
            assert_ne!(retry[index + 1], args[index + 1]);
            assert_eq!(
                retry.iter().filter(|arg| *arg == "--no-extensions").count(),
                1
            );
            assert_eq!(std::fs::read_to_string(path).unwrap(), source);
        }
        assert!(
            execution_description(EXECUTION_BACKEND, "macos", "allowed").starts_with("advisory:")
        );
        assert!(execution_description("", "linux", "allowed").starts_with("advisory:"));
    }

    #[test]
    fn reviewer_evidence_allowlist_matches_packet_paths_not_the_control_directory() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let report = crate::thread::store_artifact(&project, b"member report").unwrap();
        let unrelated =
            crate::thread::store_artifact(&project, b"unrelated control evidence").unwrap();
        let review = format!(
            "id = 'review-1'\nrepo = 'repo'\nintegration = 'main'\nbase = 'base'\ngates = []\nselected_gates = []\nphase = 'reviewing'\nverdict_event = ''\nreviewer_after = ''\nchecked_event = ''\nretry_generation = 0\nmoved = 0\ninstall_required = false\nfast_forward = false\npush = false\ninstall = false\nclose = false\nprune = false\nattention = ''\n[[members]]\nthread = 't-1'\nattempt = 1\nevent = 't-1-1-1'\nsha = 'sha'\nbranch = 'lane'\nartifact = '{report}'\n"
        );
        std::fs::create_dir_all(crate::review::dir(&project)).unwrap();
        std::fs::write(crate::review::path(&project, "review-1"), review).unwrap();
        let record = crate::thread::Thread {
            role: "reviewer".into(),
            review_id: "review-1".into(),
            thread_dir: "/work/.herdr-project/demo-t-2".into(),
            attachments: BTreeMap::from([("attachment-hash".into(), "attachment-hash".into())]),
            launch: Launch {
                brief_hash: "brief-hash".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let files = execution_evidence(&project, &record, "/box/root").unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.contains(
            &json!({"path": format!("/box/root/demo/.state/artifacts/{report}"), "hash": report})
        ));
        assert!(files.contains(
            &json!({"path": "/work/.herdr-project/demo-t-2/brief.md", "hash": "brief-hash"})
        ));
        assert!(files.contains(&json!({"path": "/work/.herdr-project/demo-t-2/attachments/attachment-hash", "hash": "attachment-hash"})));
        assert!(!serde_json::to_string(&files).unwrap().contains(&unrelated));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn authorized_publication_probe_failure_keeps_new_lanes_advisory_once() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        world
            .runner
            .on("/usr/bin/bwrap", crate::runner::fake::ok(""));
        let mut output = crate::doctor::pi_execution_fixture();
        output.stdout = output.stdout.replace(
            "ADE_AUTHORIZED_PUBLICATION=passed",
            "ADE_AUTHORIZED_PUBLICATION=failed: credential helper unavailable",
        );
        world.runner.on("ade-boundary-probe.mjs", output);
        for _ in 0..2 {
            let record = crate::thread::allocate(&project, |lane| {
                lane.launch
                    .env
                    .push(format!("HERDR_ADE_EXECUTION={EXECUTION_BACKEND}"));
            })
            .unwrap();
            let binding = bind_execution(&world.ctx(), &record, None).unwrap();
            assert!(
                binding
                    .advisory
                    .as_ref()
                    .unwrap()
                    .contains("authorized publication dry run")
            );
            crate::thread::update(&project, &record.id, |lane| {
                apply_execution(&project, lane, &binding)
            })
            .unwrap();
        }
        assert_eq!(
            crate::thread::list(&project)
                .iter()
                .map(|lane| lane.start_notices.len())
                .sum::<usize>(),
            1
        );
        assert!(
            world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|cmd| cmd.display().contains("--ade-publication-probe"))
        );
    }

    #[test]
    fn unsupported_remote_is_advisory_but_a_stored_boundary_cannot_downgrade() {
        let world = World::new();
        world.runner.on(
            "HERDR_ADE_BOX_INPUT",
            crate::runner::fake::ok(&crate::box_helper::tests::ready(
                serde_json::json!({"rows": [
                crate::pi::doctor::Row::warn("recipe lane execution", "advisory: unsupported OS")
            ], "snapshot": {}}),
            )),
        );
        let machine = crate::remote::MachineDeclaration {
            target: "scratch-mac".into(),
            path: "/usr/bin:/bin".into(),
            ..Default::default()
        };
        let mut record = crate::thread::Thread {
            launch: Launch {
                env: vec![format!("HERDR_ADE_EXECUTION={EXECUTION_BACKEND}")],
                args: vec!["--provider".into(), "same".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            bind_execution(&world.ctx(), &record, Some(&machine))
                .unwrap()
                .args,
            record.launch.args
        );
        record.launch.args.push("--no-extensions".into());
        assert!(
            bind_execution(&world.ctx(), &record, Some(&machine))
                .unwrap_err()
                .to_string()
                .contains("cannot downgrade")
        );
        assert_eq!(world.runner.count("python3"), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_probe_keeps_new_lanes_advisory_with_one_durable_machine_notice() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        world.runner.on(
            "/usr/bin/bwrap",
            crate::runner::fake::fail(1, "user namespaces restricted"),
        );
        let original = vec!["--provider".into(), "same".into(), "--no-approve".into()];
        for n in 0..2 {
            let record = crate::thread::allocate(&project, |lane| {
                lane.launch = Launch {
                    args: original.clone(),
                    env: vec![format!("HERDR_ADE_EXECUTION={EXECUTION_BACKEND}")],
                    ..Default::default()
                };
            })
            .unwrap();
            let binding = bind_execution(&world.ctx(), &record, None).unwrap();
            assert_eq!(binding.args, original);
            assert!(
                binding
                    .advisory
                    .as_ref()
                    .unwrap()
                    .contains("user namespaces restricted")
            );
            let saved = crate::thread::update(&project, &record.id, |lane| {
                apply_execution(&project, lane, &binding)
            })
            .unwrap();
            assert!(
                saved
                    .launch
                    .env
                    .contains(&"HERDR_ADE_EXECUTION=advisory".into())
            );
            assert!(
                saved
                    .placement_reason
                    .contains("user namespaces restricted")
            );
            // Receipt must not cause a new warning, nor may a same-attempt bind
            // silently promote a running advisory launch after OS recovery.
            if n == 0 {
                crate::thread::update(&project, &record.id, |lane| {
                    lane.start_notices[0].submitted = true
                })
                .unwrap();
            }
            let repeat = bind_execution(&world.ctx(), &saved, None).unwrap();
            crate::thread::update(&project, &record.id, |lane| {
                apply_execution(&project, lane, &repeat)
            })
            .unwrap();
        }
        assert_eq!(
            crate::thread::list(&project)
                .iter()
                .map(|lane| lane.start_notices.len())
                .sum::<usize>(),
            1
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_cli_tools_fall_back_once_and_do_not_retrofit_bounded_lanes() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        world
            .runner
            .on("/usr/bin/bwrap", crate::runner::fake::ok(""));
        world.runner.on(
            "ade-boundary-probe.mjs",
            crate::runner::fake::ok("ADE_BOUNDARY_TOOLS=[]"),
        );
        for _ in 0..2 {
            let record = crate::thread::allocate(&project, |lane| {
                lane.launch
                    .env
                    .push(format!("HERDR_ADE_EXECUTION={EXECUTION_BACKEND}"));
                lane.launch.args = vec!["--no-skills".into()];
            })
            .unwrap();
            let binding = bind_execution(&world.ctx(), &record, None).unwrap();
            assert!(
                binding
                    .advisory
                    .as_ref()
                    .unwrap()
                    .contains("bash, read, write, edit, ade")
            );
            assert_eq!(binding.notice, ExecutionNotice::Machine);
            let mut historical = record.clone();
            historical.launch.args.push("--no-extensions".into());
            assert!(
                bind_execution(&world.ctx(), &historical, None)
                    .unwrap_err()
                    .to_string()
                    .contains("cannot downgrade")
            );
            let saved = crate::thread::update(&project, &record.id, |lane| {
                apply_execution(&project, lane, &binding)
            })
            .unwrap();
            assert!(!execution_requested(&saved.launch));
            assert_eq!(saved.launch.args, vec!["--no-skills"]);
        }
        assert_eq!(
            crate::thread::list(&project)
                .iter()
                .map(|lane| lane.start_notices.len())
                .sum::<usize>(),
            1
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn passed_probe_binds_but_a_started_boundary_cannot_downgrade() {
        let world = World::new();
        let denied = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = denied.clone();
        world.runner.on(
            "ade-boundary-probe.mjs",
            crate::doctor::pi_execution_fixture(),
        );
        world.runner.on_fn(
            |cmd| cmd.program == "/usr/bin/bwrap",
            move |_| {
                Ok(if signal.load(std::sync::atomic::Ordering::Relaxed) {
                    crate::runner::fake::fail(1, "namespace denied")
                } else {
                    crate::runner::fake::ok("")
                })
            },
        );
        let mut record = crate::thread::Thread {
            id: "t-0001".into(),
            worktree_path: world.home.path().join("work").display().to_string(),
            launch: Launch {
                args: vec!["--no-approve".into()],
                env: vec![format!("HERDR_ADE_EXECUTION={EXECUTION_BACKEND}")],
                ..Default::default()
            },
            ..Default::default()
        };
        let binding = bind_execution(&world.ctx(), &record, None).unwrap();
        assert!(binding.advisory.is_none());
        assert_eq!(binding.notice, ExecutionNotice::None);
        assert!(binding.args.contains(&"--no-extensions".into()));
        record.launch.args = binding.args;
        record.launch_attempts = 1;
        denied.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            bind_execution(&world.ctx(), &record, None)
                .unwrap_err()
                .to_string()
                .contains("cannot downgrade")
        );
        assert!(record.launch.args.contains(&"--no-extensions".into()));
        assert!(execution_requested(&record.launch));
    }

    #[test]
    fn real_pi_cli_bounded_launch_contract() {
        use crate::runner::{RealRunner, Runner};
        match std::process::Command::new("pi").arg("--version").output() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Bypass libtest's successful-test capture: a skipped real CLI
                // witness must remain visible even under `cargo test -q`.
                writeln!(
                    std::io::stderr().lock(),
                    "SKIP real_pi_cli_bounded_launch_contract: pi is absent from PATH"
                )
                .unwrap();
                return;
            }
            Err(error) => panic!("pi exists but could not start: {error}"),
            Ok(output) => assert!(output.status.success(), "pi --version failed"),
        }
        let root = std::env::var_os("HERDR_ADE_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".herdr-ade")
            });
        let mut command = crate::doctor::pi_execution_probe_command(&root);
        // The defect's actual recipe prefix, not an SDK session. The suffix is
        // generated by exactly the same function used in bind_execution.
        command.args[1] = command.args[1].replace(
            "pi --no-skills",
            "pi --provider openai-codex --model gpt-6.1-sol --thinking high --no-skills",
        );
        let output = RealRunner.run(&command).unwrap();
        crate::doctor::pi_execution_probe_result(&output).unwrap_or_else(|error| {
            panic!(
                "{error:#}\nstdout: {}\nstderr: {}",
                output.stdout, output.stderr
            )
        });
        // Reproduce #10's exact CLI mistake: tools cannot be reactivated.
        let mut broken = command;
        for arg in &mut broken.args {
            *arg = arg.replace("--no-builtin-tools", "--no-tools");
        }
        let output = RealRunner.run(&broken).unwrap();
        let error = crate::doctor::pi_execution_probe_result(&output).unwrap_err();
        assert!(error.to_string().contains("bash, read, write, edit, ade"));
    }

    // These run without root when Linux permits unprivileged namespaces. The
    // doctor failure regression covers machines that cannot run this backend;
    // the authorized oci transcript supplies the positive enforcement witness.
    #[cfg(target_os = "linux")]
    fn isolated_trial(code: &str) {
        isolated_trial_network(code, "denied");
    }

    #[cfg(target_os = "linux")]
    fn isolated_trial_network(code: &str, network: &str) {
        use std::process::Command;
        let probe = Command::new("/usr/bin/bwrap")
            .args(["--unshare-all", "--ro-bind", "/", "/", "/usr/bin/true"])
            .output();
        if !probe.is_ok_and(|out| out.status.success())
            || Command::new("node").arg("--version").output().is_err()
        {
            eprintln!("namespace enforcement unavailable here; oci trial required");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let work = dir.path().join("work");
        std::fs::create_dir(&repo).unwrap();
        crate::testkit::git(&repo, &["init", "-q", "-b", "main"]);
        crate::testkit::git(&repo, &["config", "user.name", "Trial"]);
        crate::testkit::git(&repo, &["config", "user.email", "trial@localhost"]);
        crate::testkit::commit_file(&repo, "source.c", "int main(void) { return 0; }\n", "base");
        crate::testkit::git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "lane/trial",
                work.to_str().unwrap(),
            ],
        );
        std::fs::write(repo.join(".git/info/exclude"), ".herdr-project/\n").unwrap();
        let root = dir.path().join("control");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("records"), "protected").unwrap();
        std::fs::write(dir.path().join("credential"), "synthetic-secret").unwrap();
        let evidence = [
            root.join(".state/artifacts/member-report"),
            work.join(".herdr-project/brief.md"),
            work.join(".herdr-project/attachments/member"),
        ];
        for file in &evidence {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "packet-pinned evidence\n").unwrap();
        }
        let host_config = dir.path().join("host.gitconfig");
        std::fs::write(&host_config, "[ade]\ntrial = trusted-host\n").unwrap();
        let ade = dir.path().join("trusted-ade");
        std::fs::write(
            &ade,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ngit config --global --get ade.trial > '{}'\n",
                dir.path().join("authorized-call").display(),
                dir.path().join("authorized-environment").display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ade, std::fs::Permissions::from_mode(0o700)).unwrap();
        let policy = json!({"root": root, "ade": ade, "cwd": work, "branch": "lane/trial", "state": root.join("backend"), "network": network, "evidence": evidence.iter().map(|file| json!({"path":file, "hash":crate::thread::sha256_hex(b"packet-pinned evidence\n")})).collect::<Vec<_>>()});
        let source = include_str!("../assets/pi-execution-boundary.mjs")
            .replace("__ADE_EXECUTION_POLICY__", &policy.to_string())
            .replace("import { Type } from '@sinclair/typebox';", "");
        let backend = dir.path().join("backend.mjs");
        std::fs::write(&backend, source).unwrap();
        let script = dir.path().join("trial.mjs");
        std::fs::write(&script, format!("import assert from 'node:assert/strict';\nimport fs from 'node:fs';\nimport {{runSandbox, seal, policy, validateMutationPath}} from './backend.mjs';\nconst run = command => runSandbox(['/bin/bash', '--noprofile', '--norc', '-c', command]);\n{code}")).unwrap();
        let output = Command::new("node")
            .arg(&script)
            .env("ADE_TRIAL_ROOT", dir.path())
            .env("FAKE_PROVIDER_TOKEN", "must-not-inherit")
            .env("GIT_CONFIG_GLOBAL", &host_config)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(root.join("records")).unwrap(),
            "protected"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn namespace_allows_network_and_writable_private_cache_without_host_access() {
        isolated_trial_network(
            r#"
import net from 'node:net';
const server = net.createServer(socket => socket.end('loopback-ok'));
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
try {
  assert.equal(await run('cat < /dev/tcp/127.0.0.1/' + server.address().port), 'loopback-ok');
  await run('echo cache-ok > "$CARGO_HOME/registry/private-proof"');
  assert.equal(await run('cat "$CARGO_HOME/registry/private-proof"'), 'cache-ok\n');
  await assert.rejects(run('cat "' + process.env.ADE_TRIAL_ROOT + '/credential"'));
  await assert.rejects(run('echo hostile > "' + policy.root + '/records"'));
  await assert.rejects(run('git push "' + process.env.ADE_TRIAL_ROOT + '/repo" HEAD:refs/heads/unauthorized'));
} finally { await new Promise(resolve => server.close(resolve)); }
"#,
            "allowed",
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn namespace_denies_control_and_unrelated_writes_including_subprocesses() {
        isolated_trial(
            r#"
await run('cc source.c -o /build/trial && /build/trial && mkdir -p .herdr-project/library && echo artifact > .herdr-project/library/result');
assert.throws(() => validateMutationPath(policy.root + '/records'), /execution_control_write_denied/);
assert.throws(() => validateMutationPath('../control/records'), /execution_control_write_denied/);
validateMutationPath('.herdr-project/report.md');
for (const target of [policy.root + '/records', process.env.ADE_TRIAL_ROOT + '/unrelated/records']) {
  await assert.rejects(run(`bash -c 'echo hostile > "${target}"'`));
  await assert.rejects(run(`ln -sf '${target}' escape; echo hostile > escape`));
}
await assert.rejects(run('kill -0 ' + process.pid));
assert.equal(await run('cat .herdr-project/library/result'), 'artifact\n');
"#,
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn malformed_binding_cannot_resolve_host_git_from_the_callers_directory() {
        isolated_trial(
            r#"
const broken = fs.readFileSync(new URL('./backend.mjs', import.meta.url), 'utf8')
  .replace('"cwd":' + JSON.stringify(policy.cwd), '"cwd":""');
const backend = await import('data:text/javascript;base64,' + Buffer.from(broken).toString('base64'));
await assert.rejects(backend.runSandbox(['/usr/bin/true']), /execution_policy_invalid/);
"#,
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn namespace_denies_credentials_and_network_including_children() {
        isolated_trial(
            r#"
await assert.rejects(run(`bash -c 'cat "${process.env.ADE_TRIAL_ROOT}/credential"'`));
assert.equal(await run('printf "%s" "${FAKE_PROVIDER_TOKEN-unset}"'), 'unset');
const net = await import('node:net');
const host = net.createServer(socket => socket.end('host-only'));
await new Promise(resolve => host.listen(0, '127.0.0.1', resolve));
try { await assert.rejects(run(`bash -c 'echo send >/dev/tcp/127.0.0.1/${host.address().port}'`)); }
finally { await new Promise(resolve => host.close(resolve)); }
await assert.rejects(run("bash -c 'echo send >/dev/tcp/1.1.1.1/443'"));
"#,
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_git_denies_push_and_shared_metadata_but_authorized_import_works() {
        isolated_trial(
            r#"
await assert.rejects(run(`git push '${process.env.ADE_TRIAL_ROOT}/repo' HEAD:main`));
await assert.rejects(run(`echo hostile > '${process.env.ADE_TRIAL_ROOT}/repo/.git/config'`));
await run('echo changed > change; git add change; git commit -qm change');
await assert.rejects(seal('push', 'not authorized'));
await run('echo untracked > escape');
await assert.rejects(seal('done')); // untracked artifacts must not be silently lost
await run('rm -f escape');
await seal('done');
assert.equal(fs.readFileSync(process.env.ADE_TRIAL_ROOT + '/authorized-call', 'utf8'), '--root\n' + policy.root + '\ndone\n');
assert.equal(fs.readFileSync(process.env.ADE_TRIAL_ROOT + '/authorized-environment', 'utf8'), 'trusted-host\n');
await assert.rejects(run('git config --global --get ade.trial'));
"#,
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn namespace_exposes_only_pinned_evidence_read_only() {
        isolated_trial(
            r#"
for (const {path: file} of policy.evidence) {
  assert.equal(await run('cat "' + file + '"'), 'packet-pinned evidence\n');
  await assert.rejects(run('echo changed > "' + file + '"'));
  await assert.rejects(run('rm "' + file + '"'));
}
await assert.rejects(run('cat "' + policy.root + '/records"'));
await assert.rejects(run('ls "' + policy.root + '/.state/lanes"'));
"#,
        );
    }

    #[test]
    fn recovery_keeps_the_entire_stored_launch_without_initial_validation() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        // Today's selection is invalid and the old recipe declaration changed.
        // Neither may replace or veto a stored launch on another machine.
        std::fs::write(
            world.ctx().config_dir.join("config.toml"),
            "[routing]\ndefault = \"missing\"\nretries = 5\n\n[recipes.test_claude]\nkind = \"missing-kind\"\nenabled = false\nargs = [\"changed\"]\n",
        )
        .unwrap();
        let mut previous = Launch {
            kind: "claude".into(),
            args: vec!["--disallowedTools".into(), "Agent".into()],
            env: vec!["SAVED=1".into()],
            ready_timeout_ms: 42_000,
            policy_hash: "original-policy".into(),
            attempt: 7,
            brief_hash: "original-brief".into(),
            skill_hash: "original-skill".into(),
            recipe_id: "test_claude".into(),
            recipe_basis: "request:demo/q-choice".into(),
            recipe_request: "Rolf's original choice".into(),
            reason: "original selection evidence".into(),
            source_truncation: Some(json!({"omitted":"diff"})),
            machine: "box".into(),
            work_retries: 2,
            same_recipe_retries: 3,
            ..Launch::default()
        };
        for rule in ["default", "rule[0]", "explicit", "pin", "project", ""] {
            previous.routing_rule = rule.into();
            for class in [
                FailureClass::WorkFailed,
                FailureClass::Provider,
                FailureClass::LostConnection,
                FailureClass::ProcessGone,
            ] {
                let input = ResolveInput {
                    task: "Do the original work.",
                    workflow: "lane",
                    previous: Some(&previous),
                    failure: Some("saved failure evidence"),
                    ..Default::default()
                };
                let mut expected = previous.clone();
                if class == FailureClass::WorkFailed {
                    expected.work_retries += 1;
                } else {
                    expected.same_recipe_retries += 1;
                }
                assert_eq!(
                    resolve_failure(&world.ctx(), &project, &input, class).unwrap(),
                    expected
                );
            }
        }
        assert_eq!(world.runner.count("agent start --help"), 0);
        let journal = std::fs::read_to_string(project.state_dir().join("dispatch.jsonl")).unwrap();
        assert!(!journal.contains("\"kind\":\"pick\""));
    }

    #[test]
    fn manual_work_retry_bypasses_once_and_budget_but_requires_a_reason() {
        let world = World::new();
        let project = world.project("demo", "a.sock");
        let previous = Launch {
            work_retries: 8,
            same_recipe_retries: 4,
            routing_rule: "explicit".into(),
            ..Launch::default()
        };
        let mut input = ResolveInput {
            task: "+++\nonce = true\n+++\nRelease once.",
            workflow: "lane",
            previous: Some(&previous),
            failure: Some("verified that the release did not occur"),
            ..Default::default()
        };
        let mut expected = previous.clone();
        expected.work_retries += 1;
        assert_eq!(
            resolve_coordinator_retry(&world.ctx(), &project, &input, FailureClass::WorkFailed)
                .unwrap(),
            expected
        );
        input.failure = Some(" ");
        assert_eq!(
            resolve_coordinator_retry(&world.ctx(), &project, &input, FailureClass::WorkFailed)
                .unwrap_err()
                .to_string(),
            "recovery_failure_missing"
        );
    }
}
