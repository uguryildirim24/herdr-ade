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
use crate::plain::{self, Glossary};
use crate::project::{self, Project};
use crate::runner::{Cmd, Runner};

pub const HELP_TIMEOUT: Duration = Duration::from_secs(10);
pub const COMPACT_LIMIT: usize = 80;
pub const TEMPLATE_PINNED: &str = "You chose {plain} for {job}.";
pub const TEMPLATE_USUAL: &str = "{job} runs on {plain}, chosen for this work.";
pub const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const AGY_EFFORTS: [&str; 3] = ["low", "medium", "high"];

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

fn config_text(config_dir: &Path) -> Result<String> {
    let file = config_dir.join("config.toml");
    match std::fs::read_to_string(&file) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", file.display())),
    }
}

fn validate_doctor_config(config: &DoctorConfig) -> Result<()> {
    if !config.min_free_disk_gb.is_finite() || config.min_free_disk_gb < 0.0 {
        bail!("doctor_min_free_disk_invalid: [doctor].min_free_disk_gb must be zero or greater");
    }
    Ok(())
}

pub fn doctor_config(config_dir: &Path) -> Result<DoctorConfig> {
    let raw: DoctorOnlyConfig =
        toml::from_str(&config_text(config_dir)?).context("config.toml does not parse")?;
    validate_doctor_config(&raw.doctor)?;
    Ok(raw.doctor)
}

pub fn parse_launch_config(config_dir: &Path) -> Result<LaunchConfig> {
    let value: toml::Value =
        toml::from_str(&config_text(config_dir)?).context("config.toml does not parse")?;
    if value.get("roles").is_some() {
        bail!(
            "roles_removed: remove [roles] and every [roles.*] table from config.toml; keep model rows in [recipes.*]"
        );
    }
    let raw: RawConfig = value.try_into()?;
    validate_doctor_config(&raw.doctor)?;
    let mut recipes = builtin_recipes();
    // A configured row is a complete recipe, not an old inline role or a partial patch.
    recipes.extend(raw.recipes);
    let policy_hash = crate::thread::sha256_hex(
        serde_json::to_vec(&(&recipes, &raw.dispatch, &raw.routing))?.as_slice(),
    );
    let config = LaunchConfig {
        recipes,
        dispatch: raw.dispatch,
        routing: raw.routing,
        doctor: raw.doctor,
        policy_hash,
    };
    config.routing.validate(&config.recipes)?;
    Ok(config)
}

fn builtin_recipes() -> BTreeMap<String, Recipe> {
    let mut recipes = BTreeMap::new();
    for (id, model, plain) in [
        (
            "agy_gemini_flash",
            "gemini-3.8-flash-high",
            "the web research helper",
        ),
        (
            "claude_fable_xhigh",
            "claude-fable-5-1",
            "the planning helper",
        ),
        (
            "claude_coordinator_opus",
            "claude-opus-5",
            "the planning helper",
        ),
    ] {
        let kind = if id.starts_with("agy") {
            "agy"
        } else {
            "claude"
        };
        let mut args = vec![
            "--model".into(),
            model.into(),
            "--dangerously-skip-permissions".into(),
        ];
        if kind == "claude" {
            let effort = if id == "claude_coordinator_opus" {
                "high"
            } else {
                "xhigh"
            };
            args.extend(["--effort".into(), effort.into()]);
        }
        recipes.insert(
            id.into(),
            Recipe {
                kind: kind.into(),
                provider: kind.into(),
                args,
                plain: plain.into(),
                ..Recipe::default()
            },
        );
    }
    for row in crate::pi::recipes::pi_recipes() {
        recipes.insert(
            row.id.into(),
            Recipe {
                kind: row.kind.into(),
                provider: row.provider.into(),
                args: row.args,
                env: row.env,
                ready_timeout_ms: row.ready_timeout_ms,
                enabled: row.enabled,
                plain: row.plain.into(),
            },
        );
    }
    recipes
}

pub fn validate_config(config: &LaunchConfig, kinds: &BTreeSet<String>) -> Result<()> {
    for (id, recipe) in &config.recipes {
        if !kinds.contains(recipe.kind.trim()) {
            bail!("recipe_kind_unknown: {id}: {}", recipe.kind);
        }
        validate_flags(id, recipe)?;
        if recipe.kind == "pi" {
            crate::pi::launch::validate_args(&recipe.args)?;
            crate::pi::launch::validate_provider_column(&recipe.provider, &recipe.args)?;
        }
        check_plain(&format!("recipe {id}"), &recipe.plain)?;
    }
    Ok(())
}

/// Optional task front matter describes the deliverable or a hard runtime
/// requirement, not a model preference. URLs in the body never trigger a rule.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TaskContract {
    product: String,
    requires_claude: bool,
}

pub fn work_contract(task: &str, workflow: &str) -> Result<crate::routing::WorkContract> {
    let contract = if workflow == "coordinator" {
        TaskContract::default()
    } else if let Some(rest) = task.strip_prefix("+++\n") {
        let (front, _) = rest
            .split_once("\n+++\n")
            .context("task_contract: unclosed front matter")?;
        toml::from_str::<TaskContract>(front).context("task_contract: describe product and requires_claude; model/recipe/role overrides are forbidden")?
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
        requires_claude: contract.requires_claude,
    })
}

#[derive(Debug, Default)]
pub struct ResolveInput<'a> {
    pub task: &'a str,
    /// Selects skill text and an ordered routing rule.
    pub workflow: &'a str,
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
/// retries; a gone process restarts; only failed work may select a fallback.
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
        FailureClass::WorkFailed => resolve_launch(ctx, project, input),
        FailureClass::Provider | FailureClass::LostConnection | FailureClass::ProcessGone => {
            let config = parse_launch_config(&ctx.config_dir)?;
            validate_config(&config, &agent_kinds(ctx.env, ctx.runner)?)?;
            let work = work_contract(input.task, input.workflow)?;
            let recovery = previous.same_recipe_retries.saturating_add(1);
            let retries = config.routing.retry_limit(&work);
            if recovery > retries {
                return Err(crate::refusal::error(format!(
                    "recovery_exhausted: {} allowed {retries} same-recipe retries; waiting for the coordinator",
                    class.plain()
                )));
            }
            let mut same = previous.clone();
            same.same_recipe_retries = recovery;
            ledger(
                project,
                json!({"kind":"recovery", "class":class, "recipe":same.recipe_id,
                    "same_recipe_retry":recovery, "failure":input.failure,
                    "policy_hash":config.policy_hash}),
            )?;
            Ok(same)
        }
    }
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
        .map_or(0, |previous| previous.escalations.saturating_add(1));
    if input.previous.is_some()
        && input
            .failure
            .is_none_or(|failure| failure.trim().is_empty())
    {
        bail!("recovery_failure_missing");
    }
    let selected = config.routing.select(&hash, &work, recovery)?;
    let recipe = config
        .recipes
        .get(&selected.recipe)
        .context("routing_recipe_unknown")?;
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
        "workflow": input.workflow,
        "previous": input.previous.map(|previous| json!({"recipe":previous.recipe_id,"attempt":previous.attempt})),
        "failure": input.failure, "recovery": recovery,
        "source_truncation": input.source_truncation,
        "policy_hash": config.policy_hash}),
    )?;
    Ok(Launch {
        kind: recipe.kind.clone(),
        args: recipe.args.clone(),
        env: recipe.env.clone(),
        ready_timeout_ms: recipe.ready_timeout_ms,
        policy_hash: config.policy_hash,
        attempt: 1,
        recipe_id: selected.recipe,
        escalations: recovery,
        routing_rule: selected.rule,
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

/// Dialogue workflow labels may match routing rules. Selection happens only once
/// each complete side's brief exists; no model pin is printed by this command.
pub struct DialoguePair;
impl crate::dialogue::PairFilter for DialoguePair {
    fn check(&self, drafter: &str, critic: &str) -> std::result::Result<(), String> {
        crate::dialogue::same_role(drafter, critic)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorRow {
    pub ok: Option<bool>,
    pub label: String,
    pub detail: String,
}
pub fn doctor_rows(ctx: &Ctx) -> Result<Vec<DoctorRow>> {
    let config = parse_launch_config(&ctx.config_dir)?;
    let valid = validate_config(&config, &agent_kinds(ctx.env, ctx.runner)?);
    Ok(vec![DoctorRow {
        ok: Some(valid.is_ok()),
        label: "recipes and routing".into(),
        detail: valid
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_else(|| {
                format!(
                    "{} recipes; editable routing table is valid",
                    config.recipes.len()
                )
            }),
    }])
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

fn validate_flags(id: &str, recipe: &Recipe) -> Result<()> {
    let args: Vec<&str> = recipe.args.iter().map(String::as_str).collect();
    let has = |flag: &str| args.contains(&flag);
    match recipe.kind.as_str() {
        "claude" => {
            check_effort(id, &args, &CLAUDE_EFFORTS)?;
            if !has("--dangerously-skip-permissions") {
                bail!("recipe_permission_missing: `{id}` has no permission flag");
            }
        }
        "agy" => {
            check_effort(id, &args, &AGY_EFFORTS)?;
            if !has("--dangerously-skip-permissions") {
                bail!("recipe_permission_missing: `{id}` has no permission flag");
            }
        }
        "cursor" if !has("--force") => {
            bail!("recipe_permission_missing: `{id}` has no permission flag");
        }
        _ => {}
    }
    Ok(())
}

/// The check only verifies the effort is one of the values the CLI knows; the
/// recipe table no longer caps Opus or Fable.
fn check_effort(id: &str, args: &[&str], known: &[&str]) -> Result<()> {
    if let Some(effort) = effort_value(args)
        && !known.contains(&effort.as_str())
    {
        bail!("recipe_effort_unknown: `{id}` names effort {effort:?}");
    }
    Ok(())
}

fn check_plain(context: &str, sentence: &str) -> Result<()> {
    let result = plain::check(sentence, &Glossary::default());
    if !result.passed() {
        let fixes: Vec<String> = result
            .violations
            .iter()
            .map(|violation| violation.fix.clone())
            .collect();
        bail!(
            "recipe_reason_not_plain: {context} fails the plain check (`{sentence}`): {}",
            fixes.join("; ")
        );
    }
    Ok(())
}

fn effort_value(args: &[&str]) -> Option<String> {
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--effort" {
            return args.get(index + 1).map(|value| (*value).to_string());
        }
        index += 1;
    }
    None
}

/// The job noun of each role.
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
