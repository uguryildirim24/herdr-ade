//! The start line (§3.4) and the wrapper the plugin writes (§3.2).
//!
//! Interactive TUI only, never print mode: `herdr agent start <name> --kind pi
//! --pane <pane> --parent <coord> --timeout <ms> -- --provider <p> --model <m>
//! --thinking <level> --no-skills`. `--approve` is never passed; the settings
//! file already settles trust. The wrapper is what `pi` resolves to, and it
//! supplies `PI_CODING_AGENT_DIR` on a cold restore where no plugin env is
//! left.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// The provider and model names an executable recipe may start. Routing
/// recipes use exactly these strings (SPEC-pi v2 §3.4, §3.5).
/// `opencode-go` is the OpenCode Go plan (the DeepSeek and Muse rows, SPEC-ADE
/// §6 items 65, 80). `pro` is the local relay from `herdr-pro serve`.
pub const PROVIDERS: [&str; 4] = ["openai-codex", "opencode-go", "kimi-coding", "pro"];

/// Flags a `kind = "pi"` recipe may never carry (SPEC-pi v2 §3.5):
/// `pi_args_forbidden`. `-na` is `--no-approve`'s short form (§1) and
/// `--no-session` is a session flag the r2 strip list also drops (§3.8).
const FORBIDDEN_ARGS: [&str; 15] = [
    "--approve",
    "-a",
    "--no-approve",
    "-na",
    "--session",
    "--no-session",
    "--fork",
    "-c",
    "--continue",
    "-r",
    "--resume",
    "--config-dir",
    "-e",
    "--extension",
    "--no-extensions",
];

/// The wrapper script written by setup and linked as `~/.local/bin/pi`
/// (SPEC-pi v2 §3.2). `exec` keeps the pane's foreground process the node
/// process, which herdr's process match and `agent prompt` need.
///
/// The `~/.pi` refusal checks the folder pi will really use: pi expands a
/// leading `~` in `PI_CODING_AGENT_DIR` itself and resolves relative paths,
/// so the wrapper expands `~`, makes the path absolute, squeezes `//`,
/// refuses `.` and `..` parts, and compares both the path and its physical
/// form (symlinks resolved on the nearest existing folder) against `~/.pi`.
fn wrapper_script(agent_dir: &Path, cli_js: &Path) -> String {
    format!(
        r#"#!/bin/sh
# herdr-ade pi wrapper — pinned {package}@{version}
# Written by `herdr-pi setup`. The plugin owns this file; do not hand-edit.
fail() {{
  printf '%s\n' "$1" >&2
  exit 2
}}

home=${{HOME:-}}
if [ -n "${{PI_CODING_AGENT_DIR:-}}" ]; then
  agent=$PI_CODING_AGENT_DIR
else
  agent={agent}
fi

# pi expands a leading ~ and resolves a relative path; do the same first.
case "$agent" in
  "~") agent=$home ;;
  "~/"*) agent=$home/${{agent#"~/"}} ;;
esac
case "$agent" in
  /*) ;;
  *) agent=$PWD/$agent ;;
esac
agent=$(printf '%s\n' "$agent" | tr -s /)
case "$agent" in
  */./* | */. | */../* | */..)
    fail "herdr-ade pi: refusing a config dir with . or .. in it: $agent" ;;
esac

physical() {{
  dir=$1
  rest=
  while [ ! -d "$dir" ] && [ "$dir" != / ] && [ -n "$dir" ]; do
    rest=/${{dir##*/}}$rest
    dir=${{dir%/*}}
    [ -n "$dir" ] || dir=/
  done
  base=$(cd "$dir" 2>/dev/null && pwd -P) || base=$dir
  printf '%s\n' "$base$rest" | tr -s /
}}

pi_home() {{
  case "$1" in
    "$2/.pi" | "$2/.pi/"*) return 0 ;;
  esac
  return 1
}}

if [ -n "$home" ]; then
  real_home=$(physical "$home")
  real_agent=$(physical "$agent")
  if pi_home "$agent" "$home" || pi_home "$real_agent" "$home" ||
    pi_home "$agent" "$real_home" || pi_home "$real_agent" "$real_home"; then
    fail "herdr-ade pi: refusing a config dir under ~/.pi; use PI_CODING_AGENT_DIR elsewhere"
  fi
fi

case "${{1:-}}" in
  install | remove | uninstall | update | config)
    fail "herdr-ade pi: '$1' is refused; run herdr-pi setup, which owns the pinned install" ;;
esac

command -v node >/dev/null 2>&1 || fail "herdr-ade pi: node is not on PATH; install Node >= {min_node}"
node_version=$(node --version 2>/dev/null || true)
node_version=${{node_version#v}}
printf '%s\n' "$node_version" | awk -F. '{{
  exit !(($1+0) > 22 || (($1+0) == 22 && (($2+0) > 19 || (($2+0) == 19 && ($3+0) >= 0))))
}}' || fail "herdr-ade pi: node $node_version is older than {min_node}"

PI_CODING_AGENT_DIR=$agent
export PI_CODING_AGENT_DIR
PI_SKIP_VERSION_CHECK=1
PI_TELEMETRY=0
export PI_SKIP_VERSION_CHECK PI_TELEMETRY

exec node {cli} "$@"
"#,
        package = super::PI_PACKAGE,
        version = super::PI_VERSION,
        agent = sh_quote(&agent_dir.display().to_string()),
        cli = sh_quote(&cli_js.display().to_string()),
        min_node = min_node_string(),
    )
}

/// One single-quoted `sh` word, so a baked path with a quote, `$` or a
/// backtick stays a path.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

pub fn min_node_string() -> String {
    let (a, b, c) = super::MIN_NODE;
    format!("{a}.{b}.{c}")
}

/// Write the wrapper and make it executable.
pub fn write_wrapper(layout: &super::Layout) -> Result<PathBuf> {
    let path = layout.wrapper();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, wrapper_script(&layout.agent(), &layout.cli_js()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(path)
}

/// The args after `--` on the start line (SPEC-pi v2 §3.4).
pub fn start_args(provider: &str, model: &str, thinking: &str) -> Vec<String> {
    vec![
        "--provider".into(),
        provider.into(),
        "--model".into(),
        model.into(),
        "--thinking".into(),
        thinking.into(),
        "--no-skills".into(),
    ]
}

/// The full `herdr agent start` argv for kind `pi`. `session` is ADE's
/// `launch.resume_session` (a thread restart), never a recipe value.
pub fn agent_start_args(
    name: &str,
    pane: &str,
    parent: &str,
    timeout_ms: u64,
    recipe_args: &[String],
    session: Option<&Path>,
) -> Result<Vec<String>> {
    validate_args(recipe_args)?;
    let mut args = vec![
        "agent".to_string(),
        "start".to_string(),
        name.to_string(),
        "--kind".to_string(),
        "pi".to_string(),
        "--pane".to_string(),
        pane.to_string(),
        "--parent".to_string(),
        parent.to_string(),
        "--timeout".to_string(),
        timeout_ms.to_string(),
        "--".to_string(),
    ];
    args.extend(recipe_args.iter().cloned());
    if let Some(session) = session {
        args.extend(super::resume::append_resume_session(session, recipe_args)?);
    }
    Ok(args)
}

/// A `kind = "pi"` recipe is refused when it carries a session or trust flag
/// (SPEC-pi v2 §3.5). The code is part of the message, so callers can name it.
pub fn validate_args(args: &[String]) -> Result<()> {
    for arg in args {
        let flag = arg.split('=').next().unwrap_or(arg);
        if FORBIDDEN_ARGS.contains(&flag) {
            bail!("pi_args_forbidden: `{flag}` is not allowed on a pi row");
        }
    }
    if args.iter().any(|a| a == "--force") {
        bail!("pi_args_forbidden: `--force` never appears on a pi row");
    }
    let provider = flag_value(args, "--provider")
        .ok_or_else(|| anyhow::anyhow!("pi_args_forbidden: `--provider` is required"))?;
    if provider.eq_ignore_ascii_case("cursor") {
        bail!("pi_cursor_forbidden: Cursor stays outside pi (decision 18:30)");
    }
    if args.iter().any(|a| {
        let v = a.to_ascii_lowercase();
        v.contains("cursor/sdk") || v.contains("pi-cursor")
    }) {
        bail!("pi_cursor_forbidden: no Cursor SDK or community add-on under pi");
    }
    // Every pi row, a known provider or not (the T3 mock row too).
    if flag_value(args, "--model").is_none_or(|model| model.is_empty()) {
        bail!("pi_args_forbidden: `--model` is required on a pi row");
    }
    if !args.iter().any(|a| a == "--no-skills") {
        bail!("pi_args_forbidden: `--no-skills` is required on a pi row");
    }
    Ok(())
}

/// `env` on a pi row must not set `PI_CODING_AGENT_DIR`
/// (SPEC-pi v2 §3.5: the wrapper supplies it).
fn validate_env(env: &[(String, String)]) -> Result<()> {
    for (key, _) in env {
        if key == "PI_CODING_AGENT_DIR" {
            bail!("pi_env_forbidden: the wrapper supplies PI_CODING_AGENT_DIR");
        }
    }
    Ok(())
}

/// `provider` equals the `--provider` the args carry (SPEC-pi v2 §3.5).
pub fn validate_provider_column(provider: &str, args: &[String]) -> Result<()> {
    let in_args = flag_value(args, "--provider")
        .ok_or_else(|| anyhow::anyhow!("pi_args_forbidden: `--provider` is required"))?;
    if provider != in_args {
        bail!("pi_args_forbidden: provider `{provider}` does not equal --provider `{in_args}`");
    }
    Ok(())
}

/// The value of `--flag value` or `--flag=value`.
pub fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
        if let Some(rest) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

/// Seven levels; the chosen rows are all valid, but the table clamps some
/// (SPEC-pi v2 §1, §3.4).
const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Models that clamp a level; a custom model with no thinking map clamps
/// `xhigh` to off silently, so the recipe table is a check, not a hope.
fn thinking_supported(provider: &str, model: &str, level: &str) -> Option<bool> {
    let _ = provider;
    match (model, level) {
        ("k3", "xhigh") => Some(false),
        _ => Some(true),
    }
}

/// Refuse an unknown level or a level the model clamps.
pub fn validate_thinking(provider: &str, model: &str, level: &str) -> Result<()> {
    if !THINKING_LEVELS.contains(&level) {
        bail!("pi_args_forbidden: `{level}` is not one of the seven thinking levels");
    }
    match thinking_supported(provider, model, level) {
        Some(true) => Ok(()),
        Some(false) => bail!("pi_args_forbidden: `{model}` has no `{level}` thinking level"),
        None => bail!("pi_args_forbidden: `{model}` is not a known model"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn start_args_are_exactly_the_spec_line() {
        let recipe = args(&[
            "--provider",
            "kimi-coding",
            "--model",
            "k3",
            "--thinking",
            "low",
            "--no-skills",
        ]);
        let start = agent_start_args("a5", "w1F:p13", "w1F:p1", 30000, &recipe, None).unwrap();
        assert_eq!(
            start,
            args(&[
                "agent",
                "start",
                "a5",
                "--kind",
                "pi",
                "--pane",
                "w1F:p13",
                "--parent",
                "w1F:p1",
                "--timeout",
                "30000",
                "--",
                "--provider",
                "kimi-coding",
                "--model",
                "k3",
                "--thinking",
                "low",
                "--no-skills",
            ])
        );
        assert!(!start.iter().any(|a| a == "--approve"));
        assert!(!start.iter().any(|a| a == "-a"));
    }

    #[test]
    fn forbidden_flags_and_cursor_are_refused() {
        for flag in FORBIDDEN_ARGS {
            let bad = args(&["--provider", "kimi-coding", "--model", "x", flag]);
            let error = validate_args(&bad).unwrap_err().to_string();
            assert!(error.contains("pi_args_forbidden"), "{flag}: {error}");
        }
        for provider in ["cursor", "Cursor", "CURSOR"] {
            let cursor = args(&["--provider", provider, "--model", "x", "--no-skills"]);
            assert!(
                validate_args(&cursor)
                    .unwrap_err()
                    .to_string()
                    .contains("pi_cursor_forbidden"),
                "{provider}"
            );
        }
        let sdk = args(&[
            "--provider",
            "kimi-coding",
            "--model",
            "x",
            "--extension",
            "@cursor/sdk",
        ]);
        let error = validate_args(&sdk).unwrap_err().to_string();
        assert!(error.contains("pi_cursor_forbidden") || error.contains("pi_args_forbidden"));
    }

    #[test]
    fn model_and_no_skills_are_required_for_every_provider() {
        let mock = args(&[
            "--provider",
            "mock-provider",
            "--model",
            "mock-model",
            "--thinking",
            "low",
            "--no-skills",
        ]);
        assert!(validate_args(&mock).is_ok());
        for bad in [
            args(&["--provider", "mock-provider", "--no-skills"]),
            args(&["--provider", "mock-provider", "--model", "m"]),
            args(&["--provider", "kimi-coding", "--model", "", "--no-skills"]),
        ] {
            let error = validate_args(&bad).unwrap_err().to_string();
            assert!(error.contains("pi_args_forbidden"), "{bad:?}: {error}");
        }
    }

    #[test]
    fn env_with_the_agent_dir_is_refused() {
        let env = vec![("PI_CODING_AGENT_DIR".to_string(), "/x".to_string())];
        assert!(
            validate_env(&env)
                .unwrap_err()
                .to_string()
                .contains("pi_env_forbidden")
        );
        assert!(validate_env(&[]).is_ok());
    }

    #[test]
    fn the_provider_column_must_match() {
        let row = args(&["--provider", "kimi-coding", "--model", "x", "--no-skills"]);
        assert!(validate_provider_column("kimi-coding", &row).is_ok());
        assert!(validate_provider_column("opencode-go", &row).is_err());
    }

    /// The wrapper, run for real with `sh` and a fake `node` that prints
    /// the folder it was handed: every spelling of `~/.pi` is refused, and
    /// the baked folder and an outside folder pass.
    #[test]
    #[cfg(unix)]
    fn the_wrapper_refuses_every_spelling_of_pi_home() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        std::fs::create_dir_all(home.join("work")).unwrap();
        std::os::unix::fs::symlink(&home, dir.path().join("homelink")).unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let node = bin.join("node");
        std::fs::write(
            &node,
            "#!/bin/sh\n[ \"$1\" = --version ] && { echo v22.19.0; exit 0; }\nprintf '%s\\n' \"$PI_CODING_AGENT_DIR\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
        let baked = dir.path().join("it's $state/pi/agent");
        let wrapper = dir.path().join("pi");
        std::fs::write(
            &wrapper,
            wrapper_script(&baked, Path::new("/nowhere/cli.js")),
        )
        .unwrap();

        let run = |agent_dir: Option<&str>, cwd: &Path| {
            let mut cmd = std::process::Command::new("sh");
            cmd.arg(&wrapper)
                .arg("--version")
                .current_dir(cwd)
                .env_clear()
                .env("HOME", &home)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
            if let Some(agent_dir) = agent_dir {
                cmd.env("PI_CODING_AGENT_DIR", agent_dir);
            }
            cmd.output().unwrap()
        };

        let out = run(None, &home);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            baked.display().to_string()
        );
        let outside = dir.path().join("elsewhere");
        let out = run(Some(&outside.display().to_string()), &home);
        assert!(out.status.success(), "{out:?}");

        let link = dir.path().join("homelink/.pi/agent").display().to_string();
        let double = format!("{}//.pi/agent", home.display());
        let dotted = format!("{}/work/../.pi/agent", home.display());
        let absolute = home.join(".pi/new").display().to_string();
        for bad in [
            "~/.pi/agent",
            "~/.pi",
            ".pi/agent",
            absolute.as_str(),
            double.as_str(),
            dotted.as_str(),
            link.as_str(),
        ] {
            let out = run(Some(bad), &home);
            assert_eq!(out.status.code(), Some(2), "{bad}: {out:?}");
            assert!(out.stdout.is_empty(), "{bad} reached node");
        }
        let out = std::process::Command::new("sh")
            .arg(&wrapper)
            .arg("install")
            .env("HOME", &home)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
    }

    #[test]
    fn thinking_levels_are_checked_against_the_clamps() {
        assert!(validate_thinking("kimi-coding", "k3", "high").is_ok());
        assert!(validate_thinking("kimi-coding", "k3", "xhigh").is_err());
        assert!(validate_thinking("kimi-coding", "x", "sometimes").is_err());
    }
}
