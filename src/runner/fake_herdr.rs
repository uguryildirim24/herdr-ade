//! Argument contract for the fork at 85a7743a (test-only; no server involved).
//! Sources: cli/{agent,pane,tab,workspace,plugin,notification}.rs, cli/spec.rs,
//! app/{agents,api_helpers,api/env,api/panes}.rs, config/keybinds.rs, session.rs.
//! Unmodelled commands fail closed instead of letting a scripted reply bless them.

use std::collections::{BTreeMap, BTreeSet};

use super::Cmd;

type Check = Result<(), String>;

struct Syntax {
    positional: std::ops::RangeInclusive<usize>,
    flags: &'static str,
    values: &'static str,
    required: &'static str,
}

fn syntax(
    min: usize,
    max: usize,
    flags: &'static str,
    values: &'static str,
    required: &'static str,
) -> Syntax {
    Syntax {
        positional: min..=max,
        flags,
        values,
        required,
    }
}

struct Args<'a> {
    positional: Vec<&'a str>,
    values: BTreeMap<&'a str, Vec<&'a str>>,
    ordered: Vec<(&'a str, &'a str)>,
    flags: BTreeSet<&'a str>,
    trailing: &'a [String],
}

impl<'a> Args<'a> {
    fn value(&self, key: &str) -> Option<&'a str> {
        self.values
            .get(key)
            .and_then(|values| values.last().copied())
    }
    fn all(&self, key: &str) -> impl Iterator<Item = &'a str> + '_ {
        self.values.get(key).into_iter().flatten().copied()
    }
    fn has(&self, key: &str) -> bool {
        self.values.contains_key(key) || self.flags.contains(key)
    }
}

fn parse<'a>(
    args: &'a [String],
    rule: Syntax,
    start: bool,
    literal: bool,
) -> Result<Args<'a>, String> {
    let mut parsed = Args {
        positional: vec![],
        values: BTreeMap::new(),
        ordered: vec![],
        flags: BTreeSet::new(),
        trailing: &[],
    };
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        // The fork consumes prompt/text/command/key positionals literally,
        // including leading dashes and embedded newlines, before parsing options.
        if (parsed.positional.len() < *rule.positional.start()
            && (literal || !arg.starts_with('-')))
            || (literal
                && *rule.positional.end() == usize::MAX
                && rule.flags.is_empty()
                && rule.values.is_empty())
        {
            parsed.positional.push(arg);
        } else if arg == "--" && start {
            parsed.trailing = &args[i + 1..];
            break;
        } else if let Some(key) = arg.strip_prefix("--") {
            if rule.flags.split_whitespace().any(|flag| flag == key) {
                parsed.flags.insert(key);
            } else if rule.values.split_whitespace().any(|flag| flag == key) {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| format!("missing value for --{key}"))?;
                parsed.values.entry(key).or_default().push(value);
                parsed.ordered.push((key, value));
            } else {
                return Err(format!("unknown option --{key}"));
            }
        } else {
            if !literal && arg.starts_with('-') {
                return Err(format!("unknown option {arg}"));
            }
            parsed.positional.push(arg);
        }
        i += 1;
    }
    if !rule.positional.contains(&parsed.positional.len()) {
        return Err("missing or extra positional argument".into());
    }
    for required in rule.required.split_whitespace() {
        if !parsed.has(required) {
            return Err(format!("missing required --{required}"));
        }
    }
    Ok(parsed)
}

fn one_of(value: &str, choices: &str) -> Check {
    if choices.split_whitespace().any(|choice| choice == value) {
        Ok(())
    } else {
        Err(format!("invalid value {value:?}; expected {choices}"))
    }
}

fn u64_value(value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("expected u64, got {value:?}"))
}

fn agent_name(value: &str) -> Check {
    if matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value.len() <= 32
        && value
            .bytes()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err("invalid_agent_name: expected [a-z][a-z0-9_-]{0,31}".into())
    }
}

fn identifier(value: &str, plugin: bool) -> Check {
    let value = value.trim();
    let punctuation = if plugin {
        b":._-".as_slice()
    } else {
        b":_-".as_slice()
    };
    if !value.is_empty()
        && value.len() <= 120
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || punctuation.contains(&ch))
    {
        Ok(())
    } else {
        Err("invalid plugin/entrypoint identifier".into())
    }
}

fn source(value: &str) -> Check {
    let value = value.trim();
    if !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b":._-".contains(&ch))
    {
        Ok(())
    } else {
        Err("invalid_metadata_source".into())
    }
}

fn pane_id(value: &str) -> bool {
    let value = value.trim();
    if let Some((workspace, pane)) = value.rsplit_once(":p") {
        !workspace.is_empty()
            && !pane.is_empty()
            && pane
                .chars()
                .try_fold(0usize, |number, ch| {
                    let digit = b"123456789ABCDEFGHJKMNPQRSTVWXYZ0"
                        .iter()
                        .position(|candidate| *candidate as char == ch)?;
                    number.checked_mul(32)?.checked_add(digit + 1)
                })
                .is_some()
    } else if let Some((workspace, pane)) = value.rsplit_once('-') {
        !workspace.is_empty() && pane.parse::<usize>().is_ok()
    } else if let Some(rest) = value.strip_prefix("p_") {
        rest.rsplit_once('_').map_or_else(
            || rest.parse::<u32>().is_ok(),
            |(workspace, pane)| !workspace.is_empty() && pane.parse::<u32>().is_ok(),
        )
    } else {
        false
    }
}

fn key(value: &str) -> Check {
    let value = match value.trim() {
        "C-c" | "c-c" => "ctrl+c",
        "+" => "plus",
        other => other,
    };
    let mut found = false;
    for part in value.split('+') {
        let part = part.trim();
        let lower = part.to_lowercase();
        if [
            "ctrl", "control", "alt", "option", "shift", "super", "cmd", "command", "meta", "hyper",
        ]
        .contains(&lower.as_str())
        {
            continue;
        }
        if found || part.is_empty() {
            return Err("invalid_key".into());
        }
        found = true;
        if part.chars().count() == 1 {
            continue;
        }
        if [
            "space",
            "enter",
            "return",
            "esc",
            "escape",
            "tab",
            "backspace",
            "bs",
            "left",
            "right",
            "up",
            "down",
            "minus",
            "comma",
            "period",
            "slash",
            "backslash",
            "quote",
            "double_quote",
            "double-quote",
            "semicolon",
            "colon",
            "percent",
            "ampersand",
            "backtick",
            "plus",
        ]
        .contains(&lower.as_str())
        {
            continue;
        }
        if lower
            .strip_prefix('f')
            .is_some_and(|n| n.parse::<u8>().is_ok())
        {
            continue;
        }
        return Err("invalid_key".into());
    }
    if found {
        Ok(())
    } else {
        Err("invalid_key".into())
    }
}

fn metadata(args: &Args<'_>, pane: bool) -> Check {
    for name in ["source", "applies-to-source"] {
        if let Some(value) = args.value(name) {
            source(value)?;
        }
    }
    if args
        .value("agent")
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err("invalid_agent".into());
    }
    if let Some(ttl) = args.value("ttl-ms")
        && !(1..=86_400_000).contains(&u64_value(ttl)?)
    {
        return Err("invalid_metadata_ttl".into());
    }
    let mut tokens = BTreeMap::new();
    for (flag, value) in &args.ordered {
        if *flag == "token" {
            let (name, value) = value.split_once('=').ok_or("token must use NAME=VALUE")?;
            tokens.insert(name, normalized_text(value));
        } else if *flag == "clear-token" {
            tokens.insert(*value, None);
        }
    }
    if tokens.len() > 16 {
        return Err("invalid_metadata_token: at most 16 keys per request".into());
    }
    for (name, value) in &tokens {
        if name.is_empty()
            || name.len() > 32
            || !name
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || b"_-".contains(&ch))
        {
            return Err("invalid_metadata_token: invalid key".into());
        }
        // Values are normalized/truncated by the fork, not rejected for length.
        if pane && *name == "parent" && value.is_some() {
            if args.has("ttl-ms") {
                return Err("invalid_metadata_token: parent cannot have a ttl".into());
            }
            if value.as_deref() == Some(args.positional[0]) {
                return Err("parent_cycle".into());
            }
        }
    }
    let mut presentation = false;
    for (set, clear) in [
        ("title", "clear-title"),
        ("display-agent", "clear-display-agent"),
        ("state-label", "clear-state-labels"),
    ] {
        if args.has(set) && args.has(clear) {
            return Err("invalid_metadata_request: cannot set and clear same field".into());
        }
        presentation |= args.has(clear)
            || (set != "state-label"
                && args
                    .value(set)
                    .is_some_and(|value| normalized_text(value).is_some()));
    }
    let mut labels = BTreeMap::new();
    for label in args.all("state-label") {
        let (status, text) = label.split_once('=').ok_or("expected STATUS=TEXT")?;
        let status = status.trim().to_ascii_lowercase();
        one_of(&status, "idle working blocked done unknown")?;
        labels.insert(status, normalized_text(text));
    }
    presentation |= labels.values().any(Option::is_some);
    if tokens.is_empty() && (!pane || !presentation) {
        return Err("invalid_metadata_request: missing metadata field".into());
    }
    if !pane && tokens.contains_key("parent") {
        return Err("invalid_metadata_token: parent is pane-only".into());
    }
    Ok(())
}

/// Check every herdr invocation before fake answers (including errors) match.
/// State-dependent checks remain scripted: resource existence, busy panes,
/// duplicate live names, ancestor cycles and per-resource token capacity.
pub(super) fn validate(cmd: &Cmd) -> Check {
    let is_herdr = cmd.env.iter().any(|(key, _)| key == "HERDR_SOCKET_PATH")
        || std::path::Path::new(&cmd.program)
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("herdr"));
    if !is_herdr {
        return Ok(());
    }
    if cmd.args.iter().any(|arg| arg.contains('\0')) {
        return Err("arguments cannot contain NUL".into());
    }
    let mut args = cmd.args.as_slice();
    while args
        .first()
        .is_some_and(|arg| matches!(arg.as_str(), "--machine" | "--session"))
    {
        let flag = &args[0];
        let value = args.get(1).ok_or("missing machine/session value")?;
        if flag == "--session" {
            session_name(value)?;
        }
        args = &args[2..];
    }
    if args == ["--version"] || args == ["--help"] {
        return Ok(());
    }
    if args
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        return Ok(());
    }
    let command = args.get(..2).ok_or("missing herdr command")?;
    let path = format!("{} {}", command[0], command[1]);
    args = &args[2..];
    let path = if matches!(
        path.as_str(),
        "plugin pane" | "plugin action" | "plugin log"
    ) {
        let sub = args.first().ok_or("missing plugin subcommand")?;
        let path = format!("{path} {sub}");
        args = &args[1..];
        path
    } else {
        path
    };
    let rule = match path.as_str() {
        "agent list" | "workspace list" | "api snapshot" | "status server" => {
            syntax(0, 0, "", "", "")
        }
        "agent start" => syntax(1, 1, "", "kind pane parent timeout env", "kind pane"),
        "agent wait" => syntax(1, 1, "", "until timeout", ""),
        "pane wait-output" => syntax(1, 1, "raw", "source lines match regex timeout", ""),
        "integration status" => syntax(0, 0, "outdated-only", "", ""),
        "agent prompt" => syntax(2, 2, "wait", "until timeout", ""),
        "agent rename" | "agent set-parent" => syntax(1, 2, "clear", "", ""),
        "agent get" | "agent focus" | "pane get" | "pane close" | "workspace get"
        | "workspace focus" | "workspace close" | "tab get" | "tab focus" | "tab close"
        | "plugin pane focus" | "plugin pane close" => syntax(1, 1, "", "", ""),
        "pane list" | "tab list" => syntax(0, 0, "", "workspace", ""),
        "workspace create" => syntax(0, 0, "focus no-focus", "cwd label env", ""),
        "tab create" => syntax(0, 0, "focus no-focus", "workspace cwd label env", ""),
        "workspace rename" | "tab rename" | "pane run" | "pane send-text" | "pane send-keys"
        | "agent send-keys" => syntax(2, usize::MAX, "", "", ""),
        "pane process-info" | "pane current" | "pane layout" | "pane edges" => {
            syntax(0, 0, "current", "pane", "")
        }
        "pane read" => syntax(1, 1, "ansi raw", "source lines format", ""),
        "agent read" => syntax(1, 1, "ansi", "source lines format", ""),
        "pane report-metadata" => syntax(
            1,
            1,
            "clear-title clear-display-agent clear-state-labels",
            "source agent applies-to-source title display-agent state-label token clear-token seq ttl-ms",
            "source",
        ),
        "workspace report-metadata" => {
            syntax(1, 1, "", "source token clear-token seq ttl-ms", "source")
        }
        "notification show" => syntax(1, 1, "", "body position sound", ""),
        "session list" | "machine list" => syntax(0, 0, "json", "", ""),
        "plugin list" => syntax(0, 0, "json", "plugin", ""),
        "session stop" | "session delete" => syntax(1, 1, "json", "", ""),
        "plugin link" => syntax(1, 1, "enabled disabled", "", ""),
        "plugin config-dir" | "plugin enable" | "plugin disable" | "plugin unlink" => {
            syntax(1, 1, "", "", "")
        }
        "plugin action invoke" => syntax(1, 1, "", "plugin", ""),
        "plugin action list" => syntax(0, 0, "", "plugin", ""),
        "plugin log list" => syntax(0, 0, "", "plugin limit", ""),
        "plugin pane open" => syntax(
            0,
            0,
            "focus no-focus",
            "plugin entrypoint placement width height workspace target-pane direction cwd env",
            "plugin entrypoint",
        ),
        other => return Err(format!("unmodelled herdr contract: {other}")),
    };
    let expanded;
    if matches!(path.as_str(), "pane read" | "pane wait-output") {
        expanded = args
            .iter()
            .flat_map(|arg| {
                if let Some((flag, value)) = arg.split_once('=')
                    && [
                        "--source",
                        "--lines",
                        "--format",
                        "--match",
                        "--regex",
                        "--timeout",
                    ]
                    .contains(&flag)
                {
                    vec![flag.to_string(), value.to_string()]
                } else {
                    vec![arg.clone()]
                }
            })
            .collect::<Vec<_>>();
        args = &expanded;
    }
    let parsed = parse(
        args,
        rule,
        path == "agent start",
        !matches!(path.as_str(), "pane read" | "pane wait-output"),
    )?;
    for flag in ["timeout", "seq", "ttl-ms", "limit"] {
        for value in parsed.all(flag) {
            u64_value(value)?;
        }
    }
    for value in parsed.all("lines") {
        value.parse::<u32>().map_err(|_| "lines must be a u32")?;
    }
    for env in parsed.all("env") {
        if env.split_once('=').is_none_or(|(key, _)| key.is_empty()) {
            return Err("invalid_env: expected nonempty KEY=VALUE".into());
        }
    }
    for (flag, choices) in [
        ("until", "idle working blocked done unknown"),
        ("format", "text ansi"),
        ("placement", "overlay popup split tab zoomed fullscreen"),
        ("direction", "right down"),
        ("position", "top-left top-right bottom-left bottom-right"),
        ("sound", "none done request"),
    ] {
        for value in parsed.all(flag) {
            one_of(value, choices)?;
        }
    }
    if matches!(
        path.as_str(),
        "pane read" | "agent read" | "pane wait-output"
    ) && let Some(value) = parsed.value("source")
    {
        one_of(
            value,
            "visible recent recent-unwrapped recent_unwrapped detection",
        )?;
    }
    if path.starts_with("plugin ") {
        if let Some(value) = parsed.value("plugin") {
            identifier(value, true)?;
        }
        if let Some(value) = parsed.value("entrypoint") {
            identifier(value, false)?;
        }
        if matches!(
            path.as_str(),
            "plugin config-dir" | "plugin enable" | "plugin disable" | "plugin unlink"
        ) {
            identifier(parsed.positional[0], true)?;
        }
        if path == "plugin action invoke" {
            identifier(parsed.positional[0], false)?;
        }
        if (parsed.has("width") || parsed.has("height"))
            && parsed
                .value("placement")
                .is_some_and(|value| value != "popup")
        {
            return Err("width/height require popup placement".into());
        }
        for value in parsed.all("width").chain(parsed.all("height")) {
            let percentage = value.ends_with('%');
            let value = value
                .strip_suffix('%')
                .unwrap_or(value)
                .parse::<u16>()
                .map_err(|_| "invalid popup dimension")?;
            if percentage && !(1..=100).contains(&value) {
                return Err("invalid popup dimension".into());
            }
        }
    }
    match path.as_str() {
        "pane wait-output" => {
            if parsed.has("match") == parsed.has("regex") {
                return Err("expected --match OR --regex".into());
            }
            if let Some(value) = parsed.value("regex") {
                regex::Regex::new(value).map_err(|error| format!("invalid_regex: {error}"))?;
            }
        }
        "agent start" => {
            agent_name(parsed.positional[0])?;
            // detect/mod.rs: interactive executable is absent only for chatgpt.
            one_of(
                parsed.value("kind").unwrap(),
                "pi claude codex gemini cursor devin agy cline omp mastracode opencode copilot kimi kiro droid amp grok hermes kilo qodercli qwen letta maki muse dsh",
            ).or_else(|_| agent_kind_alias(parsed.value("kind").unwrap()))?;
            for pane in parsed.all("pane") {
                if !pane_id(pane) {
                    return Err("invalid pane id".into());
                }
            }
            for parent in parsed.all("parent") {
                if !parent_id(parent) {
                    return Err("invalid parent pane id".into());
                }
            }
            let timeout = parsed
                .value("timeout")
                .map(u64_value)
                .transpose()?
                .unwrap_or(30_000);
            if !(3_001..=300_000).contains(&timeout) {
                return Err("invalid_agent_timeout".into());
            }
            if parsed
                .trailing
                .iter()
                .any(|arg| arg.chars().any(char::is_control))
            {
                return Err("invalid_agent_argument: control character".into());
            }
        }
        "agent rename" | "agent set-parent" => {
            if (parsed.positional.len() == 2) == parsed.has("clear") {
                return Err("expected name/parent OR --clear".into());
            }
            if parsed.positional.len() == 2 {
                if path == "agent rename" {
                    agent_name(parsed.positional[1])?;
                } else if !parent_id(parsed.positional[1]) {
                    return Err("invalid parent pane id".into());
                }
            }
        }
        "agent prompt" => {
            if parsed.positional[1].is_empty() {
                return Err("empty_agent_prompt".into());
            }
            if (parsed.has("until") || parsed.has("timeout")) && !parsed.has("wait") {
                return Err("--until/--timeout require --wait".into());
            }
        }
        "pane send-keys" | "agent send-keys" => {
            for value in &parsed.positional[1..] {
                key(value)?;
            }
        }
        "pane report-metadata" | "workspace report-metadata" => {
            metadata(&parsed, path.starts_with("pane"))?
        }
        "session stop" | "session delete" => {
            session_name(parsed.positional[0])?;
            if path == "session delete" && parsed.positional[0] == "default" {
                return Err("deleting default session is not supported".into());
            }
        }
        _ => {}
    }
    Ok(())
}

fn normalized_text(value: &str) -> Option<String> {
    let value = value
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(80)
        .collect::<String>();
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

fn parent_id(value: &str) -> bool {
    pane_id(value)
        || value
            .split_once(':')
            .is_some_and(|(label, pane)| !label.trim().is_empty() && pane_id(pane))
}

fn agent_kind_alias(value: &str) -> Check {
    let mut value = value.trim().to_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js"] {
        if value.ends_with(suffix) {
            value.truncate(value.len() - suffix.len());
            break;
        }
    }
    let value = value
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(&value);
    if [
        "pi",
        "claude",
        "claude-code",
        "codex",
        "gemini",
        "cursor",
        "cursor-agent",
        "devin",
        "devin-cli",
        "devin cli",
        "agy",
        "antigravity",
        "antigravity-cli",
        "cline",
        ".cline",
        "omp",
        "mastracode",
        "mastra-code",
        "mastra code",
        "opencode",
        "opencode2",
        "open-code",
        "copilot",
        "github-copilot",
        "ghcs",
        "kimi",
        "kimi-code",
        "kimi code",
        "kiro",
        "kiro-cli",
        "droid",
        "amp",
        "amp-local",
        "grok",
        "grok-build",
        "hermes",
        "hermes-agent",
        "kilo",
        "kilo-code",
        "kilo code",
        "qodercli",
        "qoderclicn",
        "qoder",
        "qodercn",
        "qwen",
        "qwen-code",
        "qwen code",
        "letta",
        "letta-code",
        "letta code",
        "maki",
        "muse",
        "muse-code",
        "muse-cli",
        "dsh",
        "dsh-tui",
        "dst",
    ]
    .contains(&value)
        || value
            .strip_prefix("muse-bin-")
            .is_some_and(|rest| rest.starts_with(|ch: char| ch.is_ascii_digit()))
    {
        Ok(())
    } else {
        Err("unsupported interactive agent kind".into())
    }
}

fn session_name(value: &str) -> Check {
    if !value.is_empty()
        && value.len() <= 64
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"._-".contains(&ch))
    {
        Ok(())
    } else {
        Err("invalid session name".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn metadata_boundaries_follow_normalization_not_invented_value_limits() {
        let check = |extra: Vec<String>| {
            let cmd = Cmd::new("herdr", Duration::from_secs(1))
                .args(["pane", "report-metadata", "w1:p1", "--source", "ade"])
                .args(extra);
            validate(&cmd)
        };
        for (count, accepted) in [(16, true), (17, false)] {
            let args = (0..count)
                .flat_map(|i| ["--token".to_string(), format!("key{i}=value")])
                .collect();
            assert_eq!(check(args).is_ok(), accepted);
        }
        for (len, accepted) in [(32, true), (33, false)] {
            assert_eq!(
                check(vec!["--token".into(), format!("{}=value", "k".repeat(len))]).is_ok(),
                accepted
            );
        }
        for ttl in [1, 86_400_000] {
            assert!(
                check(vec![
                    "--ttl-ms".into(),
                    ttl.to_string(),
                    "--token".into(),
                    "key=value".into()
                ])
                .is_ok()
            );
        }
        assert!(
            check(vec![
                "--token".into(),
                format!("key={}", "value".repeat(100))
            ])
            .is_ok()
        );
        assert!(check(vec!["--token".into(), "parent=w1:p1".into()]).is_err());
        assert!(
            check(vec![
                "--token".into(),
                "parent=w2:p1".into(),
                "--clear-token".into(),
                "parent".into(),
                "--ttl-ms".into(),
                "1".into()
            ])
            .is_ok()
        );
        assert!(
            check(vec![
                "--clear-token".into(),
                "parent".into(),
                "--token".into(),
                "parent=w2:p1".into(),
                "--ttl-ms".into(),
                "1".into()
            ])
            .is_err()
        );
        assert!(
            check(vec![
                "--token".into(),
                "parent=\u{1}".into(),
                "--ttl-ms".into(),
                "1".into()
            ])
            .is_ok()
        );
    }

    #[test]
    #[should_panic(expected = "fake herdr rejected")]
    fn even_scripted_failures_cannot_hide_invalid_arguments() {
        use super::super::{
            Runner,
            fake::{FakeRunner, fail},
        };
        let runner = FakeRunner::new();
        runner.on("pane read", fail(1, "deliberate transport error"));
        let _ = runner.run(
            &Cmd::new("herdr", Duration::from_secs(1))
                .args(["pane", "read", "w1:p1", "--source", "screen"]),
        );
    }

    #[test]
    fn rejects_fork_argument_errors_and_preserves_legal_edge_values() {
        for args in [
            vec!["agent", "start", "UPPER", "--kind", "pi", "--pane", "w1:p1"],
            vec!["agent", "start", "worker", "--pane", "w1:p1"],
            vec!["agent", "start", "worker", "--kind", "pi"],
            vec![
                "agent", "start", "worker", "--kind", "unknown", "--pane", "w1:p1",
            ],
            vec!["agent", "start", "worker", "--kind", "pi", "--pane", "bad"],
            vec![
                "agent",
                "start",
                "worker",
                "--kind",
                "pi",
                "--pane",
                "w1:p1",
                "--timeout",
                "3000",
            ],
            vec![
                "agent",
                "start",
                "worker",
                "--kind",
                "pi",
                "--pane",
                "w1:p1",
                "--timeout",
                "300001",
            ],
            vec![
                "agent",
                "start",
                "worker",
                "--kind",
                "pi",
                "--pane",
                "w1:p1",
                "--",
                "newline\n",
            ],
            vec!["agent", "rename", "w1:p1", "two words"],
            vec!["agent", "prompt", "w1:p1", ""],
            vec!["agent", "prompt", "w1:p1", "text", "--timeout", "1"],
            vec!["agent", "wait", "w1:p1", "--until", "starting"],
            vec!["agent", "wait", "w1:p1", "--timeout", "-1"],
            vec!["pane", "read", "w1:p1", "--source", "screen"],
            vec!["pane", "read", "w1:p1", "--format", "json"],
            vec!["pane", "read", "w1:p1", "--lines", "4294967296"],
            vec!["pane", "read", "--unknown"],
            vec!["pane", "wait-output", "w1:p1", "--regex", "["],
            vec!["pane", "wait-output", "w1:p1"],
            vec![
                "plugin",
                "pane",
                "open",
                "--plugin",
                "ade",
                "--entrypoint",
                "tab",
                "--placement",
                "popup",
                "--width",
                "80%%",
            ],
            vec![
                "agent", "start", "worker", "--kind", "pi", "--pane", "bad", "--pane", "w1:p1",
            ],
            vec!["plugin", "pane", "open", "--plugin", "ade"],
            vec![
                "plugin",
                "pane",
                "open",
                "--plugin",
                "bad id",
                "--entrypoint",
                "tab",
            ],
            vec![
                "plugin",
                "pane",
                "open",
                "--plugin",
                "ade",
                "--entrypoint",
                "bad.id",
            ],
            vec!["pane", "send-keys", "w1:p1", "unsupported"],
            vec!["workspace", "create", "--env", "=empty-key"],
            vec!["workspace", "create", "--env", "MISSING_SEPARATOR"],
            vec!["pane", "report-metadata", "w1:p1", "--token", "ok=v"],
            vec![
                "pane",
                "report-metadata",
                "w1:p1",
                "--source",
                "bad source",
                "--token",
                "ok=v",
            ],
            vec![
                "pane",
                "report-metadata",
                "w1:p1",
                "--source",
                "ade",
                "--ttl-ms",
                "0",
                "--token",
                "ok=v",
            ],
            vec![
                "pane",
                "report-metadata",
                "w1:p1",
                "--source",
                "ade",
                "--ttl-ms",
                "86400001",
                "--token",
                "ok=v",
            ],
            vec![
                "pane",
                "report-metadata",
                "w1:p1",
                "--source",
                "ade",
                "--ttl-ms",
                "1",
                "--token",
                "parent=w2:p1",
            ],
            vec![
                "pane",
                "report-metadata",
                "w1:p1",
                "--source",
                "ade",
                "--token",
                "bad.key=v",
            ],
            vec!["pane", "report-metadata", "w1:p1", "--source", "ade"],
            vec![
                "pane",
                "report-metadata",
                "w1:p1",
                "--source",
                "ade",
                "--title",
                "title",
                "--clear-title",
            ],
            vec!["session", "stop", "../escape"],
            vec!["session", "delete", "default"],
            vec!["notification", "show", "title", "--sound", "loud"],
            vec!["plugin", "pane", "open", "--placement", "unknown"],
        ] {
            let cmd = Cmd::new("herdr", Duration::from_secs(1)).args(args.clone());
            assert!(validate(&cmd).is_err(), "accepted {args:?}");
        }
        for args in [
            vec![
                "agent",
                "start",
                "a",
                "--kind",
                "pi",
                "--pane",
                "w1:p1",
                "--timeout",
                "3001",
                "--env",
                "EMPTY=",
                "--",
                "--timeout",
                "0",
            ],
            vec!["agent", "wait", "w1:p1", "--timeout", "0"],
            vec![
                "agent",
                "start",
                "worker",
                "--kind",
                "CLAUDE-CODE.exe",
                "--pane",
                "w1:pA",
                "--parent",
                "box:w1:p1",
            ],
            vec![
                "pane",
                "read",
                "--source=recent_unwrapped",
                "--lines=4294967295",
                "w1:p1",
            ],
            vec!["pane", "run", "w1:p1", "command", "--literal-argument"],
            vec![
                "agent",
                "wait",
                "w1:p1",
                "--timeout",
                "18446744073709551615",
            ],
            vec![
                "agent",
                "prompt",
                "w1:p1",
                "--literal\ntext",
                "--wait",
                "--until",
                "blocked",
            ],
            vec![
                "pane",
                "send-keys",
                "w1:p1",
                "C-c",
                "+",
                "shift+tab",
                "f255",
                "ö",
            ],
            vec![
                "pane",
                "report-metadata",
                "w1:p1",
                "--source",
                " ade:source ",
                "--token",
                "ok=control\nvalue",
            ],
        ] {
            let cmd = Cmd::new("herdr", Duration::from_secs(1)).args(args.clone());
            assert!(
                validate(&cmd).is_ok(),
                "rejected {args:?}: {:?}",
                validate(&cmd)
            );
        }
        assert!(agent_name(&"a".repeat(32)).is_ok());
        assert!(agent_name(&"a".repeat(33)).is_err());
        assert!(source(&"a".repeat(80)).is_ok());
        assert!(source(&"a".repeat(81)).is_err());
        assert!(session_name(&"a".repeat(64)).is_ok());
        assert!(session_name(&"a".repeat(65)).is_err());
    }
}
