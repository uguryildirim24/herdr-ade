//! Launch-time resolution: validate the recipe table, run the picker's gates
//! and return the `Launch` object the thread record stores
//! (SPEC-jev-picker v2 §2 and §3).
//!
//! `resolve_launch` takes the project lock itself for the daily cap and for the
//! config re-read, and releases it before any Jev call: the caller must not
//! hold the project lock when it calls this module (SPEC-jev-picker v2 §3
//! step 5).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::{CostClass, Gate, Launch, Recipe, ResolverMode};
use crate::jev::{self, Question};
use crate::paths::{Ctx, Env};
use crate::plain::{self, Glossary};
use crate::project::Project;
use crate::runner::{Cmd, Runner};

/// `[roles]` defaults (SPEC-jev-picker v2 §2 Config).
pub const DEFAULT_JEV_TIMEOUT_MS: u64 = 3_000;
pub const DEFAULT_JEV_DAILY_CAP: u64 = 200;
pub const DEFAULT_FLOOR_SIDEWAYS: f64 = 0.50;
pub const DEFAULT_FLOOR_UPGRADE: f64 = 0.70;
/// `herdr agent start --help` is a table check, not a launch.
pub const HELP_TIMEOUT: Duration = Duration::from_secs(10);

/// Roles that never call the resolver (SPEC-jev-picker v2 §2).
pub const NEVER_RESOLVED: [&str; 2] = ["pro", "coordinator"];
/// Reserved scalar keys under `[roles]`.
pub const RESERVED_ROLES: [&str; 5] = [
    "resolver",
    "jev_model",
    "jev_timeout_ms",
    "jev_daily_cap",
    "floor",
];

/// Fallback vocabulary (SPEC-jev-picker v2 §2 Output, §3 step 6).
pub const FALLBACK_OVERRIDE: &str = "override";
pub const FALLBACK_PROJECT: &str = "project";
pub const FALLBACK_SHADOW: &str = "shadow";
pub const FALLBACK_RESOLVER_OFF: &str = "resolver_off";
pub const FALLBACK_NOT_OPTED_IN: &str = "not_opted_in";
pub const FALLBACK_SINGLE_ROW: &str = "single_row";
pub const FALLBACK_NO_GATE: &str = "no_gate";
pub const FALLBACK_DAILY_CAP: &str = "daily_cap";
pub const FALLBACK_NO_KEY: &str = "no_key";
pub const FALLBACK_NO_CURL: &str = "no_curl";
pub const FALLBACK_TIMEOUT: &str = "timeout";
pub const FALLBACK_MALFORMED: &str = "malformed";
pub const FALLBACK_MODEL_MISMATCH: &str = "model_mismatch";
pub const FALLBACK_ERROR: &str = "error";
pub const FALLBACK_NOT_ALLOWED: &str = "not_allowed";
pub const FALLBACK_CONFIG_CHANGED: &str = "config_changed";

/// Reason templates, fixed in the binary and checked at load
/// (SPEC-jev-picker v2 §3, "Where the reason shows").
pub const TEMPLATE_PICKED: &str = "{job} looks like {clause}, so it runs on {plain}.";
pub const TEMPLATE_DEFAULT: &str = "{job} looks like ordinary work, so it runs on {plain}.";
pub const TEMPLATE_PINNED: &str = "You chose {plain} for {job}.";
pub const TEMPLATE_FALLBACK: &str =
    "{job} runs on {plain}, the usual choice, because the picker did not answer.";
pub const TEMPLATE_SHADOW: &str = "{job} runs on {plain}; the picker would have chosen {pick}.";
/// The compact `ade_last` form (SPEC-jev-picker v2 §3 Publication).
pub const COMPACT_LIMIT: usize = 80;

/// The cost-class floors for gate thresholds (SPEC-jev-picker v2 §2 Design C).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Floor {
    pub sideways: f64,
    pub upgrade: f64,
}

impl Default for Floor {
    fn default() -> Self {
        Floor {
            sideways: DEFAULT_FLOOR_SIDEWAYS,
            upgrade: DEFAULT_FLOOR_UPGRADE,
        }
    }
}

impl Floor {
    /// The floor a gate's cost class names.
    pub fn of(&self, cost: CostClass) -> f64 {
        match cost {
            CostClass::Sideways | CostClass::Default => self.sideways,
            CostClass::Upgrade => self.upgrade,
        }
    }
}

/// One role's picker settings (`default`, `allowed`, `escalate`, `gates`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RolePicker {
    pub default: String,
    pub allowed: Vec<String>,
    pub escalate: Vec<String>,
    pub gates: Vec<Gate>,
}

/// The picker's view of `~/.config/herdr-ade/config.toml`
/// (SPEC-jev-picker v2 §2 Config).
#[derive(Debug, Clone, PartialEq)]
pub struct PickerConfig {
    pub resolver: ResolverMode,
    pub jev_model: String,
    pub jev_timeout_ms: u64,
    pub jev_daily_cap: u64,
    pub floor: Floor,
    pub recipes: BTreeMap<String, Recipe>,
    pub roles: BTreeMap<String, RolePicker>,
    /// PROJECT.md opt-in (SPEC-jev-picker v2 question 13), default off.
    pub opted_in: bool,
    /// Covers the resolver fields, recipes, lists, gate text, thresholds,
    /// redaction and excerpt rules, and `jev_model` (SPEC-jev-picker v2
    /// Design norm 8, SPEC-ADE D11).
    pub policy_hash: String,
    /// Inline D2 roles written without `plain`: they get the shipped phrase
    /// and `doctor` warns (SPEC-ADE §6 item 47), never a refusal.
    pub inline_without_plain: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    resolver: ResolverMode,
    jev_model: String,
    jev_timeout_ms: u64,
    jev_daily_cap: u64,
    floor: Floor,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            resolver: ResolverMode::Off,
            jev_model: jev::JEV_MODEL.to_string(),
            jev_timeout_ms: DEFAULT_JEV_TIMEOUT_MS,
            jev_daily_cap: DEFAULT_JEV_DAILY_CAP,
            floor: Floor::default(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    roles: BTreeMap<String, toml::Value>,
    #[serde(default)]
    recipes: BTreeMap<String, Recipe>,
}

/// A `[roles.<name>]` table: either an inline D2 row or a recipe reference.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRole {
    kind: Option<String>,
    args: Option<Vec<String>>,
    env: Option<Vec<String>>,
    ready_timeout_ms: Option<u64>,
    provider: Option<String>,
    cost: Option<CostClass>,
    enabled: Option<bool>,
    plain: Option<String>,
    default: Option<String>,
    allowed: Option<Vec<String>>,
    escalate: Option<Vec<String>>,
    gates: Option<Vec<Gate>>,
}

/// Parse the safety file's picker tables. An absent file is the shipped
/// default: `resolver = "off"` and an empty table.
pub fn parse_picker_config(config_dir: &Path, opted_in: bool) -> Result<PickerConfig> {
    let file = config_dir.join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let raw: RawConfig = if text.trim().is_empty() {
        RawConfig::default()
    } else {
        toml::from_str(&text).with_context(|| format!("{} does not parse", file.display()))?
    };
    let mut settings = Settings::default();
    let mut role_tables = raw.roles;
    for key in RESERVED_ROLES {
        if let Some(value) = role_tables.remove(key) {
            settings.set(key, value, &file)?;
        }
    }
    let mut recipes = raw.recipes;
    let mut roles = BTreeMap::new();
    let mut inline_without_plain = Vec::new();
    for (name, value) in role_tables {
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
    let mut config = finish(settings, recipes, roles, opted_in)?;
    config.inline_without_plain = inline_without_plain;
    Ok(config)
}

fn finish(
    settings: Settings,
    recipes: BTreeMap<String, Recipe>,
    roles: BTreeMap<String, RolePicker>,
    opted_in: bool,
) -> Result<PickerConfig> {
    if settings.jev_timeout_ms == 0 {
        bail!("jev_timeout_ms must be at least 1");
    }
    for (name, floor) in [
        ("sideways", settings.floor.sideways),
        ("upgrade", settings.floor.upgrade),
    ] {
        if !(0.0..=1.0).contains(&floor) || floor == 0.0 {
            bail!("floor.{name} must be more than 0 and at most 1");
        }
    }
    let policy_hash = policy_hash(&settings, &recipes, &roles);
    Ok(PickerConfig {
        resolver: settings.resolver,
        jev_model: settings.jev_model,
        jev_timeout_ms: settings.jev_timeout_ms,
        jev_daily_cap: settings.jev_daily_cap,
        floor: settings.floor,
        recipes,
        roles,
        opted_in,
        policy_hash,
        inline_without_plain: Vec::new(),
    })
}

impl Settings {
    fn set(&mut self, key: &str, value: toml::Value, file: &Path) -> Result<()> {
        match key {
            "resolver" => {
                let text = value.as_str().with_context(|| {
                    format!("{}: [roles] {key} must be a string", file.display())
                })?;
                self.resolver = match text {
                    "off" => ResolverMode::Off,
                    "shadow" => ResolverMode::Shadow,
                    // The first plugin round ships `off` and `shadow` only: no
                    // launch follows a pick until the second labelled month
                    // passes (SPEC-jev-picker v2 §4.1, question 1; lane brief).
                    "jev" => bail!(
                        "resolver_mode_unavailable: {}: [roles] resolver = \"jev\" is not in this plugin round; use \"shadow\" to record picks",
                        file.display()
                    ),
                    other => bail!(
                        "{}: [roles] resolver must be \"off\" or \"shadow\", not {other:?}",
                        file.display()
                    ),
                };
            }
            "jev_model" => {
                let text = value.as_str().with_context(|| {
                    format!("{}: [roles] {key} must be a string", file.display())
                })?;
                if text.trim().is_empty() {
                    bail!("{}: [roles] jev_model is empty", file.display());
                }
                self.jev_model = text.to_string();
            }
            "jev_timeout_ms" => {
                let ms = value.as_integer().with_context(|| {
                    format!("{}: [roles] {key} must be a whole number", file.display())
                })?;
                self.jev_timeout_ms = u64::try_from(ms).with_context(|| {
                    format!("{}: [roles] {key} must be positive", file.display())
                })?;
            }
            "jev_daily_cap" => {
                let cap = value.as_integer().with_context(|| {
                    format!("{}: [roles] {key} must be a whole number", file.display())
                })?;
                self.jev_daily_cap = u64::try_from(cap).with_context(|| {
                    format!("{}: [roles] {key} must be positive", file.display())
                })?;
            }
            "floor" => {
                let table = value.as_table().with_context(|| {
                    format!("{}: [roles] floor must be a table", file.display())
                })?;
                if let Some(sideways) = table.get("sideways").and_then(|v| v.as_float()) {
                    self.floor.sideways = sideways;
                }
                if let Some(upgrade) = table.get("upgrade").and_then(|v| v.as_float()) {
                    self.floor.upgrade = upgrade;
                }
            }
            _ => bail!("unknown [roles] setting `{key}`"),
        }
        Ok(())
    }
}

fn parse_role(
    name: &str,
    raw: RawRole,
    recipes: &mut BTreeMap<String, Recipe>,
) -> Result<RolePicker> {
    let inline = raw.kind.is_some();
    let reference = raw.default.is_some()
        || raw.allowed.is_some()
        || raw.escalate.is_some()
        || raw.gates.is_some();
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
            cost: raw.cost.unwrap_or_default(),
            enabled: raw.enabled.unwrap_or(true),
            plain: raw
                .plain
                .clone()
                .filter(|plain| !plain.trim().is_empty())
                .unwrap_or_else(|| "the usual helper".to_string()),
        };
        recipes.insert(id.clone(), recipe);
        return Ok(RolePicker {
            default: id.clone(),
            allowed: vec![id],
            escalate: Vec::new(),
            gates: Vec::new(),
        });
    }
    let default = raw.default.clone().unwrap_or_default();
    if default.trim().is_empty() {
        bail!("role_default_missing: [roles.{name}] has no default and no inline row");
    }
    Ok(RolePicker {
        default,
        allowed: raw.allowed.clone().unwrap_or_default(),
        escalate: raw.escalate.clone().unwrap_or_default(),
        gates: raw.gates.clone().unwrap_or_default(),
    })
}

#[derive(Serialize)]
struct PolicyView<'a> {
    resolver: ResolverMode,
    jev_model: &'a str,
    jev_timeout_ms: u64,
    jev_daily_cap: u64,
    floor: Floor,
    recipes: &'a BTreeMap<String, Recipe>,
    roles: &'a BTreeMap<String, RolePicker>,
    excerpt_version: u32,
    redaction: String,
}

/// SHA-256 over the picker-relevant config and the fixed redaction and reason
/// rules (SPEC-jev-picker v2 Design norm 8).
fn policy_hash(
    settings: &Settings,
    recipes: &BTreeMap<String, Recipe>,
    roles: &BTreeMap<String, RolePicker>,
) -> String {
    let scrub = jev::ScrubList::new(recipes);
    let view = PolicyView {
        resolver: settings.resolver,
        jev_model: &settings.jev_model,
        jev_timeout_ms: settings.jev_timeout_ms,
        jev_daily_cap: settings.jev_daily_cap,
        floor: settings.floor,
        recipes,
        roles,
        excerpt_version: jev::EXCERPT_VERSION,
        redaction: scrub.fingerprint(),
    };
    jev::sha256_hex(serde_json::to_string(&view).unwrap_or_default().as_bytes())
}

/// The kinds `herdr agent start` accepts, read from its `--help`
/// (SPEC-jev-picker v2 §2 Validation).
pub fn agent_kinds(env: &Env, runner: &dyn Runner) -> Result<BTreeSet<String>> {
    let bin = env.herdr_bin();
    let output = runner
        .run(&Cmd::new(&bin, HELP_TIMEOUT).args(["agent", "start", "--help"]))
        .with_context(|| format!("could not run `{bin} agent start --help`"))?;
    let text = if output.stdout.trim().is_empty() {
        output.stderr.as_str()
    } else {
        output.stdout.as_str()
    };
    jev::parse_kinds(text).context("`herdr agent start --help` did not list kinds")
}

/// Validate the table before any tab or worktree exists
/// (SPEC-jev-picker v2 §2 Validation).
pub fn validate_config(config: &PickerConfig, kinds: &BTreeSet<String>) -> Result<()> {
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
        let mut gate_seen = BTreeSet::new();
        for gate in &role.gates {
            if gate.recipe == role.default {
                bail!(
                    "gate_is_default: [roles.{name}] gate `{}` is the default",
                    gate.recipe
                );
            }
            if !role.allowed.contains(&gate.recipe) {
                bail!(
                    "gate_not_allowed: [roles.{name}] gate `{}` is not in allowed",
                    gate.recipe
                );
            }
            let Some(recipe) = config.recipes.get(&gate.recipe) else {
                bail!(
                    "recipe_unknown: [roles.{name}] gate names `{}`",
                    gate.recipe
                );
            };
            if !recipe.enabled {
                bail!(
                    "recipe_disabled: [roles.{name}] gate `{}` is enabled = false",
                    gate.recipe
                );
            }
            if gate.cost == CostClass::Default {
                bail!(
                    "gate_cost_missing: [roles.{name}] gate `{}` has no cost class",
                    gate.recipe
                );
            }
            if let Some(threshold) = gate.threshold
                && !(threshold > 0.0 && threshold <= 1.0)
            {
                bail!(
                    "gate_threshold_invalid: [roles.{name}] gate `{}` threshold {threshold}",
                    gate.recipe
                );
            }
            if gate.instructions.trim().is_empty() {
                bail!(
                    "gate_instructions_missing: [roles.{name}] gate `{}`",
                    gate.recipe
                );
            }
            if gate.criteria.is_true.trim().is_empty() || gate.criteria.is_false.trim().is_empty() {
                bail!(
                    "gate_criteria_missing: [roles.{name}] gate `{}`",
                    gate.recipe
                );
            }
            if !gate_seen.insert(gate.recipe.clone()) {
                bail!(
                    "gate_duplicate: [roles.{name}] gates `{}` twice",
                    gate.recipe
                );
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
        "claude" | "agy" => {
            let model = model_value(&args).unwrap_or_default().to_ascii_lowercase();
            if model.contains("opus") || model.contains("fable") {
                if effort_value(&args).as_deref() != Some("high") {
                    bail!(
                        "recipe_effort_forbidden: `{id}` names Opus or Fable without `--effort high`"
                    );
                }
            }
            if !has("--dangerously-skip-permissions")
                && !has("--dangerously-bypass-approvals-and-sandbox")
            {
                bail!("recipe_permission_missing: `{id}` has no permission flag");
            }
        }
        "cursor" => {
            if !has("--force") && !has("--yolo") {
                bail!("recipe_permission_missing: `{id}` has no permission flag");
            }
        }
        _ => {}
    }
    Ok(())
}

/// Every `plain` phrase and every rendered reason template must pass the
/// plain check as a birth sentence (`recipe_reason_not_plain`).
fn check_reasons_are_plain(config: &PickerConfig) -> Result<()> {
    for (id, recipe) in &config.recipes {
        check_plain(&format!("recipe {id} plain phrase"), &recipe.plain)?;
    }
    for (name, role) in &config.roles {
        for id in &role.allowed {
            let Some(recipe) = config.recipes.get(id) else {
                continue;
            };
            for sentence in [
                default_reason(name, &recipe.plain),
                pinned_reason(name, &recipe.plain),
                fallback_reason(name, &recipe.plain),
            ] {
                check_plain(name, &sentence)?;
            }
        }
        for gate in &role.gates {
            let Some(recipe) = config.recipes.get(&gate.recipe) else {
                continue;
            };
            if let Some(clause) = reason_clause(&gate.recipe) {
                check_plain(name, &picked_reason(name, clause, &recipe.plain))?;
            }
            if let Some(default) = config.recipes.get(&role.default) {
                check_plain(name, &shadow_reason(name, &default.plain, &recipe.plain))?;
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
        if args[index] == "--model" || args[index] == "-m" {
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
        if let Some(value) = args[index].strip_prefix("--effort=") {
            return Some(value.to_string());
        }
        index += 1;
    }
    None
}

/// The job noun of each role (SPEC-jev-picker v2 §3).
pub fn job_noun(role: &str) -> &'static str {
    match role {
        "reviewer" => "this review",
        "critic" => "this second opinion",
        "drafter" => "this draft",
        "research" => "this lookup",
        _ => "this task",
    }
}

/// The reason clause of each shipped non-default recipe, fixed in the binary
/// (SPEC-jev-picker v2 §3).
pub fn reason_clause(recipe_id: &str) -> Option<&'static str> {
    match recipe_id {
        "claude_opus_high" => Some("design or spec work"),
        "codex_sol_high" => Some("number or engine work"),
        "codex_astra_high" => Some("the hardest number work"),
        "agy_gemini_flash" => Some("web research"),
        "claude_fable_high" => Some("a second opinion on a draft"),
        _ => None,
    }
}

fn render(template: &str, job: &str, clause: &str, plain: &str, pick: &str) -> String {
    template
        .replace("{job}", job)
        .replace("{clause}", clause)
        .replace("{plain}", plain)
        .replace("{pick}", pick)
}

pub fn picked_reason(role: &str, clause: &str, plain: &str) -> String {
    render(TEMPLATE_PICKED, job_noun(role), clause, plain, "")
}

pub fn default_reason(role: &str, plain: &str) -> String {
    render(TEMPLATE_DEFAULT, job_noun(role), "", plain, "")
}

pub fn pinned_reason(role: &str, plain: &str) -> String {
    render(TEMPLATE_PINNED, job_noun(role), "", plain, "")
}

pub fn fallback_reason(role: &str, plain: &str) -> String {
    render(TEMPLATE_FALLBACK, job_noun(role), "", plain, "")
}

pub fn shadow_reason(role: &str, plain: &str, pick: &str) -> String {
    render(TEMPLATE_SHADOW, job_noun(role), "", plain, pick)
}

/// `"<job> runs on <plain>"`, at most 80 characters: the ticker's `ade_last`
/// token (SPEC-jev-picker v2 §3 Publication).
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
/// the sibling's (kind alone is not the model)
/// (SPEC-jev-picker v2 §2 Pairs).
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

/// What `resolve_launch` needs from the verb.
#[derive(Debug, Default, Clone)]
pub struct ResolveInput<'a> {
    pub role: &'a str,
    pub task: &'a str,
    /// `--recipe <id>` from the verb (SPEC-jev-picker v2 §3 step 1).
    pub recipe: Option<&'a str>,
    /// PROJECT.md front matter `kind`/`args` pin (decision 3, §3 step 2).
    pub project_pin: Option<Recipe>,
    /// The drafter's resolved launch for a `dialogue start` critic (§2 Pairs).
    pub sibling: Option<&'a Launch>,
    /// The project has opted in (`jev = true`, question 13).
    pub opted_in: bool,
    pub round: Option<&'a str>,
    pub title: Option<&'a str>,
    /// The `--plain` birth sentence (D17 item 6).
    pub sentence: Option<&'a str>,
    /// Repository basename for the state.
    pub repo: Option<&'a str>,
    /// The role's policy lines, verbatim, for the gate instructions.
    pub policy: Option<&'a str>,
}

/// Resolve one launch before any tab or worktree exists
/// (SPEC-jev-picker v2 §2 and §3).
pub fn resolve_launch(ctx: &Ctx, project: &Project, input: &ResolveInput) -> Result<Launch> {
    // Pro is started by pro-mcp and adopted; the coordinator is opened by
    // `open`. Neither is ever launched from the table (SPEC-jev-picker v2 §2,
    // SPEC-ADE D7).
    if NEVER_RESOLVED.contains(&input.role) {
        bail!(
            "role_not_resolved: role `{}` is never launched by the picker",
            input.role
        );
    }
    let config = parse_picker_config(&ctx.config_dir, input.opted_in)?;
    let kinds = agent_kinds(ctx.env, ctx.runner)?;
    validate_config(&config, &kinds)?;

    // Step 1: `--recipe <id>` pins; Jev is skipped (decision 13 / D2).
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
        let mut launch = launch_from(recipe, &config, ResolverMode::Pin, input.role);
        launch.recipe_id = id.to_string();
        launch.reason = pinned_reason(input.role, &recipe.plain);
        launch.fallback = Some(FALLBACK_OVERRIDE.to_string());
        return Ok(launch);
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

    // Step 2: a PROJECT.md front matter pin for this role (decision 3).
    if let Some(pin) = &input.project_pin {
        if pin.kind != default_recipe.kind && pin.args.is_empty() {
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
        // A pin that keeps the kind and gives no args keeps the default's args
        // (D2: only a kind change must bring its own args). The pinned row then
        // passes the same flag checks as a table row, so a pin cannot drop a
        // permission flag or name Opus without `--effort high`.
        let mut pin = pin.clone();
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
        let mut launch = launch_from(pin, &config, ResolverMode::Pin, input.role);
        launch.recipe_id = format!("{}_project", input.role);
        launch.reason = pinned_reason(input.role, plain);
        launch.compact_reason = compact_reason(input.role, plain);
        launch.fallback = Some(FALLBACK_PROJECT.to_string());
        return Ok(launch);
    }

    // The task may name a model; the plugin warns and resolves as usual
    // (question 16), never silently following it.
    let scrub = jev::ScrubList::new(&config.recipes);
    if scrub.names_a_model(input.task).is_some() {
        eprintln!("task names a model; pass --recipe to pin it");
    }

    // The pair filter runs before the picker, in every mode (§2 Pairs).
    let allowed = pair_filter(&role.allowed, &config.recipes, input.sibling);
    let (default_id, default_recipe) = role_default(&config, &role, input.role, input.sibling)?;

    // Step 4: off, not opted in, one row or no gates never call Jev.
    if config.resolver == ResolverMode::Off {
        return Ok(default_launch(
            &default_recipe,
            &default_id,
            &config,
            input.role,
            FALLBACK_RESOLVER_OFF,
        ));
    }
    if !config.opted_in {
        return Ok(default_launch(
            &default_recipe,
            &default_id,
            &config,
            input.role,
            FALLBACK_NOT_OPTED_IN,
        ));
    }
    let gates: Vec<Gate> = role
        .gates
        .iter()
        .filter(|gate| allowed.contains(&gate.recipe))
        .cloned()
        .collect();
    if allowed.len() <= 1 {
        return Ok(default_launch(
            &default_recipe,
            &default_id,
            &config,
            input.role,
            FALLBACK_SINGLE_ROW,
        ));
    }
    if gates.is_empty() {
        return Ok(default_launch(
            &default_recipe,
            &default_id,
            &config,
            input.role,
            FALLBACK_NO_GATE,
        ));
    }

    // Step 5: the key, the daily cap and the one call per resolve.
    let Some(key) = jev::load_key(ctx.env) else {
        return Ok(fallback_launch(
            &default_recipe,
            &default_id,
            &config,
            input.role,
            FALLBACK_NO_KEY,
        ));
    };
    if !project_take_daily_call(project, config.jev_daily_cap)? {
        return Ok(fallback_launch(
            &default_recipe,
            &default_id,
            &config,
            input.role,
            FALLBACK_DAILY_CAP,
        ));
    }

    let questions = gate_questions(&gates, &config.floor);
    let state = jev::build_state(&jev::StateInput {
        task: jev::transform_task(input.task, &scrub),
        title: input.title.map(str::to_string),
        sentence: input.sentence.map(str::to_string),
        role: input.role.to_string(),
        round: input.round.map(str::to_string),
        project_name: Some(project_display_name(project)),
        project_goal: project_goal(project),
        repo: input.repo.map(str::to_string),
        policy: input.policy.map(str::to_string),
    });
    let body = jev::request_body(&state, &questions, &config.jev_model);
    let prompt_hash = jev::prompt_hash(&questions, jev::EXCERPT_VERSION);
    let budget = Duration::from_millis(config.jev_timeout_ms);
    let call = jev::call(ctx.runner, &key, &body, budget);

    // Step 6: acceptance.
    let (mut launch, answers) = match &call.transport {
        jev::Transport::NoCurl => (
            fallback_launch(
                &default_recipe,
                &default_id,
                &config,
                input.role,
                FALLBACK_NO_CURL,
            ),
            None,
        ),
        jev::Transport::Timeout | jev::Transport::Failed { .. } => (
            fallback_launch(
                &default_recipe,
                &default_id,
                &config,
                input.role,
                FALLBACK_TIMEOUT,
            ),
            None,
        ),
        jev::Transport::Response { status, body } => {
            if *status != 200 {
                let fallback = format!("http_{status}");
                (
                    fallback_launch(&default_recipe, &default_id, &config, input.role, &fallback),
                    None,
                )
            } else {
                match jev::parse_body(body, &config.jev_model) {
                    jev::Parsed::Malformed => (
                        fallback_launch(
                            &default_recipe,
                            &default_id,
                            &config,
                            input.role,
                            FALLBACK_MALFORMED,
                        ),
                        None,
                    ),
                    jev::Parsed::ModelMismatch { model } => {
                        let mut launch = fallback_launch(
                            &default_recipe,
                            &default_id,
                            &config,
                            input.role,
                            FALLBACK_MODEL_MISMATCH,
                        );
                        launch.jev_model = Some(model);
                        (launch, None)
                    }
                    jev::Parsed::Answers(answers) => {
                        if questions
                            .iter()
                            .any(|question| !answers.nouls.contains_key(&question.id))
                        {
                            (
                                fallback_launch(
                                    &default_recipe,
                                    &default_id,
                                    &config,
                                    input.role,
                                    FALLBACK_ERROR,
                                ),
                                Some(answers),
                            )
                        } else {
                            let launch = accepted_launch(
                                &config,
                                input,
                                &questions,
                                &answers,
                                &default_recipe,
                                &default_id,
                            );
                            (launch, Some(answers))
                        }
                    }
                }
            }
        }
    };
    launch.jev_prompt_hash = prompt_hash;
    launch.excerpt_version = jev::EXCERPT_VERSION;
    if let Some(answers) = answers {
        if launch.jev_model.is_none() {
            launch.jev_model = Some(answers.model);
        }
        launch.jev_input_tokens = answers.input_tokens;
        launch.jev_probabilities = answers.nouls.clone();
        launch.jev_confidence = answers
            .nouls
            .values()
            .copied()
            .fold(None, |max: Option<f64>, p| {
                Some(max.map_or(p, |m| m.max(p)))
            });
    }

    // Step 8: retake the project lock and re-read the config; if it moved
    // while the call was in flight, discard the answer and launch the new
    // table's default, validated like any other.
    let after = {
        let _lock = project.lock()?;
        parse_picker_config(&ctx.config_dir, input.opted_in)?
    };
    if after.policy_hash != config.policy_hash {
        validate_config(&after, &kinds)?;
        let role = after
            .roles
            .get(input.role)
            .with_context(|| format!("role_unknown: `{}`", input.role))?;
        let (id, recipe) = role_default(&after, role, input.role, input.sibling)?;
        return Ok(fallback_launch(
            &recipe,
            &id,
            &after,
            input.role,
            FALLBACK_CONFIG_CHANGED,
        ));
    }
    Ok(launch)
}

/// The role's default after the pair filter: the first remaining allowed row
/// in file order when the filter removed it (SPEC-jev-picker v2 §2 Pairs).
fn role_default(
    config: &PickerConfig,
    role: &RolePicker,
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

fn accepted_launch(
    config: &PickerConfig,
    input: &ResolveInput,
    questions: &[Question],
    answers: &jev::Answers,
    default_recipe: &Recipe,
    default_id: &str,
) -> Launch {
    let shadow = config.resolver == ResolverMode::Shadow;
    let Some(question) = jev::fired_gate(questions, &answers.nouls) else {
        let mut launch = launch_from(default_recipe, config, config.resolver, input.role);
        launch.recipe_id = default_id.to_string();
        launch.reason = default_reason(input.role, &default_recipe.plain);
        launch.fallback = Some(
            if shadow {
                FALLBACK_SHADOW
            } else {
                FALLBACK_NO_GATE
            }
            .to_string(),
        );
        return launch;
    };
    let picked_plain = config
        .recipes
        .get(&question.id)
        .map(|recipe| recipe.plain.as_str())
        .unwrap_or_default();
    if shadow {
        let mut launch = launch_from(default_recipe, config, ResolverMode::Shadow, input.role);
        launch.recipe_id = default_id.to_string();
        launch.gate = Some(question.id.clone());
        launch.gate_p = answers.nouls.get(&question.id).copied();
        launch.jev_pick = Some(question.id.clone());
        launch.reason = shadow_reason(input.role, &default_recipe.plain, picked_plain);
        launch.fallback = Some(FALLBACK_SHADOW.to_string());
        return launch;
    }
    let allowed = config
        .roles
        .get(input.role)
        .map(|role| role.allowed.contains(&question.id))
        .unwrap_or(false);
    if !allowed {
        return fallback_launch(
            default_recipe,
            default_id,
            config,
            input.role,
            FALLBACK_NOT_ALLOWED,
        );
    }
    let recipe = config.recipes.get(&question.id).unwrap_or(default_recipe);
    let mut launch = launch_from(recipe, config, ResolverMode::Jev, input.role);
    launch.recipe_id = question.id.clone();
    launch.gate = Some(question.id.clone());
    launch.gate_p = answers.nouls.get(&question.id).copied();
    launch.jev_pick = Some(question.id.clone());
    launch.reason = reason_clause(&question.id)
        .map(|clause| picked_reason(input.role, clause, picked_plain))
        .unwrap_or_else(|| default_reason(input.role, picked_plain));
    launch
}

fn project_display_name(project: &Project) -> String {
    project
        .read_project_md()
        .ok()
        .map(|(settings, _)| settings.name)
        .unwrap_or_default()
}

fn project_goal(project: &Project) -> Option<String> {
    project
        .read_project_md()
        .ok()
        .map(|(settings, _)| settings.goal)
        .filter(|goal| !goal.trim().is_empty())
}

fn gate_questions(gates: &[Gate], floor: &Floor) -> Vec<Question> {
    gates
        .iter()
        .map(|gate| Question {
            id: gate.recipe.clone(),
            instructions: gate.instructions.clone(),
            criteria_true: gate.criteria.is_true.clone(),
            criteria_false: gate.criteria.is_false.clone(),
            threshold: gate.threshold.unwrap_or_else(|| floor.of(gate.cost)),
        })
        .collect()
}

fn launch_from(
    recipe: &Recipe,
    config: &PickerConfig,
    resolver: ResolverMode,
    role: &str,
) -> Launch {
    Launch {
        kind: recipe.kind.clone(),
        args: recipe.args.clone(),
        env: recipe.env.clone(),
        ready_timeout_ms: recipe.ready_timeout_ms,
        policy_hash: config.policy_hash.clone(),
        attempt: 1,
        brief_hash: String::new(),
        recipe_id: String::new(),
        resolver,
        reason: String::new(),
        compact_reason: compact_reason(role, &recipe.plain),
        jev_prompt_hash: String::new(),
        excerpt_version: jev::EXCERPT_VERSION,
        ..Launch::default()
    }
}

fn default_launch(
    recipe: &Recipe,
    id: &str,
    config: &PickerConfig,
    role: &str,
    fallback: &str,
) -> Launch {
    let mut launch = launch_from(recipe, config, config.resolver, role);
    launch.recipe_id = id.to_string();
    launch.reason = fallback_reason(role, &recipe.plain);
    launch.fallback = Some(fallback.to_string());
    launch
}

fn fallback_launch(
    recipe: &Recipe,
    id: &str,
    config: &PickerConfig,
    role: &str,
    fallback: &str,
) -> Launch {
    let mut launch = launch_from(recipe, config, config.resolver, role);
    launch.recipe_id = id.to_string();
    launch.reason = fallback_reason(role, &recipe.plain);
    launch.fallback = Some(fallback.to_string());
    launch
}

/// The per-project, per-UTC-day call counter. Returns false when the cap is
/// already spent (SPEC-jev-picker v2 §3 step 5).
fn project_take_daily_call(project: &Project, cap: u64) -> Result<bool> {
    let _lock = project.lock()?;
    let now = jiff::Timestamp::now().as_second();
    let day = now / 86_400;
    let dir = project.dir().join(".state/jev-calls");
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let path = dir.join(format!("{day}.count"));
    let count: u64 = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    if count >= cap {
        return Ok(false);
    }
    crate::project::write_atomic(&path, (count + 1).to_string().as_bytes())?;
    Ok(true)
}

/// One `doctor` row: `ok` is `Some(true)`/`Some(false)`/`None` (warn), the
/// same three marks `doctor` prints (SPEC-ADE D1, SPEC-jev-picker v2 §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorRow {
    pub ok: Option<bool>,
    pub label: String,
    pub detail: String,
}

/// The picker rows for A1's `doctor` (SPEC-jev-picker v2 §5). The
/// `GET /v1/models` row is the only live call and runs only when the resolver
/// is not `off`.
pub fn doctor_rows(ctx: &Ctx) -> Result<Vec<DoctorRow>> {
    let config = parse_picker_config(&ctx.config_dir, false)?;
    let mut rows = Vec::new();
    rows.push(DoctorRow {
        ok: Some(true),
        label: "picker".into(),
        detail: format!(
            "resolver is {}; {} recipe(s), {} role(s)",
            match config.resolver {
                ResolverMode::Off => "off",
                ResolverMode::Shadow => "shadow",
                ResolverMode::Jev => "jev",
                ResolverMode::Pin => "pin",
            },
            config.recipes.len(),
            config.roles.len()
        ),
    });
    for name in &config.inline_without_plain {
        rows.push(DoctorRow {
            ok: None,
            label: format!("role {name}"),
            detail: format!(
                "[roles.{name}] has no plain phrase; the board says \"the usual helper\" until you add one"
            ),
        });
    }
    if config.resolver == ResolverMode::Off && config.recipes.is_empty() {
        return Ok(rows);
    }

    // The key, its mode, curl and the live probe matter only when the picker
    // calls out: `doctor` checks them when the resolver is not off
    // (SPEC-jev-picker v2 §5).
    if config.resolver != ResolverMode::Off {
        key_rows(ctx, &mut rows);
    }
    table_rows(ctx, &config, &mut rows);
    Ok(rows)
}

fn key_rows(ctx: &Ctx, rows: &mut Vec<DoctorRow>) {
    let key = jev::key_report(ctx.env);
    match key.source {
        jev::KeySource::Env => rows.push(DoctorRow {
            ok: Some(true),
            label: "picker key".into(),
            detail: "from TYPESAFE_API_KEY".into(),
        }),
        jev::KeySource::File => rows.push(DoctorRow {
            ok: Some(true),
            label: "picker key".into(),
            detail: format!("at {}", key.path.display()),
        }),
        jev::KeySource::Missing => rows.push(DoctorRow {
            ok: None,
            label: "picker key".into(),
            detail: format!(
                "no key: set TYPESAFE_API_KEY or write {}",
                key.path.display()
            ),
        }),
    }
    if let Some(mode) = key.mode {
        if key.readable_by_others {
            rows.push(DoctorRow {
                ok: None,
                label: "picker key mode".into(),
                detail: format!("{mode:o} is readable by group or other; use 600"),
            });
        } else {
            rows.push(DoctorRow {
                ok: Some(true),
                label: "picker key mode".into(),
                detail: format!("{mode:o}"),
            });
        }
    }

    match ctx
        .runner
        .run(&Cmd::new(jev::CURL, HELP_TIMEOUT).arg("--version"))
    {
        Ok(output) if output.success() => rows.push(DoctorRow {
            ok: Some(true),
            label: "curl".into(),
            detail: output
                .stdout
                .lines()
                .next()
                .unwrap_or(jev::CURL)
                .to_string(),
        }),
        Ok(output) => rows.push(DoctorRow {
            ok: None,
            label: "curl".into(),
            detail: format!("{}: {}", jev::CURL, output.error_text()),
        }),
        Err(error) => rows.push(DoctorRow {
            ok: None,
            label: "curl".into(),
            detail: format!("{error:#}"),
        }),
    }

    if let Some(key) = jev::load_key(ctx.env) {
        let (status, body) = jev::models_probe(ctx.runner, &key, Duration::from_secs(3));
        let detail = match (status, body.is_empty()) {
            (200, _) => "GET /v1/models answered 200".to_string(),
            (0, _) => format!("GET /v1/models did not answer: {body}"),
            (_, true) => format!("GET /v1/models answered {status}"),
            (_, false) => format!("GET /v1/models answered {status}: {body}"),
        };
        rows.push(DoctorRow {
            ok: if status == 200 { Some(true) } else { None },
            label: "picker models".into(),
            detail,
        });
    }
}

fn table_rows(ctx: &Ctx, config: &PickerConfig, rows: &mut Vec<DoctorRow>) {
    match agent_kinds(ctx.env, ctx.runner) {
        Ok(kinds) => match validate_config(config, &kinds) {
            Ok(()) => rows.push(DoctorRow {
                ok: Some(true),
                label: "picker recipes".into(),
                detail: format!("{} recipe(s) valid", config.recipes.len()),
            }),
            Err(error) => rows.push(DoctorRow {
                ok: Some(false),
                label: "picker recipes".into(),
                detail: format!("{error:#}"),
            }),
        },
        Err(error) => rows.push(DoctorRow {
            ok: None,
            label: "picker recipes".into(),
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
            || role
                .gates
                .iter()
                .any(|gate| gate.recipe.starts_with("codex_"))
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

/// Print the rows with `doctor`'s three marks and fail when one is a FAIL.
/// The hidden `picker doctor` verb uses this until A1's `doctor` calls
/// `doctor_rows`.
pub fn picker_doctor(ctx: &Ctx) -> Result<()> {
    let mut healthy = true;
    for row in doctor_rows(ctx)? {
        let mark = match row.ok {
            Some(true) => "ok  ",
            Some(false) => {
                healthy = false;
                "FAIL"
            }
            None => "warn",
        };
        println!("[{mark}] {}: {}", row.label, row.detail);
    }
    if !healthy {
        bail!("the picker has failing checks");
    }
    Ok(())
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
    // Tests use the FakeRunner with scripted curl replies, never the service.
    use crate::runner::fake::ok;
    use crate::scenarios::World;

    const KINDS_HELP: &str = "Options:\n      --kind <KIND>\n          [possible values: pi, claude, codex, gemini, cursor, devin, agy, opencode, kimi, muse]\n";

    fn config_text(resolver: &str) -> String {
        format!(
            r#"
[roles]
resolver = "{resolver}"
jev_model = "jev-1.13.0"
jev_timeout_ms = 3000
jev_daily_cap = 200
floor = {{ sideways = 0.50, upgrade = 0.70 }}

[recipes.cursor_grok_xhigh]
kind = "cursor"
provider = "cursor"
args = ["--model", "cursor-grok-4.6-xhigh", "--force"]
env = []
ready_timeout_ms = 30000
cost = "default"
plain = "the usual coding helper"

[recipes.agy_gemini_flash]
kind = "agy"
provider = "agy"
args = ["--model", "gemini-3.8-flash-high", "--dangerously-skip-permissions"]
env = []
ready_timeout_ms = 60000
cost = "sideways"
plain = "the web research helper"

[recipes.claude_opus_high]
kind = "claude"
provider = "claude"
args = ["--model", "claude-opus-5", "--effort", "high", "--dangerously-skip-permissions"]
env = []
ready_timeout_ms = 90000
cost = "upgrade"
plain = "the strongest design helper"

[roles.lane]
default = "cursor_grok_xhigh"
allowed = ["cursor_grok_xhigh", "agy_gemini_flash"]

[[roles.lane.gates]]
recipe = "agy_gemini_flash"
cost = "sideways"
threshold = 0.75
instructions = "Is the main job of `task` to read public web pages, vendor pages or datasheets and cite them, rather than to write or change the product?"
criteria = {{ true = "Web research with citations.", false = "Implementation, review, or spec writing." }}
"#
        )
    }

    fn world(resolver: &str, task: &str) -> (World, Project, String) {
        let world = World::new();
        std::fs::create_dir_all(world.home.path().join("cfg")).unwrap();
        std::fs::write(
            world.home.path().join("cfg/config.toml"),
            config_text(resolver),
        )
        .unwrap();
        let key = world.home.path().join(".config/typesafe/api_key");
        std::fs::create_dir_all(key.parent().unwrap()).unwrap();
        std::fs::write(&key, "test-key\n").unwrap();
        let project = world.project("demo", "s.sock");
        (world, project, task.to_string())
    }

    fn with_help(world: &World) {
        world.runner.on("agent start --help", ok(KINDS_HELP));
    }

    fn single_response(world: &World, stdout: &str) {
        let stdout = stdout.to_string();
        world.runner.on_fn(
            |cmd| cmd.display().contains("/usr/bin/curl"),
            move |_| Ok(ok(&stdout)),
        );
    }

    fn answers_body(noul: f64) -> String {
        format!(
            "{{\"model\":\"jev-1.13.0\",\"answers\":{{\"agy_gemini_flash\":{{\"type\":\"noul\",\"noul\":{noul}}}}},\"usage\":{{\"input_tokens\":356}}}}\n200"
        )
    }

    fn run(world: &World, project: &Project, task: &str) -> Result<Launch> {
        let ctx = world.ctx();
        resolve_launch(
            &ctx,
            project,
            &ResolveInput {
                role: "lane",
                task,
                opted_in: true,
                ..ResolveInput::default()
            },
        )
    }

    #[test]
    fn off_launches_the_default_without_calling_jev() {
        let (world, project, task) = world("off", "write the parser");
        with_help(&world);
        let launch = run(&world, &project, &task).unwrap();
        assert_eq!(launch.recipe_id, "cursor_grok_xhigh");
        assert_eq!(launch.resolver, ResolverMode::Off);
        assert_eq!(launch.fallback.as_deref(), Some(FALLBACK_RESOLVER_OFF));
        assert_eq!(world.runner.count("/usr/bin/curl"), 0);
        assert_eq!(
            launch.reason,
            "this task runs on the usual coding helper, the usual choice, because the picker did not answer."
        );
        assert_eq!(
            launch.compact_reason,
            "this task runs on the usual coding helper"
        );
    }

    #[test]
    fn the_jev_mode_is_refused_in_this_round() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), config_text("jev")).unwrap();
        let error = parse_picker_config(home.path(), true).unwrap_err();
        assert!(
            format!("{error:#}").contains("resolver_mode_unavailable"),
            "{error:#}"
        );
        // An absent file is the shipped default: off.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            parse_picker_config(empty.path(), true).unwrap().resolver,
            ResolverMode::Off
        );
    }

    /// The `jev` acceptance path, kept for the round that turns it on: a gate
    /// over its threshold switches and records the pick.
    #[test]
    fn a_gate_over_its_threshold_switches_in_jev_mode() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), config_text("shadow")).unwrap();
        let mut config = parse_picker_config(home.path(), true).unwrap();
        config.resolver = ResolverMode::Jev;
        let role = config.roles["lane"].clone();
        let questions = gate_questions(&role.gates, &config.floor);
        let mut nouls = BTreeMap::new();
        nouls.insert("agy_gemini_flash".to_string(), 0.91);
        let answers = jev::Answers {
            model: jev::JEV_MODEL.into(),
            nouls,
            input_tokens: Some(356),
        };
        let default = config.recipes["cursor_grok_xhigh"].clone();
        let input = ResolveInput {
            role: "lane",
            ..ResolveInput::default()
        };
        let launch = accepted_launch(
            &config,
            &input,
            &questions,
            &answers,
            &default,
            "cursor_grok_xhigh",
        );
        assert_eq!(launch.recipe_id, "agy_gemini_flash");
        assert_eq!(launch.kind, "agy");
        assert_eq!(launch.resolver, ResolverMode::Jev);
        assert_eq!(launch.gate.as_deref(), Some("agy_gemini_flash"));
        assert_eq!(launch.jev_pick.as_deref(), Some("agy_gemini_flash"));
        assert_eq!(launch.gate_p, Some(0.91), "the Noul, not the threshold");
        assert_eq!(launch.fallback, None);
        assert_eq!(
            launch.reason,
            "this task looks like web research, so it runs on the web research helper."
        );
        assert_eq!(
            launch.compact_reason,
            "this task runs on the web research helper"
        );
    }

    #[test]
    fn a_gate_over_its_threshold_in_shadow_records_the_pick_only() {
        let (world, project, task) = world("shadow", "read the vendor pages and cite them");
        with_help(&world);
        single_response(&world, &answers_body(0.91));
        let launch = run(&world, &project, &task).unwrap();
        assert_eq!(launch.recipe_id, "cursor_grok_xhigh");
        assert_eq!(launch.kind, "cursor");
        assert_eq!(launch.jev_pick.as_deref(), Some("agy_gemini_flash"));
        assert_eq!(launch.gate.as_deref(), Some("agy_gemini_flash"));
        assert_eq!(launch.gate_p, Some(0.91), "the Noul, not the threshold");
        assert_eq!(launch.jev_input_tokens, Some(356));
        assert_eq!(launch.jev_model.as_deref(), Some(jev::JEV_MODEL));
        assert_eq!(world.runner.count("/usr/bin/curl"), 1);
    }

    #[test]
    fn a_low_noul_keeps_the_default_with_no_gate() {
        let (world, project, task) = world("shadow", "write the parser");
        with_help(&world);
        single_response(&world, &answers_body(0.40));
        let launch = run(&world, &project, &task).unwrap();
        assert_eq!(launch.recipe_id, "cursor_grok_xhigh");
        assert_eq!(launch.resolver, ResolverMode::Shadow);
        assert_eq!(launch.fallback.as_deref(), Some(FALLBACK_SHADOW));
        assert_eq!(launch.jev_pick, None);
        assert_eq!(
            launch.reason,
            "this task looks like ordinary work, so it runs on the usual coding helper."
        );
    }

    #[test]
    fn another_model_is_a_fallback() {
        let (world, project, task) = world("shadow", "read the vendor pages");
        with_help(&world);
        let body = "{\"model\":\"jev-1.12.0\",\"answers\":{\"agy_gemini_flash\":{\"type\":\"noul\",\"noul\":0.99}}}\n200";
        single_response(&world, body);
        let launch = run(&world, &project, &task).unwrap();
        assert_eq!(launch.recipe_id, "cursor_grok_xhigh");
        assert_eq!(launch.fallback.as_deref(), Some(FALLBACK_MODEL_MISMATCH));
        assert_eq!(launch.jev_model.as_deref(), Some("jev-1.12.0"));
    }

    #[test]
    fn a_429_is_retried_once_then_picked() {
        let (world, project, task) = world("shadow", "read the vendor pages");
        with_help(&world);
        let responses =
            std::cell::RefCell::new(vec!["{}\n429".to_string(), answers_body(0.91)].into_iter());
        world.runner.on_fn(
            |cmd| cmd.display().contains("/usr/bin/curl"),
            move |_| {
                let stdout = responses
                    .borrow_mut()
                    .next()
                    .unwrap_or_else(|| "{}\n500".to_string());
                Ok(ok(&stdout))
            },
        );
        let launch = run(&world, &project, &task).unwrap();
        assert_eq!(launch.recipe_id, "cursor_grok_xhigh");
        assert_eq!(launch.jev_pick.as_deref(), Some("agy_gemini_flash"));
        assert_eq!(world.runner.count("/usr/bin/curl"), 2);
    }

    #[test]
    fn a_timeout_a_401_and_a_malformed_body_fall_back() {
        for (name, output, fallback) in [
            (
                "timeout",
                crate::runner::Output {
                    timed_out: true,
                    ..crate::runner::Output::default()
                },
                FALLBACK_TIMEOUT,
            ),
            (
                "401",
                crate::runner::Output {
                    code: Some(0),
                    stdout: "{}\n401".into(),
                    ..crate::runner::Output::default()
                },
                "http_401",
            ),
            (
                "malformed",
                crate::runner::Output {
                    code: Some(0),
                    stdout: "not json\n200".into(),
                    ..crate::runner::Output::default()
                },
                FALLBACK_MALFORMED,
            ),
        ] {
            let (world, project, task) = world("shadow", "read the vendor pages");
            with_help(&world);
            world.runner.on_fn(
                |cmd| cmd.display().contains("/usr/bin/curl"),
                move |_| Ok(output.clone()),
            );
            let launch = run(&world, &project, &task).unwrap();
            assert_eq!(launch.recipe_id, "cursor_grok_xhigh", "{name}");
            assert_eq!(launch.fallback.as_deref(), Some(fallback), "{name}");
        }
    }

    #[test]
    fn a_config_change_during_the_call_launches_the_new_default() {
        let (world, project, task) = world("shadow", "read the vendor pages");
        with_help(&world);
        let config = world.home.path().join("cfg/config.toml");
        let changed = config_text("shadow")
            .replace(
                "default = \"cursor_grok_xhigh\"\nallowed = [\"cursor_grok_xhigh\", \"agy_gemini_flash\"]",
                "default = \"cursor_grok_xhigh\"\nallowed = [\"cursor_grok_xhigh\", \"agy_gemini_flash\", \"claude_opus_high\"]",
            )
            .replace(
                "default = \"cursor_grok_xhigh\"",
                "default = \"claude_opus_high\"",
            );
        assert_ne!(changed, config_text("shadow"));
        let body = answers_body(0.91);
        world.runner.on_fn(
            |cmd| cmd.display().contains("/usr/bin/curl"),
            move |_| {
                // Rolf edits the table while the call is in flight.
                std::fs::write(&config, &changed).unwrap();
                Ok(ok(&body))
            },
        );
        let launch = run(&world, &project, &task).unwrap();
        assert_eq!(launch.fallback.as_deref(), Some(FALLBACK_CONFIG_CHANGED));
        assert_eq!(launch.recipe_id, "claude_opus_high");
        assert_eq!(launch.kind, "claude");
        assert_eq!(launch.jev_pick, None);
    }

    #[test]
    fn a_task_that_names_a_model_is_not_a_pin() {
        let (world, project, task) = world(
            "shadow",
            "Use the Opus helper, claude-opus-5 with high effort, for this parser.",
        );
        with_help(&world);
        single_response(&world, &answers_body(0.10));
        let launch = run(&world, &project, &task).unwrap();
        assert_eq!(launch.recipe_id, "cursor_grok_xhigh");
        assert_eq!(launch.kind, "cursor");
        assert_eq!(launch.fallback.as_deref(), Some(FALLBACK_SHADOW));
        // The name never reached the service.
        let sent: String = world
            .runner
            .calls
            .borrow()
            .iter()
            .filter(|cmd| cmd.program == jev::CURL)
            .filter_map(|cmd| cmd.stdin.clone())
            .collect();
        assert!(sent.contains("data-binary"), "{sent}");
        assert!(!sent.to_ascii_lowercase().contains("opus"), "{sent}");
    }

    #[test]
    fn every_call_counts_against_the_daily_cap_once() {
        let (world, project, task) = world("shadow", "read the vendor pages");
        with_help(&world);
        // A 429 then a 429: two attempts, one call.
        single_response(&world, "{}\n429");
        let launch = run(&world, &project, &task).unwrap();
        assert_eq!(launch.fallback.as_deref(), Some("http_429"));
        assert_eq!(world.runner.count("/usr/bin/curl"), 2);
        let day = jiff::Timestamp::now().as_second() / 86_400;
        let path = project.dir().join(format!(".state/jev-calls/{day}.count"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "1");
        run(&world, &project, &task).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "2");
    }

    #[test]
    fn the_daily_cap_stops_the_call() {
        let (world, project, task) = world("shadow", "read the vendor pages");
        with_help(&world);
        single_response(&world, &answers_body(0.91));
        let dir = project.dir().join(".state/jev-calls");
        std::fs::create_dir_all(&dir).unwrap();
        let day = jiff::Timestamp::now().as_second() / 86_400;
        std::fs::write(dir.join(format!("{day}.count")), "200").unwrap();
        let launch = run(&world, &project, &task).unwrap();
        assert_eq!(launch.fallback.as_deref(), Some(FALLBACK_DAILY_CAP));
        assert_eq!(world.runner.count("/usr/bin/curl"), 0);
    }

    #[test]
    fn shadow_launches_the_default_and_records_the_pick() {
        let (world, project, task) = world("shadow", "read the vendor pages");
        with_help(&world);
        single_response(&world, &answers_body(0.91));
        let launch = run(&world, &project, &task).unwrap();
        assert_eq!(launch.recipe_id, "cursor_grok_xhigh");
        assert_eq!(launch.resolver, ResolverMode::Shadow);
        assert_eq!(launch.jev_pick.as_deref(), Some("agy_gemini_flash"));
        assert_eq!(launch.fallback.as_deref(), Some(FALLBACK_SHADOW));
        assert_eq!(
            launch.reason,
            "this task runs on the usual coding helper; the picker would have chosen the web research helper."
        );
        assert_eq!(
            launch.compact_reason,
            "this task runs on the usual coding helper"
        );
    }

    #[test]
    fn a_recipe_pin_skips_jev_and_the_agent_pin_is_project() {
        let (world, project, task) = world("shadow", "read the vendor pages");
        with_help(&world);
        single_response(&world, &answers_body(0.91));
        let ctx = world.ctx();
        let launch = resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                role: "lane",
                task: &task,
                recipe: Some("agy_gemini_flash"),
                opted_in: true,
                ..ResolveInput::default()
            },
        )
        .unwrap();
        assert_eq!(launch.recipe_id, "agy_gemini_flash");
        assert_eq!(launch.resolver, ResolverMode::Pin);
        assert_eq!(launch.fallback.as_deref(), Some(FALLBACK_OVERRIDE));
        assert_eq!(
            launch.reason,
            "You chose the web research helper for this task."
        );
        assert_eq!(world.runner.count("/usr/bin/curl"), 0);

        let pin = Recipe {
            kind: "claude".into(),
            args: vec![
                "--model".into(),
                "claude-opus-5".into(),
                "--effort".into(),
                "high".into(),
                "--dangerously-skip-permissions".into(),
            ],
            plain: "the strongest design helper".into(),
            ..Recipe::default()
        };
        let launch = resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                role: "lane",
                task: &task,
                project_pin: Some(pin),
                opted_in: true,
                ..ResolveInput::default()
            },
        )
        .unwrap();
        assert_eq!(launch.resolver, ResolverMode::Pin);
        assert_eq!(launch.fallback.as_deref(), Some(FALLBACK_PROJECT));
        assert_eq!(launch.kind, "claude");
    }

    #[test]
    fn a_project_pin_keeps_the_default_plain_and_refuses_an_unknown_kind() {
        let (world, project, task) = world("shadow", "x");
        with_help(&world);
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
            &project,
            &ResolveInput {
                role: "lane",
                task: &task,
                project_pin: Some(pin),
                opted_in: true,
                ..ResolveInput::default()
            },
        )
        .unwrap();
        assert_eq!(
            launch.reason,
            "You chose the usual coding helper for this task."
        );
        assert_eq!(
            launch.compact_reason,
            "this task runs on the usual coding helper"
        );

        let pin = Recipe {
            kind: "nope".into(),
            args: vec!["--x".into()],
            ..Recipe::default()
        };
        let error = resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                role: "lane",
                task: &task,
                project_pin: Some(pin),
                opted_in: true,
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
        let (world, project, task) = world("shadow", "x");
        with_help(&world);
        let ctx = world.ctx();
        let resolve = |pin: Recipe| {
            resolve_launch(
                &ctx,
                &project,
                &ResolveInput {
                    role: "lane",
                    task: &task,
                    project_pin: Some(pin),
                    opted_in: true,
                    ..ResolveInput::default()
                },
            )
        };
        // Same kind, no args: the default's args, permission flag included.
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
        // A kind change without args is role_args_missing.
        let error = resolve(Recipe {
            kind: "claude".into(),
            ..Recipe::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("role_args_missing"), "{error:#}");
        // A pin without its permission flag, or Opus without high effort.
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
        let error = resolve(Recipe {
            kind: "claude".into(),
            args: vec![
                "--model".into(),
                "claude-opus-5".into(),
                "--effort".into(),
                "max".into(),
                "--dangerously-skip-permissions".into(),
            ],
            ..Recipe::default()
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("recipe_effort_forbidden"),
            "{error:#}"
        );
    }

    #[test]
    fn a_recipe_outside_the_allowed_list_is_refused() {
        let (world, project, task) = world("shadow", "x");
        with_help(&world);
        let ctx = world.ctx();
        let error = resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                role: "lane",
                task: &task,
                recipe: Some("claude_opus_high"),
                opted_in: true,
                ..ResolveInput::default()
            },
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("recipe_not_allowed"),
            "{error:#}"
        );
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
    }

    #[test]
    fn validation_refuses_bad_tables() {
        let kinds = jev::parse_kinds(KINDS_HELP).unwrap();
        let home = tempfile::tempdir().unwrap();
        let config_dir = home.path();
        std::fs::write(config_dir.join("config.toml"), config_text("shadow")).unwrap();
        let mut config = parse_picker_config(config_dir, true).unwrap();

        let mut bad = config.clone();
        bad.recipes.get_mut("cursor_grok_xhigh").unwrap().kind = "nope".into();
        let error = validate_config(&bad, &kinds).unwrap_err();
        assert!(
            error.to_string().contains("recipe_kind_unknown"),
            "{error:#}"
        );

        let mut bad = config.clone();
        bad.recipes.get_mut("claude_opus_high").unwrap().args = vec![
            "--model".into(),
            "claude-opus-5".into(),
            "--dangerously-skip-permissions".into(),
        ];
        let error = validate_config(&bad, &kinds).unwrap_err();
        assert!(
            error.to_string().contains("recipe_effort_forbidden"),
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
        bad.roles.get_mut("lane").unwrap().default = "agy_gemini_flash".into();
        bad.roles.get_mut("lane").unwrap().allowed =
            vec!["cursor_grok_xhigh".into(), "agy_gemini_flash".into()];
        // The gate targets the default now.
        let error = validate_config(&bad, &kinds).unwrap_err();
        assert!(error.to_string().contains("gate_is_default"), "{error:#}");

        config.recipes.get_mut("cursor_grok_xhigh").unwrap().plain = "the biorhythm helper".into();
        let error = validate_config(&config, &kinds).unwrap_err();
        assert!(
            error.to_string().contains("recipe_reason_not_plain"),
            "{error:#}"
        );
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
            picked_reason("lane", "web research", "the web research helper"),
            default_reason("lane", "the usual coding helper"),
            pinned_reason("reviewer", "the strongest design helper"),
            fallback_reason("critic", "the second opinion helper"),
            shadow_reason(
                "drafter",
                "the usual coding helper",
                "the second opinion helper",
            ),
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
    fn the_policy_hash_covers_recipes_lists_gates_and_thresholds() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path();
        std::fs::write(dir.join("config.toml"), config_text("shadow")).unwrap();
        let base = parse_picker_config(dir, true).unwrap();
        let mut changed = config_text("shadow");
        changed = changed.replace("threshold = 0.75", "threshold = 0.80");
        std::fs::write(dir.join("config.toml"), changed).unwrap();
        let moved = parse_picker_config(dir, true).unwrap();
        assert_ne!(base.policy_hash, moved.policy_hash);
        let same = parse_picker_config(dir, true).unwrap();
        assert_eq!(moved.policy_hash, same.policy_hash);
    }

    #[test]
    fn doctor_warns_on_an_inline_role_without_plain_and_skips_the_key_when_off() {
        let world = World::new();
        std::fs::create_dir_all(world.home.path().join("cfg")).unwrap();
        std::fs::write(
            world.home.path().join("cfg/config.toml"),
            "[roles.lane]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\n\n[roles.pro]\nkind = \"chatgpt\"\n",
        )
        .unwrap();
        with_help(&world);
        world.runner.on_fn(
            |cmd| cmd.display().contains("zsh -lic"),
            |_| Ok(ok("/usr/local/bin/claude\n")),
        );
        let ctx = world.ctx();
        // The config loads and resolves: a missing phrase is not a refusal.
        let config = parse_picker_config(&ctx.config_dir, false).unwrap();
        assert_eq!(config.inline_without_plain, ["lane"]);
        let rows = doctor_rows(&ctx).unwrap();
        let lane = rows.iter().find(|row| row.label == "role lane").unwrap();
        assert_eq!(lane.ok, None, "{lane:?}");
        assert!(!rows.iter().any(|row| row.label == "role pro"), "{rows:?}");
        // Resolver off: no key, curl or probe rows, and no curl run.
        assert!(
            !rows.iter().any(|row| row.label.starts_with("picker key")),
            "{rows:?}"
        );
        assert_eq!(world.runner.count("/usr/bin/curl"), 0);
        assert!(
            rows.iter()
                .any(|row| row.label == "picker recipes" && row.ok == Some(true)),
            "{rows:?}"
        );
    }

    #[test]
    fn pro_and_the_coordinator_never_resolve() {
        let (world, project, task) = world("shadow", "x");
        with_help(&world);
        let ctx = world.ctx();
        for role in ["pro", "coordinator"] {
            let error = resolve_launch(
                &ctx,
                &project,
                &ResolveInput {
                    role,
                    task: &task,
                    opted_in: true,
                    ..ResolveInput::default()
                },
            )
            .unwrap_err();
            assert!(error.to_string().contains("role_not_resolved"), "{error:#}");
        }
        assert_eq!(world.runner.count("/usr/bin/curl"), 0);
    }

    #[test]
    fn doctor_rows_report_off_and_the_shadow_probe() {
        let world = World::new();
        std::fs::create_dir_all(world.home.path().join("cfg")).unwrap();
        let ctx = world.ctx();
        let rows = doctor_rows(&ctx).unwrap();
        assert_eq!(rows[0].label, "picker");
        assert!(rows[0].detail.contains("off"));

        std::fs::write(
            world.home.path().join("cfg/config.toml"),
            config_text("shadow"),
        )
        .unwrap();
        let key_path = world.home.path().join(".config/typesafe/api_key");
        std::fs::create_dir_all(key_path.parent().unwrap()).unwrap();
        std::fs::write(&key_path, "test-key\n").unwrap();
        with_help(&world);
        world.runner.on_fn(
            |cmd| cmd.display().contains("/usr/bin/curl"),
            |cmd| {
                if cmd.display().contains("--version") {
                    Ok(ok("curl 8.7.1\n"))
                } else {
                    Ok(ok("{\"models\":[]}\n200"))
                }
            },
        );
        world.runner.on_fn(
            |cmd| cmd.display().contains("zsh -lic"),
            |_| Ok(ok("/usr/local/bin/cursor-agent\n")),
        );
        let rows = doctor_rows(&ctx).unwrap();
        let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
        assert!(labels.contains(&"picker models"), "{labels:?}");
        assert!(labels.contains(&"picker recipes"), "{labels:?}");
        assert!(
            labels.iter().any(|label| label.starts_with("kind ")),
            "{labels:?}"
        );
    }
}
