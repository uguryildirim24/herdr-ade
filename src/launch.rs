//! Dispatch resolves the full work brief, never a coordinator-selected model.
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
pub const MAX_ESCALATIONS: u32 = 3;
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

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchConfig {
    pub recipes: BTreeMap<String, Recipe>,
    pub dispatch: DispatchConfig,
    pub policy_hash: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    recipes: BTreeMap<String, Recipe>,
    dispatch: DispatchConfig,
}

pub fn parse_launch_config(config_dir: &Path) -> Result<LaunchConfig> {
    let file = config_dir.join("config.toml");
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", file.display())),
    };
    let value: toml::Value = toml::from_str(&text).context("config.toml does not parse")?;
    if value.get("roles").is_some() {
        bail!(
            "roles_removed: remove [roles] and every [roles.*] table from config.toml; keep model rows in [recipes.*]"
        );
    }
    let raw: RawConfig = value.try_into()?;
    let mut recipes = builtin_recipes();
    // A configured row is a complete recipe, not an old inline role or a partial patch.
    recipes.extend(raw.recipes);
    let policy_hash =
        crate::thread::sha256_hex(serde_json::to_vec(&(&recipes, &raw.dispatch))?.as_slice());
    Ok(LaunchConfig {
        recipes,
        dispatch: raw.dispatch,
        policy_hash,
    })
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
            args.extend(["--effort".into(), "xhigh".into()]);
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
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WorkContract {
    product: String,
    requires_claude: bool,
}

pub fn work_contract(task: &str, workflow: &str) -> Result<Value> {
    let contract = if workflow == "coordinator" {
        WorkContract::default()
    } else if let Some(rest) = task.strip_prefix("+++\n") {
        let (front, _) = rest
            .split_once("\n+++\n")
            .context("task_contract: unclosed front matter")?;
        toml::from_str::<WorkContract>(front).context("task_contract: describe product and requires_claude; model/recipe/role overrides are forbidden")?
    } else {
        WorkContract::default()
    };
    if !matches!(
        contract.product.as_str(),
        "" | "code" | "web-research" | "spec"
    ) {
        bail!("task_contract: product must be code, web-research or spec");
    }
    let mut value = serde_json::to_value(contract)?;
    value["workflow"] = json!(workflow);
    Ok(value)
}

#[derive(Debug, Default)]
pub struct ResolveInput<'a> {
    pub task: &'a str,
    pub state: Value,
    /// Internal workflow label only: determines skill text, never a model role.
    pub workflow: &'a str,
    pub previous: Option<&'a Launch>,
    pub failure: Option<&'a str>,
}

pub fn resolve_launch(ctx: &Ctx, project: &Project, input: &ResolveInput) -> Result<Launch> {
    let result = resolve(ctx, project, input);
    if let Err(error) = &result {
        ledger(
            project,
            json!({"kind":"dispatch-refused", "brief_hash":crate::thread::sha256_hex(input.task.as_bytes()),
            "failure":input.failure, "error":format!("{error:#}")}),
        )?;
    }
    result
}

fn resolve(ctx: &Ctx, project: &Project, input: &ResolveInput) -> Result<Launch> {
    if input.task.trim().is_empty() {
        bail!("dispatch_brief_missing: supply the full task file");
    }
    let config = parse_launch_config(&ctx.config_dir)?;
    validate_config(&config, &agent_kinds(ctx.env, ctx.runner)?)?;
    let policy_path = ctx.config_dir.join("routing.json");
    let policy_bytes =
        std::fs::read(&policy_path).context("routing_policy_missing: install routing.json")?;
    let policy = crate::routing::Policy::parse(&policy_bytes)?;
    policy.validate_recipes(&config.recipes)?;
    let work = work_contract(input.task, input.workflow)?;
    let hash = crate::thread::sha256_hex(input.task.as_bytes());
    let excluded = policy.exclusion(input.task, &work);
    let escalations = input
        .previous
        .map_or(0, |p| p.escalations.saturating_add(1));
    if escalations > MAX_ESCALATIONS {
        bail!("escalation_bound: at most {MAX_ESCALATIONS} model changes per lane");
    }
    if input.previous.is_some() && input.failure.is_none_or(|f| f.trim().is_empty()) {
        bail!("escalation_failure_missing");
    }
    // Fixed exclusions are never sent to Jev, including after failure.
    if excluded.is_some() && input.previous.is_some() {
        bail!(
            "escalation_excluded: fixed research, Claude, spec or human-pinned work cannot change models"
        );
    }
    if input.previous.is_some_and(|p| p.strength == 0) {
        bail!("escalation_tier_missing: the earlier launch has no recorded capability tier");
    }
    // A policy edit cannot turn the same recipe into its own escalation.
    let previous_tier = input.previous.map(|p| {
        p.strength
            .max(policy.models.get(&p.recipe_id).map_or(0, |m| m.tier))
    });
    let (id, assessment, decision, rule) = if let Some(id) = excluded {
        (
            id.to_string(),
            None,
            None,
            if policy.pins.contains_key(&hash) {
                "human-pin"
            } else {
                "exclusion"
            },
        )
    } else {
        if previous_tier.is_some_and(|tier| {
            !policy
                .routes
                .iter()
                .any(|r| policy.models[&r.recipe].tier > tier)
        }) {
            bail!("escalation_exhausted: no stronger model remains");
        }
        let state = crate::routing::scrub(
            json!({"brief": input.task, "repository": input.state,
            "failure": input.failure}),
            &config.recipes,
        );
        let assessment = crate::jev::call(ctx, &policy.request(state), &policy.questions)?;
        let decision = policy.select(&assessment, previous_tier)?;
        (
            decision.recipe.clone(),
            Some(assessment),
            Some(decision),
            "jev-scores",
        )
    };
    let recipe = config.recipes.get(&id).context("recipe_unknown")?;
    if !recipe.enabled {
        bail!("recipe_disabled: {id}");
    }
    if parse_launch_config(&ctx.config_dir)?.policy_hash != config.policy_hash
        || std::fs::read(&policy_path)? != policy_bytes
    {
        bail!("dispatch_policy_changed: config changed during selection; dispatch again");
    }
    ledger(
        project,
        json!({"kind": if escalations > 0 { "escalation" } else { "pick" },
        "brief_hash": hash, "recipe": id, "rule": rule, "assessment": assessment, "decision": decision,
        "low_confidence": decision.as_ref().is_some_and(|d| d.confidence < policy.confidence_floor),
        "previous": input.previous.map(|p| json!({"recipe":p.recipe_id,"tier":p.strength,"attempt":p.attempt})),
        "failure": input.failure, "escalations": escalations,
        "policy_hash": config.policy_hash, "routing_hash": crate::thread::sha256_hex(&policy_bytes)}),
    )?;
    Ok(Launch {
        kind: recipe.kind.clone(),
        args: recipe.args.clone(),
        env: recipe.env.clone(),
        ready_timeout_ms: recipe.ready_timeout_ms,
        policy_hash: config.policy_hash,
        attempt: 1,
        strength: policy.models.get(&id).map_or(0, |m| m.tier),
        recipe_id: id,
        escalations,
        reason: if rule == "human-pin" {
            pinned_reason(input.workflow, &recipe.plain)
        } else {
            usual_reason(input.workflow, &recipe.plain)
        },
        compact_reason: compact_reason(input.workflow, &recipe.plain),
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

/// Only measured repository facts, never a summary invented from a title.
pub fn repository_state(ctx: &Ctx, repo: Option<&str>, base: Option<&str>) -> Result<Value> {
    let Some(repo) = repo else {
        return Ok(json!({"repository": null}));
    };
    let run = |args: &[&str]| -> Result<String> {
        let out = ctx.runner.run(
            &Cmd::new("git", HELP_TIMEOUT)
                .arg("-C")
                .arg(repo)
                .args(args.iter().copied()),
        )?;
        if !out.success() {
            bail!(
                "dispatch_repository: git probe failed: {}",
                out.error_text()
            );
        }
        Ok(out.stdout)
    };
    Ok(
        json!({"path": repo, "base": base, "head": run(&["rev-parse", "HEAD"])?,
        "status": run(&["status", "--short"])?, "files": run(&["ls-files"])?,
        "recent_changes": run(&["log", "-5", "--oneline", "--stat"])?}),
    )
}

/// Dialogue workflow labels are not model roles. Selection happens only once
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
    let valid = validate_config(&config, &agent_kinds(ctx.env, ctx.runner)?).and_then(|()| {
        crate::routing::Policy::read(&ctx.config_dir.join("routing.json"))?
            .validate_recipes(&config.recipes)
    });
    Ok(vec![
        DoctorRow {
            ok: Some(valid.is_ok()),
            label: "recipes".into(),
            detail: valid.err().map(|e| format!("{e:#}")).unwrap_or_else(|| {
                format!(
                    "{} model recipes; Jev chooses from the full brief",
                    config.recipes.len()
                )
            }),
        },
        DoctorRow {
            ok: Some(
                ctx.env
                    .var("TYPESAFE_API_KEY")
                    .is_some_and(|k| !k.trim().is_empty()),
            ),
            label: "Jev".into(),
            detail: "TYPESAFE_API_KEY must be set in the dispatch process environment".into(),
        },
    ])
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
