//! Launch-time resolution: validate the recipe table and return the `Launch`
//! object the thread record stores. A lane's model is the role's `default`, or
//! a row the coordinator pins by hand; there is no picker.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::{Launch, Recipe};
use crate::paths::{Ctx, Env};
use crate::plain::{self, Glossary};
use crate::runner::{Cmd, Runner};

/// `herdr agent start --help` is a table check, not a launch.
pub const HELP_TIMEOUT: Duration = Duration::from_secs(10);

/// Roles that are never launched from the roles table.
pub const NEVER_RESOLVED: [&str; 2] = ["pro", "coordinator"];

/// The lane picker was removed. A config that still carries one of these keys
/// fails with `picker_removed` naming the key.
pub const REMOVED_ROLE_KEYS: [&str; 5] = [
    "resolver",
    "jev_model",
    "jev_timeout_ms",
    "jev_daily_cap",
    "floor",
];
pub const REMOVED_ROLE_FIELDS: [&str; 2] = ["gates", "cost"];
pub const REMOVED_RECIPE_FIELDS: [&str; 1] = ["cost"];

/// Reason templates, fixed in the binary and checked at load.
pub const TEMPLATE_PINNED: &str = "You chose {plain} for {job}.";
/// The default row's sentence, with no picker in the picture.
pub const TEMPLATE_USUAL: &str = "{job} runs on {plain}, the usual choice.";
/// The compact `ade_last` form.
pub const COMPACT_LIMIT: usize = 80;

/// Effort values the Claude CLI accepts.
pub const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Effort values the agy CLI accepts (`agy --help`).
pub const AGY_EFFORTS: [&str; 3] = ["low", "medium", "high"];

/// One role's recipe lists: the `default` it launches with, the `allowed` rows
/// a `--recipe` pin may name, and the `escalate` rows reserved for a stalled
/// lane when Rolf asks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleConfig {
    pub default: String,
    pub allowed: Vec<String>,
    pub escalate: Vec<String>,
    /// The role's machine choice (SPEC-remote D2, §4.1). Empty means the
    /// recipe's own row, then the project default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub machine: String,
}

/// The safety file's view of `~/.config/herdr-ade/config.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchConfig {
    pub recipes: BTreeMap<String, Recipe>,
    pub roles: BTreeMap<String, RoleConfig>,
    /// SHA-256 over the recipes and roles.
    pub policy_hash: String,
    /// Inline roles written without `plain`: they get the shipped phrase and
    /// `doctor` warns, never a refusal.
    pub inline_without_plain: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    roles: BTreeMap<String, toml::Value>,
    #[serde(default)]
    recipes: BTreeMap<String, Recipe>,
}

/// A `[roles.<name>]` table: either an inline row or a recipe reference.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRole {
    kind: Option<String>,
    args: Option<Vec<String>>,
    env: Option<Vec<String>>,
    ready_timeout_ms: Option<u64>,
    provider: Option<String>,
    enabled: Option<bool>,
    plain: Option<String>,
    default: Option<String>,
    allowed: Option<Vec<String>>,
    escalate: Option<Vec<String>>,
    machine: Option<String>,
}

/// The ready-made rows the plugin ships: research on agy and the Fable
/// planner. A config row may not reuse one.
fn builtin_recipes() -> Vec<(&'static str, Recipe)> {
    vec![
        (
            "agy_gemini_flash",
            Recipe {
                kind: "agy".into(),
                provider: "agy".into(),
                args: vec![
                    "--model".into(),
                    "gemini-3.8-flash-high".into(),
                    "--dangerously-skip-permissions".into(),
                ],
                ready_timeout_ms: 60_000,
                enabled: true,
                plain: "the web research helper".into(),
                ..Recipe::default()
            },
        ),
        (
            "claude_fable_xhigh",
            Recipe {
                kind: "claude".into(),
                provider: "claude".into(),
                args: vec![
                    "--model".into(),
                    "claude-fable-5-1".into(),
                    "--effort".into(),
                    "xhigh".into(),
                    "--dangerously-skip-permissions".into(),
                ],
                ready_timeout_ms: 90_000,
                enabled: true,
                plain: "the planning helper".into(),
                ..Recipe::default()
            },
        ),
    ]
}

/// The ready-made roles the plugin ships. Config rows override them.
fn builtin_roles() -> BTreeMap<String, RoleConfig> {
    let mut roles = BTreeMap::new();
    roles.insert(
        "research".to_string(),
        RoleConfig {
            default: "agy_gemini_flash".into(),
            allowed: vec!["agy_gemini_flash".into()],
            escalate: Vec::new(),
            machine: String::new(),
        },
    );
    roles.insert(
        "planner".to_string(),
        RoleConfig {
            default: "claude_fable_xhigh".into(),
            allowed: vec!["claude_fable_xhigh".into()],
            escalate: Vec::new(),
            machine: String::new(),
        },
    );
    roles
}

/// Parse the safety file's roles and recipes. An absent file is the shipped
/// default.
pub fn parse_launch_config(config_dir: &Path) -> Result<LaunchConfig> {
    let file = config_dir.join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let raw: RawConfig = if text.trim().is_empty() {
        RawConfig::default()
    } else {
        let value: toml::Value =
            toml::from_str(&text).with_context(|| format!("{} does not parse", file.display()))?;
        reject_removed_keys(&value)?;
        toml::from_str(&text).with_context(|| format!("{} does not parse", file.display()))?
    };

    let builtins = builtin_recipes();
    let mut recipes: BTreeMap<String, Recipe> = builtins
        .iter()
        .map(|(id, recipe)| ((*id).to_string(), recipe.clone()))
        .collect();
    for (id, recipe) in raw.recipes {
        if builtins.iter().any(|(builtin, _)| *builtin == id) {
            bail!("recipe_builtin: `{id}` is a ready-made row; use another id");
        }
        recipes.insert(id, recipe);
    }

    let mut roles = builtin_roles();
    let mut inline_without_plain = Vec::new();
    for (name, value) in raw.roles {
        let raw_role: RawRole = value
            .try_into()
            .with_context(|| format!("[roles.{name}] does not parse"))?;
        if raw_role.kind.is_some()
            && raw_role
                .plain
                .as_deref()
                .is_none_or(|p| p.trim().is_empty())
            && !NEVER_RESOLVED.contains(&name.as_str())
        {
            inline_without_plain.push(name.clone());
        }
        roles.insert(name.clone(), parse_role(&name, raw_role, &mut recipes)?);
    }
    builtin_pi_recipes(&mut recipes, &roles)?;

    let mut config = finish(recipes, roles);
    config.inline_without_plain = inline_without_plain;
    Ok(config)
}

/// Refuse a config that still carries a removed picker key.
fn reject_removed_keys(root: &toml::Value) -> Result<()> {
    let Some(root) = root.as_table() else {
        return Ok(());
    };
    if let Some(roles) = root.get("roles").and_then(toml::Value::as_table) {
        for key in REMOVED_ROLE_KEYS {
            if roles.contains_key(key) {
                bail!("picker_removed: [roles] {key} is gone; the lane picker was removed");
            }
        }
        for (name, value) in roles {
            let Some(role) = value.as_table() else {
                continue;
            };
            for field in REMOVED_ROLE_FIELDS {
                if role.contains_key(field) {
                    bail!(
                        "picker_removed: [roles.{name}] {field} is gone; the lane picker was removed"
                    );
                }
            }
        }
    }
    if let Some(recipes) = root.get("recipes").and_then(toml::Value::as_table) {
        for (id, value) in recipes {
            let Some(recipe) = value.as_table() else {
                continue;
            };
            for field in REMOVED_RECIPE_FIELDS {
                if recipe.contains_key(field) {
                    bail!(
                        "picker_removed: [recipes.{id}] {field} is gone; the lane picker was removed"
                    );
                }
            }
        }
    }
    Ok(())
}

/// The ready-made `kind = "pi"` rows join the table under their own ids; a
/// config row may not reuse one, and a row that is not start-time allowed
/// stays out of an `allowed` list.
fn builtin_pi_recipes(
    recipes: &mut BTreeMap<String, Recipe>,
    roles: &BTreeMap<String, RoleConfig>,
) -> Result<()> {
    for row in crate::pi::roles::pi_recipes() {
        if recipes.contains_key(row.id) {
            bail!(
                "recipe_builtin: `{}` is a ready-made pi row; use another id",
                row.id
            );
        }
        if !row.start_time_allowed
            && let Some(name) = roles
                .iter()
                .find(|(_, role)| role.allowed.iter().any(|a| a == row.id))
                .map(|(name, _)| name)
        {
            bail!(
                "recipe_not_start_time: `{}` may be escalated to, not in [roles.{name}].allowed",
                row.id
            );
        }
        recipes.insert(
            row.id.to_string(),
            Recipe {
                kind: row.kind.to_string(),
                args: row.args.clone(),
                env: row.env.clone(),
                ready_timeout_ms: row.ready_timeout_ms,
                provider: row.provider.to_string(),
                enabled: row.enabled,
                plain: row.plain.to_string(),
                machine: String::new(),
            },
        );
    }
    Ok(())
}

fn finish(recipes: BTreeMap<String, Recipe>, roles: BTreeMap<String, RoleConfig>) -> LaunchConfig {
    let policy_hash = policy_hash(&recipes, &roles);
    LaunchConfig {
        recipes,
        roles,
        policy_hash,
        inline_without_plain: Vec::new(),
    }
}

fn parse_role(
    name: &str,
    raw: RawRole,
    recipes: &mut BTreeMap<String, Recipe>,
) -> Result<RoleConfig> {
    let inline = raw.kind.is_some();
    let reference = raw.default.is_some() || raw.allowed.is_some() || raw.escalate.is_some();
    if inline && reference {
        bail!("role_form_mixed: [roles.{name}] has an inline row and a recipe reference");
    }
    if inline {
        let kind = raw.kind.clone().unwrap_or_default();
        if kind.trim().is_empty() {
            bail!("role_kind_missing: [roles.{name}] has an empty kind");
        }
        let id = format!("{name}_inline");
        let recipe = Recipe {
            provider: raw.provider.clone().unwrap_or_else(|| kind.clone()),
            kind,
            args: raw.args.clone().unwrap_or_default(),
            env: raw.env.clone().unwrap_or_default(),
            ready_timeout_ms: raw
                .ready_timeout_ms
                .unwrap_or_else(|| Recipe::default().ready_timeout_ms),
            enabled: raw.enabled.unwrap_or(true),
            plain: raw
                .plain
                .clone()
                .filter(|plain| !plain.trim().is_empty())
                .unwrap_or_else(|| "the usual helper".to_string()),
            machine: raw.machine.clone().unwrap_or_default(),
        };
        recipes.insert(id.clone(), recipe);
        return Ok(RoleConfig {
            default: id.clone(),
            allowed: vec![id],
            escalate: Vec::new(),
            machine: raw.machine.clone().unwrap_or_default(),
        });
    }
    let default = raw.default.clone().unwrap_or_default();
    if default.trim().is_empty() {
        bail!("role_default_missing: [roles.{name}] has no default and no inline row");
    }
    Ok(RoleConfig {
        default,
        allowed: raw.allowed.clone().unwrap_or_default(),
        escalate: raw.escalate.clone().unwrap_or_default(),
        machine: raw.machine.clone().unwrap_or_default(),
    })
}

#[derive(Serialize)]
struct PolicyView<'a> {
    recipes: &'a BTreeMap<String, Recipe>,
    roles: &'a BTreeMap<String, RoleConfig>,
}

/// SHA-256 over the recipes and roles.
fn policy_hash(recipes: &BTreeMap<String, Recipe>, roles: &BTreeMap<String, RoleConfig>) -> String {
    use sha2::{Digest, Sha256};
    let view = PolicyView { recipes, roles };
    let bytes = serde_json::to_string(&view).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes.as_bytes());
    format!("{:x}", hasher.finalize())
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

/// Validate the table before any tab or worktree exists.
pub fn validate_config(config: &LaunchConfig, kinds: &BTreeSet<String>) -> Result<()> {
    for (id, recipe) in &config.recipes {
        if is_unresolved_inline(id) {
            continue;
        }
        let kind = recipe.kind.trim();
        if kind.is_empty() {
            bail!("recipe_kind_unknown: recipe `{id}` has no kind");
        }
        if !kinds.contains(kind) {
            bail!(
                "recipe_kind_unknown: recipe `{id}` uses kind {kind:?}, which `herdr agent start` does not accept"
            );
        }
        validate_flags(id, recipe)?;
    }
    for (name, role) in &config.roles {
        if NEVER_RESOLVED.contains(&name.as_str()) {
            continue;
        }
        if role.default.is_empty() {
            bail!("role_default_missing: [roles.{name}] has no default");
        }
        if !role.allowed.contains(&role.default) {
            bail!(
                "role_default_not_allowed: [roles.{name}] default `{}` is not in allowed",
                role.default
            );
        }
        let mut seen = BTreeSet::new();
        for id in &role.allowed {
            let recipe = config
                .recipes
                .get(id)
                .with_context(|| format!("recipe_unknown: [roles.{name}] names `{id}`"))?;
            if !recipe.enabled {
                bail!(
                    "recipe_disabled: `{id}` is enabled = false but is in [roles.{name}].allowed"
                );
            }
            if !seen.insert(id.clone()) {
                bail!("recipe_duplicate: [roles.{name}] names `{id}` twice");
            }
        }
        for id in &role.escalate {
            if !config.recipes.contains_key(id) {
                bail!("recipe_unknown: [roles.{name}] escalate names `{id}`");
            }
        }
    }
    check_reasons_are_plain(config)
}

fn is_unresolved_inline(id: &str) -> bool {
    NEVER_RESOLVED
        .iter()
        .any(|role| format!("{role}_inline") == id)
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

/// Every `plain` phrase and every rendered reason must pass the plain check.
fn check_reasons_are_plain(config: &LaunchConfig) -> Result<()> {
    for (id, recipe) in &config.recipes {
        check_plain(&format!("recipe {id} plain phrase"), &recipe.plain)?;
    }
    for (name, role) in &config.roles {
        for id in &role.allowed {
            let Some(recipe) = config.recipes.get(id) else {
                continue;
            };
            for sentence in [
                pinned_reason(name, &recipe.plain),
                usual_reason(name, &recipe.plain),
            ] {
                check_plain(name, &sentence)?;
            }
        }
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

/// The `--model` value of a recipe's args, from `--model <v>` or `model=<v>`.
pub fn model_value(args: &[&str]) -> Option<String> {
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--model" {
            return args.get(index + 1).map(|value| (*value).to_string());
        }
        if let Some(value) = args[index].strip_prefix("model=") {
            return Some(value.to_string());
        }
        index += 1;
    }
    None
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

/// The pair filter for `dialogue start`: drop every recipe whose model equals
/// the sibling's (kind alone is not the model).
pub fn pair_filter(
    allowed: &[String],
    recipes: &BTreeMap<String, Recipe>,
    sibling: Option<&Launch>,
) -> Vec<String> {
    let Some(sibling) = sibling else {
        return allowed.to_vec();
    };
    let sibling_model = model_value(&sibling.args.iter().map(String::as_str).collect::<Vec<_>>());
    let Some(sibling_model) = sibling_model else {
        return allowed.to_vec();
    };
    allowed
        .iter()
        .filter(|id| {
            recipes.get(*id).is_none_or(|recipe| {
                model_value(&recipe.args.iter().map(String::as_str).collect::<Vec<_>>())
                    .is_none_or(|model| model != sibling_model)
            })
        })
        .cloned()
        .collect()
}

/// The pair filter `dialogue start` uses: the critic role must keep an allowed
/// row whose model differs from the drafter's default, and that row is pinned
/// on the critic's start line. Pro is adopted, never launched, so it pairs
/// with any drafter.
pub struct DialoguePair(pub LaunchConfig);

impl crate::dialogue::PairFilter for DialoguePair {
    fn check(&self, drafter: &str, critic: &str) -> std::result::Result<Option<String>, String> {
        crate::dialogue::same_role(drafter, critic)?;
        if NEVER_RESOLVED.contains(&critic) {
            return Ok(None);
        }
        let config = &self.0;
        let row = |name: &str| {
            config
                .roles
                .get(name)
                .ok_or_else(|| format!("role_unknown: `{name}`"))
        };
        let (_, drafted) =
            role_default(config, row(drafter)?, drafter, None).map_err(|e| format!("{e:#}"))?;
        let sibling = Launch {
            kind: drafted.kind,
            args: drafted.args,
            ..Launch::default()
        };
        role_default(config, row(critic)?, critic, Some(&sibling))
            .map(|(id, _)| Some(id))
            .map_err(|e| format!("{e:#}"))
    }
}

/// What `resolve_launch` needs from the verb.
#[derive(Debug, Default, Clone)]
pub struct ResolveInput<'a> {
    pub role: &'a str,
    /// `--recipe <id>`: pins one row from the role's `allowed` list.
    pub recipe: Option<&'a str>,
    /// PROJECT.md front matter `kind`/`args` pin.
    pub project_pin: Option<Recipe>,
    /// The drafter's resolved launch for a `dialogue start` critic.
    pub sibling: Option<&'a Launch>,
}

/// Resolve one launch before any tab or worktree exists.
pub fn resolve_launch(ctx: &Ctx, input: &ResolveInput) -> Result<Launch> {
    // Pro is started by pro-mcp and adopted; the coordinator is opened by
    // `open`. Neither is ever launched from the table.
    if NEVER_RESOLVED.contains(&input.role) {
        bail!(
            "role_not_resolved: role `{}` is never launched from the roles table",
            input.role
        );
    }
    let config = parse_launch_config(&ctx.config_dir)?;
    let kinds = agent_kinds(ctx.env, ctx.runner)?;
    validate_config(&config, &kinds)?;
    // The role's machine choice overrides its recipe's own row (SPEC-remote
    // §4.1). An empty role row keeps the recipe's `machine`.
    let role_machine = config
        .roles
        .get(input.role)
        .map(|row| row.machine.clone())
        .unwrap_or_default();
    let finish = |mut launch: Launch| -> Launch {
        if !role_machine.is_empty() {
            launch.machine = role_machine.clone();
        }
        launch
    };

    // A `--recipe <id>` pin.
    if let Some(id) = input.recipe {
        let recipe = config
            .recipes
            .get(id)
            .with_context(|| format!("recipe_unknown: `{id}`"))?;
        let role = config
            .roles
            .get(input.role)
            .with_context(|| format!("role_unknown: `{}`", input.role))?;
        if !role.allowed.contains(&id.to_string()) {
            bail!(
                "recipe_not_allowed: `{id}` is not in the allowed list of role `{}`",
                input.role
            );
        }
        let mut launch = launch_from(recipe, &config.policy_hash, input.role);
        launch.recipe_id = id.to_string();
        launch.reason = pinned_reason(input.role, &recipe.plain);
        return Ok(finish(launch));
    }

    let role = config
        .roles
        .get(input.role)
        .with_context(|| format!("role_unknown: `{}`", input.role))?
        .clone();
    let default_id = role.default.clone();
    let default_recipe = config
        .recipes
        .get(&default_id)
        .with_context(|| {
            format!(
                "recipe_unknown: [roles.{}] default `{default_id}`",
                input.role
            )
        })?
        .clone();

    // A PROJECT.md front matter pin for this role.
    if let Some(pin) = &input.project_pin {
        if !pin.kind.trim().is_empty() && pin.kind != default_recipe.kind && pin.args.is_empty() {
            bail!(
                "role_args_missing: [roles.{}] kind changes without args",
                input.role
            );
        }
        if !kinds.contains(pin.kind.trim()) {
            bail!(
                "recipe_kind_unknown: the PROJECT.md pin for role `{}` uses kind {:?}",
                input.role,
                pin.kind
            );
        }
        // A pin that keeps the kind and gives no args keeps the default's args.
        let mut pin = pin.clone();
        if pin.kind.trim().is_empty() {
            pin.kind = default_recipe.kind.clone();
        }
        if pin.args.is_empty() {
            pin.args = default_recipe.args.clone();
        }
        validate_flags(&format!("{}_project", input.role), &pin)?;
        let pin = &pin;
        let plain = if pin.plain.trim().is_empty() {
            &default_recipe.plain
        } else {
            &pin.plain
        };
        let mut launch = launch_from(pin, &config.policy_hash, input.role);
        launch.recipe_id = format!("{}_project", input.role);
        launch.reason = pinned_reason(input.role, plain);
        launch.compact_reason = compact_reason(input.role, plain);
        return Ok(finish(launch));
    }

    // The role's default after the pair filter.
    let (id, recipe) = role_default(&config, &role, input.role, input.sibling)?;
    Ok(finish(default_launch(
        &recipe,
        &id,
        &config.policy_hash,
        input.role,
    )))
}

/// The role's default after the pair filter: the first remaining allowed row
/// in file order when the filter removed it.
fn role_default(
    config: &LaunchConfig,
    role: &RoleConfig,
    name: &str,
    sibling: Option<&Launch>,
) -> Result<(String, Recipe)> {
    let allowed = pair_filter(&role.allowed, &config.recipes, sibling);
    let id = if allowed.contains(&role.default) {
        role.default.clone()
    } else {
        allowed.first().cloned().with_context(|| {
            format!("dialogue_same_model: only the drafter's model remains for role `{name}`")
        })?
    };
    let recipe = config
        .recipes
        .get(&id)
        .with_context(|| format!("recipe_unknown: `{id}`"))?
        .clone();
    Ok((id, recipe))
}

fn launch_from(recipe: &Recipe, policy_hash: &str, role: &str) -> Launch {
    Launch {
        kind: recipe.kind.clone(),
        args: recipe.args.clone(),
        env: recipe.env.clone(),
        ready_timeout_ms: recipe.ready_timeout_ms,
        policy_hash: policy_hash.to_string(),
        attempt: 1,
        brief_hash: String::new(),
        recipe_id: String::new(),
        reason: String::new(),
        compact_reason: compact_reason(role, &recipe.plain),
        machine: recipe.machine.clone(),
    }
}

fn default_launch(recipe: &Recipe, id: &str, policy_hash: &str, role: &str) -> Launch {
    let mut launch = launch_from(recipe, policy_hash, role);
    launch.recipe_id = id.to_string();
    launch.reason = usual_reason(role, &recipe.plain);
    launch
}

/// One `doctor` row: `ok` is `Some(true)`/`Some(false)`/`None` (warn), the
/// same three marks `doctor` prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorRow {
    pub ok: Option<bool>,
    pub label: String,
    pub detail: String,
}

/// The recipe and role rows for `doctor`.
pub fn doctor_rows(ctx: &Ctx) -> Result<Vec<DoctorRow>> {
    let config = parse_launch_config(&ctx.config_dir)?;
    let mut rows = Vec::new();
    for name in &config.inline_without_plain {
        rows.push(DoctorRow {
            ok: None,
            label: format!("role {name}"),
            detail: format!(
                "[roles.{name}] has no plain phrase; the board says \"the usual helper\" until you add one"
            ),
        });
    }
    table_rows(ctx, &config, &mut rows);
    Ok(rows)
}

fn table_rows(ctx: &Ctx, config: &LaunchConfig, rows: &mut Vec<DoctorRow>) {
    match agent_kinds(ctx.env, ctx.runner) {
        Ok(kinds) => match validate_config(config, &kinds) {
            Ok(()) => rows.push(DoctorRow {
                ok: Some(true),
                label: "recipes".into(),
                detail: format!("{} recipe(s) valid", config.recipes.len()),
            }),
            Err(error) => rows.push(DoctorRow {
                ok: Some(false),
                label: "recipes".into(),
                detail: format!("{error:#}"),
            }),
        },
        Err(error) => rows.push(DoctorRow {
            ok: None,
            label: "recipes".into(),
            detail: format!("{error:#}"),
        }),
    }

    let mut checked_kinds = BTreeSet::new();
    for recipe in config.recipes.values().filter(|recipe| recipe.enabled) {
        let kind = recipe.kind.trim();
        if kind.is_empty() || !checked_kinds.insert(kind.to_string()) {
            continue;
        }
        let exe = kind_executable(kind);
        let output = ctx
            .runner
            .run(&Cmd::new("zsh", HELP_TIMEOUT).args(["-lic", &format!("command -v {exe}")]));
        match output {
            Ok(output) if output.success() && !output.stdout.trim().is_empty() => {
                rows.push(DoctorRow {
                    ok: Some(true),
                    label: format!("kind {kind}"),
                    detail: output.stdout.trim().to_string(),
                })
            }
            _ => rows.push(DoctorRow {
                ok: None,
                label: format!("kind {kind}"),
                detail: format!("`{exe}` was not found; the launch will fail for this kind"),
            }),
        }
    }

    let codex_in_a_list = config.roles.values().any(|role| {
        role.default.starts_with("codex_")
            || role.escalate.iter().any(|id| id.starts_with("codex_"))
            || role.allowed.iter().any(|id| id.starts_with("codex_"))
    });
    if codex_in_a_list {
        rows.push(DoctorRow {
            ok: None,
            label: "codex quota".into(),
            detail: "Codex usage is not visible to the plugin; disable the recipe by hand when its weekly use runs out"
                .into(),
        });
    }
    for (id, recipe) in &config.recipes {
        if id.contains("astra") && recipe.enabled {
            rows.push(DoctorRow {
                ok: None,
                label: format!("recipe {id}"),
                detail: "the hardest problem helper is enabled; Codex weekly use is not visible to the plugin"
                    .into(),
            });
        }
    }
}

/// The executable a kind starts, for the doctor's `command -v`.
pub fn kind_executable(kind: &str) -> &str {
    match kind {
        "cursor" => "cursor-agent",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::ok;
    use crate::scenarios::World;

    const KINDS_HELP: &str = "Options:\n      --kind <KIND>\n          [possible values: pi, claude, codex, gemini, cursor, devin, agy, opencode, kimi, muse]\n";

    fn config_text() -> String {
        r#"
[recipes.cursor_grok_xhigh]
kind = "cursor"
provider = "cursor"
args = ["--model", "cursor-grok-4.6-xhigh", "--force"]
env = []
ready_timeout_ms = 30000
plain = "the usual coding helper"

[recipes.claude_opus_high]
kind = "claude"
provider = "claude"
args = ["--model", "claude-opus-5", "--effort", "high", "--dangerously-skip-permissions"]
env = []
ready_timeout_ms = 90000
plain = "the strongest design helper"

[roles.lane]
default = "cursor_grok_xhigh"
allowed = ["cursor_grok_xhigh"]
escalate = ["claude_opus_high"]
"#
        .to_string()
    }

    fn world() -> (World, String) {
        let world = World::new();
        std::fs::create_dir_all(world.home.path().join("cfg")).unwrap();
        std::fs::write(world.home.path().join("cfg/config.toml"), config_text()).unwrap();
        world.runner.on("agent start --help", ok(KINDS_HELP));
        (world, "write the parser".to_string())
    }

    fn run(world: &World, recipe: Option<&str>) -> Result<Launch> {
        resolve_launch(
            &world.ctx(),
            &ResolveInput {
                role: "lane",
                recipe,
                ..ResolveInput::default()
            },
        )
    }

    #[test]
    fn the_role_default_launches() {
        let (world, _task) = world();
        let launch = run(&world, None).unwrap();
        assert_eq!(launch.recipe_id, "cursor_grok_xhigh");
        assert_eq!(launch.kind, "cursor");
        assert_eq!(
            launch.reason,
            "this task runs on the usual coding helper, the usual choice."
        );
        assert_eq!(
            launch.compact_reason,
            "this task runs on the usual coding helper"
        );
    }

    #[test]
    fn a_recipe_pin_uses_the_allowed_row() {
        let (world, _task) = world();
        let launch = run(&world, Some("cursor_grok_xhigh")).unwrap();
        assert_eq!(launch.recipe_id, "cursor_grok_xhigh");
        assert_eq!(
            launch.reason,
            "You chose the usual coding helper for this task."
        );
    }

    #[test]
    fn a_recipe_outside_the_allowed_list_is_refused() {
        let (world, _task) = world();
        let error = run(&world, Some("claude_opus_high")).unwrap_err();
        assert!(
            error.to_string().contains("recipe_not_allowed"),
            "{error:#}"
        );
    }

    #[test]
    fn a_project_pin_keeps_the_default_plain_and_refuses_an_unknown_kind() {
        let (world, _task) = world();
        let ctx = world.ctx();
        let pin = Recipe {
            kind: "claude".into(),
            args: vec![
                "--model".into(),
                "claude-opus-5".into(),
                "--effort".into(),
                "high".into(),
                "--dangerously-skip-permissions".into(),
            ],
            ..Recipe::default()
        };
        let launch = resolve_launch(
            &ctx,
            &ResolveInput {
                role: "lane",
                project_pin: Some(pin),
                ..ResolveInput::default()
            },
        )
        .unwrap();
        assert_eq!(launch.kind, "claude");
        assert_eq!(
            launch.reason,
            "You chose the usual coding helper for this task."
        );

        let pin = Recipe {
            kind: "nope".into(),
            args: vec!["--x".into()],
            ..Recipe::default()
        };
        let error = resolve_launch(
            &ctx,
            &ResolveInput {
                role: "lane",
                project_pin: Some(pin),
                ..ResolveInput::default()
            },
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("recipe_kind_unknown"),
            "{error:#}"
        );
    }

    #[test]
    fn a_project_pin_passes_the_flag_checks() {
        let (world, _task) = world();
        let ctx = world.ctx();
        let resolve = |pin: Recipe| {
            resolve_launch(
                &ctx,
                &ResolveInput {
                    role: "lane",
                    project_pin: Some(pin),
                    ..ResolveInput::default()
                },
            )
        };
        let launch = resolve(Recipe {
            kind: "cursor".into(),
            ..Recipe::default()
        })
        .unwrap();
        assert_eq!(
            launch.args,
            ["--model", "cursor-grok-4.6-xhigh", "--force"],
            "{launch:?}"
        );
        let error = resolve(Recipe {
            kind: "claude".into(),
            ..Recipe::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("role_args_missing"), "{error:#}");
        let error = resolve(Recipe {
            kind: "cursor".into(),
            args: vec!["--model".into(), "cursor-grok-4.6-xhigh".into()],
            ..Recipe::default()
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("recipe_permission_missing"),
            "{error:#}"
        );
        // Opus at xhigh is allowed now; only an unknown effort fails.
        resolve(Recipe {
            kind: "claude".into(),
            args: vec![
                "--model".into(),
                "claude-opus-5".into(),
                "--effort".into(),
                "xhigh".into(),
                "--dangerously-skip-permissions".into(),
            ],
            ..Recipe::default()
        })
        .unwrap();
        let error = resolve(Recipe {
            kind: "claude".into(),
            args: vec![
                "--model".into(),
                "claude-opus-5".into(),
                "--effort".into(),
                "turbo".into(),
                "--dangerously-skip-permissions".into(),
            ],
            ..Recipe::default()
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("recipe_effort_unknown"),
            "{error:#}"
        );
    }

    #[test]
    fn pro_and_the_coordinator_never_resolve() {
        let (world, _task) = world();
        let ctx = world.ctx();
        for role in ["pro", "coordinator"] {
            let error = resolve_launch(
                &ctx,
                &ResolveInput {
                    role,
                    ..ResolveInput::default()
                },
            )
            .unwrap_err();
            assert!(error.to_string().contains("role_not_resolved"), "{error:#}");
        }
    }

    #[test]
    fn builtin_research_and_planner_roles_resolve() {
        let world = World::new();
        std::fs::create_dir_all(world.home.path().join("cfg")).unwrap();
        world.runner.on("agent start --help", ok(KINDS_HELP));
        let ctx = world.ctx();
        let research = resolve_launch(
            &ctx,
            &ResolveInput {
                role: "research",
                ..ResolveInput::default()
            },
        )
        .unwrap();
        assert_eq!(research.recipe_id, "agy_gemini_flash");
        assert_eq!(research.kind, "agy");
        let planner = resolve_launch(
            &ctx,
            &ResolveInput {
                role: "planner",
                ..ResolveInput::default()
            },
        )
        .unwrap();
        assert_eq!(planner.recipe_id, "claude_fable_xhigh");
        assert_eq!(planner.kind, "claude");
        assert!(planner.args.contains(&"xhigh".to_string()));
    }

    #[test]
    fn a_removed_picker_key_is_a_named_error() {
        let home = tempfile::tempdir().unwrap();
        for text in [
            "[roles]\nresolver = \"off\"\n",
            "[roles]\njev_daily_cap = 200\n[roles.lane]\ndefault = \"x\"\nallowed = [\"x\"]\n",
            "[recipes.x]\nkind = \"claude\"\ncost = \"upgrade\"\n",
            "[roles.lane]\ndefault = \"x\"\nallowed = [\"x\"]\n[[roles.lane.gates]]\nrecipe = \"x\"\n",
        ] {
            std::fs::write(home.path().join("config.toml"), text).unwrap();
            let error = parse_launch_config(home.path()).unwrap_err();
            assert!(
                format!("{error:#}").contains("picker_removed"),
                "{text}: {error:#}"
            );
        }
    }

    #[test]
    fn a_role_may_not_mix_an_inline_row_with_a_reference() {
        let mut roles = BTreeMap::new();
        roles.insert(
            "lane".to_string(),
            toml::Value::Table(
                toml::from_str::<toml::Table>("kind = \"claude\"\ndefault = \"x\"\n").unwrap(),
            ),
        );
        let raw = RawConfig {
            roles,
            recipes: BTreeMap::new(),
        };
        let raw_role: RawRole = raw.roles.into_iter().next().unwrap().1.try_into().unwrap();
        let error = parse_role("lane", raw_role, &mut BTreeMap::new()).unwrap_err();
        assert!(error.to_string().contains("role_form_mixed"), "{error:#}");
    }

    #[test]
    fn a_d2_inline_role_becomes_its_own_recipe() {
        let mut recipes = BTreeMap::new();
        let raw = RawRole {
            kind: Some("claude".into()),
            args: Some(vec!["--dangerously-skip-permissions".into()]),
            ..RawRole::default()
        };
        let role = parse_role("lane", raw, &mut recipes).unwrap();
        assert_eq!(role.default, "lane_inline");
        assert_eq!(role.allowed, ["lane_inline"]);
        assert_eq!(recipes["lane_inline"].kind, "claude");
        assert_eq!(recipes["lane_inline"].plain, "the usual helper");
    }

    #[test]
    fn the_reason_templates_are_plain_and_the_compact_form_fits() {
        let glossary = Glossary::default();
        for sentence in [
            usual_reason("lane", "the usual coding helper"),
            pinned_reason("reviewer", "the strongest design helper"),
            usual_reason("research", "the web research helper"),
            usual_reason("planner", "the planning helper"),
        ] {
            let result = plain::check(&sentence, &glossary);
            assert!(result.passed(), "{sentence}: {:?}", result.violations);
        }
        let compact = compact_reason("lane", "the web research helper");
        assert_eq!(compact, "this task runs on the web research helper");
        assert!(compact.len() <= COMPACT_LIMIT);
        let long = compact_reason("lane", &"helper ".repeat(30));
        assert!(long.len() <= COMPACT_LIMIT, "{long}");
    }

    #[test]
    fn the_critic_pair_filter_drops_the_drafters_model() {
        let sibling = Launch {
            args: vec!["--model".into(), "cursor-grok-4.6-xhigh".into()],
            ..Launch::default()
        };
        let allowed = vec![
            "cursor_grok_xhigh".to_string(),
            "claude_opus_high".to_string(),
        ];
        let mut recipes = BTreeMap::new();
        recipes.insert(
            "cursor_grok_xhigh".into(),
            Recipe {
                args: vec!["--model".into(), "cursor-grok-4.6-xhigh".into()],
                ..Recipe::default()
            },
        );
        recipes.insert(
            "claude_opus_high".into(),
            Recipe {
                args: vec!["--model".into(), "claude-opus-5".into()],
                ..Recipe::default()
            },
        );
        assert_eq!(
            pair_filter(&allowed, &recipes, Some(&sibling)),
            ["claude_opus_high"]
        );
        assert_eq!(pair_filter(&allowed, &recipes, None), allowed);

        use crate::dialogue::PairFilter;
        let row = |default: &str| RoleConfig {
            default: default.into(),
            allowed: allowed.clone(),
            ..RoleConfig::default()
        };
        let mut config = parse_launch_config(Path::new("/nonexistent")).unwrap();
        config.recipes = recipes;
        config
            .roles
            .insert("drafter".into(), row("cursor_grok_xhigh"));
        config
            .roles
            .insert("critic".into(), row("cursor_grok_xhigh"));
        let pair = DialoguePair(config);
        assert_eq!(
            pair.check("drafter", "critic").unwrap().as_deref(),
            Some("claude_opus_high")
        );
        assert_eq!(pair.check("drafter", "pro").unwrap(), None);
        assert!(
            pair.check("drafter", "ghost")
                .unwrap_err()
                .contains("role_unknown")
        );
    }

    #[test]
    fn validation_refuses_bad_tables() {
        let kinds = parse_kinds(KINDS_HELP).unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), config_text()).unwrap();
        let mut config = parse_launch_config(home.path()).unwrap();

        let mut bad = config.clone();
        bad.recipes.get_mut("cursor_grok_xhigh").unwrap().kind = "nope".into();
        let error = validate_config(&bad, &kinds).unwrap_err();
        assert!(
            error.to_string().contains("recipe_kind_unknown"),
            "{error:#}"
        );

        let mut bad = config.clone();
        bad.recipes.get_mut("cursor_grok_xhigh").unwrap().args =
            vec!["--model".into(), "cursor-grok-4.6-xhigh".into()];
        let error = validate_config(&bad, &kinds).unwrap_err();
        assert!(
            error.to_string().contains("recipe_permission_missing"),
            "{error:#}"
        );

        let mut bad = config.clone();
        bad.roles.get_mut("lane").unwrap().default = "claude_opus_high".into();
        bad.roles.get_mut("lane").unwrap().allowed = vec!["cursor_grok_xhigh".into()];
        let error = validate_config(&bad, &kinds).unwrap_err();
        assert!(
            error.to_string().contains("role_default_not_allowed"),
            "{error:#}"
        );

        config.recipes.get_mut("cursor_grok_xhigh").unwrap().plain = "the zorbulate helper".into();
        let error = validate_config(&config, &kinds).unwrap_err();
        assert!(
            error.to_string().contains("recipe_reason_not_plain"),
            "{error:#}"
        );
    }

    #[test]
    fn doctor_warns_on_an_inline_role_without_plain_and_lists_recipes() {
        let world = World::new();
        std::fs::create_dir_all(world.home.path().join("cfg")).unwrap();
        std::fs::write(
            world.home.path().join("cfg/config.toml"),
            "[roles.lane]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\n\n[roles.pro]\nkind = \"chatgpt\"\n",
        )
        .unwrap();
        world.runner.on("agent start --help", ok(KINDS_HELP));
        world.runner.on_fn(
            |cmd| cmd.display().contains("zsh -lic"),
            |_| Ok(ok("/usr/local/bin/claude\n")),
        );
        let ctx = world.ctx();
        let config = parse_launch_config(&ctx.config_dir).unwrap();
        assert_eq!(config.inline_without_plain, ["lane"]);
        let rows = doctor_rows(&ctx).unwrap();
        let lane = rows.iter().find(|row| row.label == "role lane").unwrap();
        assert_eq!(lane.ok, None, "{lane:?}");
        assert!(
            rows.iter()
                .any(|row| row.label == "recipes" && row.ok == Some(true)),
            "{rows:?}"
        );
    }
}
