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
pub const COMPACT_LIMIT: usize = 80;
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
    let document = crate::config::Document::read(config_dir)?;
    let value = toml::Value::Table(document.decode::<toml::Table>()?);
    if value.get("roles").is_some() {
        bail!(
            "roles_removed: remove [roles] and every [roles.*] table from config.toml; keep model rows in [recipes.*]"
        );
    }
    if value
        .get("routing")
        .and_then(|routing| routing.get("rules"))
        .and_then(toml::Value::as_array)
        .is_some_and(|rules| {
            rules
                .iter()
                .any(|rule| rule.get("requires_claude").is_some())
        })
    {
        bail!(
            "routing_requires_claude_removed: replace `requires_claude = true` with `capability = \"native-chat\"` in each [[routing.rules]] row"
        );
    }
    let raw: RawConfig = value.try_into()?;
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
    let config = LaunchConfig {
        recipes,
        adapters,
        dispatch: raw.dispatch,
        routing: raw.routing,
        doctor: raw.doctor,
        policy_hash,
    };
    config.routing.validate(&config.recipes)?;
    Ok(config)
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
            reach.push("Rolf's one-off choice".into());
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
    crate::talk::resolve_request(project, basis)
        .map(|request| request.qualified_basis())
        .map_err(|error| crate::refusal::error(error.to_string()))
}

/// Validate and record Rolf's one-off recipe choice before a lane is created.
/// The quote must occur verbatim in a request attached to the stable task.
pub fn authorize_explicit_recipe(
    ctx: &Ctx,
    project: &Project,
    task_id: &str,
    task_text: &str,
    workflow: &str,
    recipe_id: &str,
    basis: &str,
) -> Result<String> {
    let quote = basis.trim();
    if quote.is_empty() {
        return Err(crate::refusal::error(
            "recipe_basis_missing: --recipe requires --basis with Rolf's exact words",
        ));
    }
    if task_id.is_empty() {
        return Err(crate::refusal::error(
            "recipe_task_missing: --recipe requires --job or a task created with --request",
        ));
    }
    let task = crate::task::load(project, task_id)?;
    let request = task
        .authority
        .iter()
        .filter_map(|authority| crate::talk::resolve_request(project, authority).ok())
        .find(|request| request.text.contains(quote))
        .with_context(
            || "recipe_authority: --basis must quote Rolf's words from a request on this task",
        )?;
    let config = parse_launch_config(&ctx.config_dir)?;
    validate_config(&config, &agent_kinds(ctx.env, ctx.runner)?)?;
    let recipe = config
        .recipes
        .get(recipe_id)
        .with_context(|| format!("routing_recipe_unknown: {recipe_id}"))?;
    if !recipe.enabled {
        bail!("routing_recipe_disabled: {recipe_id}");
    }
    let work = work_contract(task_text, workflow)?;
    if let Some(capability) = &work.capability
        && !recipe.capabilities.contains(capability)
    {
        bail!("routing_capability_missing: recipe `{recipe_id}` does not declare `{capability}`");
    }
    Ok(request.basis())
}

/// Optional task front matter describes the deliverable or a hard runtime
/// requirement, not a model preference. URLs in the body never trigger a rule.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TaskContract {
    product: String,
    capability: Option<String>,
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
    })
}

#[derive(Debug, Default)]
pub struct ResolveInput<'a> {
    pub task: &'a str,
    /// Selects skill text and an ordered routing rule.
    pub workflow: &'a str,
    /// One recipe Rolf named for this lane. Ordinary starts leave this empty.
    pub recipe: Option<&'a str>,
    /// A recipe selected for the project's coordinator. Unlike a lane's
    /// one-off choice, it is retained on the coordinator binding itself.
    pub project_recipe: Option<&'a str>,
    /// Rolf's verbatim words and their task-bound request, validated before
    /// dispatch and persisted on the launch record.
    pub recipe_basis: Option<&'a str>,
    pub recipe_request: Option<&'a str>,
    pub previous: Option<&'a Launch>,
    pub failure: Option<&'a str>,
    /// Evidence omitted by an upstream task builder, already disclosed in the
    /// brief; copied into the launch record and dispatch ledger.
    pub source_truncation: Option<&'a Value>,
}

pub fn resolve_launch(ctx: &Ctx, project: &Project, input: &ResolveInput) -> Result<Launch> {
    let result = resolve(ctx, project, input);
    if let Err(error) = &result {
        ledger(
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
    use crate::contracts::FailureClass;
    let previous = input.previous.context("recovery_previous_missing")?;
    match class {
        FailureClass::Unknown => Err(crate::refusal::error(
            "recovery_unknown: waiting for the coordinator",
        )),
        FailureClass::WorkFailed if previous.routing_rule == "explicit" => {
            let config = parse_launch_config(&ctx.config_dir)?;
            validate_config(&config, &agent_kinds(ctx.env, ctx.runner)?)?;
            let recovery = previous.work_retries.saturating_add(1);
            if recovery > config.routing.retries {
                return Err(crate::refusal::error(format!(
                    "recovery_exhausted: Rolf's one-off recipe allowed {} retries; waiting for the coordinator",
                    config.routing.retries
                )));
            }
            let mut same = previous.clone();
            same.work_retries = recovery;
            ledger(
                project,
                json!({"kind":"recovery", "class":class, "recipe":same.recipe_id,
                    "explicit_retry":recovery, "failure":input.failure,
                    "policy_hash":config.policy_hash}),
            )?;
            Ok(same)
        }
        FailureClass::WorkFailed => resolve_launch(ctx, project, input),
        FailureClass::Provider | FailureClass::LostConnection | FailureClass::ProcessGone => {
            same_recipe_retry(ctx, project, input, class, "recovery", true)
        }
    }
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
    ledger_kind: &str,
    automatic: bool,
) -> Result<Launch> {
    let previous = input.previous.context("recovery_previous_missing")?;
    let config = parse_launch_config(&ctx.config_dir)?;
    validate_config(&config, &agent_kinds(ctx.env, ctx.runner)?)?;
    let work = work_contract(input.task, input.workflow)?;
    let recovery = previous.same_recipe_retries.saturating_add(1);
    let retries = config.routing.retry_limit(&work);
    if automatic && recovery > retries {
        return Err(crate::refusal::error(format!(
            "recovery_exhausted: {} allowed {retries} same-recipe retries; waiting for the coordinator",
            class.plain()
        )));
    }
    let mut same = previous.clone();
    same.same_recipe_retries = recovery;
    ledger(
        project,
        json!({"kind":ledger_kind, "class":class, "recipe":same.recipe_id,
            "same_recipe_retry":recovery, "failure":input.failure,
            "policy_hash":config.policy_hash}),
    )?;
    Ok(same)
}

fn resolve(ctx: &Ctx, project: &Project, input: &ResolveInput) -> Result<Launch> {
    if input.task.trim().is_empty() {
        bail!("dispatch_brief_missing: supply the full task file");
    }
    let config = parse_launch_config(&ctx.config_dir)?;
    validate_config(&config, &agent_kinds(ctx.env, ctx.runner)?)?;
    let work = work_contract(input.task, input.workflow)?;
    let hash = crate::thread::sha256_hex(input.task.as_bytes());
    let recovery = input
        .previous
        .map_or(0, |previous| previous.work_retries.saturating_add(1));
    if input.previous.is_some()
        && input
            .failure
            .is_none_or(|failure| failure.trim().is_empty())
    {
        bail!("recovery_failure_missing");
    }
    let selected = match (input.project_recipe, input.recipe) {
        (Some(recipe), None) => {
            if input.previous.is_some() {
                bail!(
                    "recipe_override_recovery: a project recipe is chosen only when its coordinator starts"
                );
            }
            crate::routing::Selection {
                recipe: recipe.to_string(),
                rule: "project".into(),
                pinned: true,
            }
        }
        (None, Some(recipe)) => {
            if input.previous.is_some() {
                bail!(
                    "recipe_override_recovery: a one-off recipe is chosen only when the lane starts"
                );
            }
            if input
                .recipe_basis
                .is_none_or(|basis| basis.trim().is_empty())
                || input
                    .recipe_request
                    .is_none_or(|request| !request.starts_with("request:"))
            {
                bail!(
                    "recipe_authority_missing: a one-off recipe needs Rolf's quoted words and task request"
                );
            }
            crate::routing::Selection {
                recipe: recipe.to_string(),
                rule: "explicit".into(),
                pinned: true,
            }
        }
        (None, None) => {
            let selected = config.routing.select(&hash, &work, recovery)?;
            if let Some(previous) = input.previous {
                crate::routing::Selection {
                    recipe: previous.recipe_id.clone(),
                    rule: previous.routing_rule.clone(),
                    pinned: selected.pinned,
                }
            } else {
                selected
            }
        }
        (Some(_), Some(_)) => bail!("recipe_choice_ambiguous"),
    };
    let recipe = config
        .recipes
        .get(&selected.recipe)
        .context("routing_recipe_unknown")?;
    if let Some(capability) = &work.capability
        && !recipe.capabilities.contains(capability)
    {
        bail!(
            "routing_capability_missing: recipe `{}` does not declare `{capability}`",
            selected.recipe
        );
    }
    if !recipe.enabled {
        bail!("routing_recipe_disabled: {}", selected.recipe);
    }
    if parse_launch_config(&ctx.config_dir)?.policy_hash != config.policy_hash {
        bail!("dispatch_policy_changed: config changed during selection; dispatch again");
    }
    ledger(
        project,
        json!({"kind": if recovery > 0 { "recovery" } else { "pick" },
        "brief_hash": hash, "recipe": selected.recipe, "rule": selected.rule,
        "workflow": input.workflow, "basis": input.recipe_basis,
        "request": input.recipe_request,
        "previous": input.previous.map(|previous| json!({"recipe":previous.recipe_id,"attempt":previous.attempt})),
        "failure": input.failure, "recovery": recovery,
        "source_truncation": input.source_truncation,
        "policy_hash": config.policy_hash}),
    )?;
    Ok(Launch {
        kind: recipe.kind.clone(),
        args: crate::adapters::launch_args(
            config
                .adapters
                .get(&recipe.kind)
                .context("adapter_unknown")?,
            recipe,
        ),
        env: recipe.env.clone(),
        ready_timeout_ms: if recipe.ready_timeout_ms == 0 {
            config.adapters[&recipe.kind].ready_timeout_ms
        } else {
            recipe.ready_timeout_ms
        },
        policy_hash: config.policy_hash,
        attempt: 1,
        recipe_id: selected.recipe,
        work_retries: recovery,
        routing_rule: selected.rule,
        recipe_basis: input.recipe_basis.unwrap_or_default().to_string(),
        recipe_request: input.recipe_request.unwrap_or_default().to_string(),
        reason: if selected.pinned {
            pinned_reason(input.workflow, &recipe.plain)
        } else {
            usual_reason(input.workflow, &recipe.plain)
        },
        compact_reason: compact_reason(input.workflow, &recipe.plain),
        source_truncation: input.source_truncation.cloned(),
        machine: config.dispatch.machine,
        ..Launch::default()
    })
}

/// Append-only dispatch ledger. No network call is made while holding its lock.
pub fn ledger(project: &Project, mut row: Value) -> Result<()> {
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
pub fn doctor_rows(ctx: &Ctx) -> Result<Vec<DoctorRow>> {
    let config = parse_launch_config(&ctx.config_dir)?;
    let valid = validate_recipe_reachability(&config)
        .and_then(|_| validate_config(&config, &agent_kinds(ctx.env, ctx.runner)?));
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

/// `"<job> runs on <plain>"`, at most 80 characters: the ticker's `ade_last`
/// token.
pub fn compact_reason(role: &str, plain: &str) -> String {
    let sentence = format!("{} runs on {}", job_noun(role), plain.trim());
    if sentence.len() <= COMPACT_LIMIT {
        return sentence;
    }
    let mut end = COMPACT_LIMIT;
    while end > 0 && !sentence.is_char_boundary(end) {
        end -= 1;
    }
    match sentence[..end].rfind(char::is_whitespace) {
        Some(pos) if pos > 0 => sentence[..pos].trim_end().to_string(),
        _ => sentence[..end].trim_end().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_names_every_recipe_and_each_exact_path() {
        let ordinary = Recipe {
            kind: "pi".into(),
            plain: "the quick helper".into(),
            capabilities: vec!["pictures".into()],
            ..Recipe::default()
        };
        let disabled = Recipe {
            enabled: false,
            plain: "the sleeping helper".into(),
            ..ordinary.clone()
        };
        let config = LaunchConfig {
            recipes: BTreeMap::from([("ordinary".into(), ordinary), ("sleeping".into(), disabled)]),
            adapters: BTreeMap::new(),
            dispatch: DispatchConfig::default(),
            routing: crate::routing::Routing {
                default: "ordinary".into(),
                rules: vec![crate::routing::Rule {
                    product: Some("web-research".into()),
                    capability: Some("pictures".into()),
                    recipe: "ordinary".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            doctor: DoctorConfig::default(),
            policy_hash: String::new(),
        };
        validate_recipe_reachability(&config).unwrap();
        let lines = context_recipe_lines(&config);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("capabilities=pictures"), "{:?}", lines);
        assert!(lines[0].contains("reach: default"), "{:?}", lines);
        assert!(
            lines[0].contains(
                "brief front matter: product = \"web-research\", capability = \"pictures\""
            ),
            "{:?}",
            lines
        );
        assert!(lines[0].contains("Rolf's one-off choice"), "{:?}", lines);
        assert!(!lines.iter().any(|line| line.contains("thread start")));
        assert!(lines[1].contains("[disabled]"), "{:?}", lines);
        assert!(!lines[1].contains("reach:"), "{:?}", lines);
    }

    #[test]
    fn doctor_rejects_an_enabled_recipe_with_neither_route_nor_command() {
        let config = LaunchConfig {
            recipes: BTreeMap::from([(
                "orphan".into(),
                Recipe {
                    kind: "undeclared".into(),
                    enabled: true,
                    ..Recipe::default()
                },
            )]),
            adapters: BTreeMap::new(),
            dispatch: DispatchConfig::default(),
            routing: crate::routing::Routing::default(),
            doctor: DoctorConfig::default(),
            policy_hash: String::new(),
        };
        let error = validate_recipe_reachability(&config)
            .unwrap_err()
            .to_string();
        assert!(error.contains("recipe_unreachable"), "{error}");
    }

    #[test]
    fn removed_claude_matcher_names_the_exact_config_replacement() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"[routing]
default = "claude_fable_xhigh"
[[routing.rules]]
requires_claude = true
recipe = "claude_fable_xhigh"
"#,
        )
        .unwrap();
        let error = parse_launch_config(dir.path()).unwrap_err().to_string();
        assert!(
            error.contains("replace `requires_claude = true` with `capability = \"native-chat\"`"),
            "{error}"
        );
    }
}
