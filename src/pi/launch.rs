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
/// §6 items 65, 80).
pub(crate) const PROVIDERS: [&str; 3] = ["openai-codex", "opencode-go", "kimi-coding"];

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

pub(crate) fn min_node_string() -> String {
    let (a, b, c) = super::MIN_NODE;
    format!("{a}.{b}.{c}")
}

/// Write the wrapper and make it executable.
pub(crate) fn write_wrapper(layout: &super::Layout) -> Result<PathBuf> {
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

/// The complete interactive recipe contract, independent of argv spelling.
#[derive(Debug)]
pub(crate) struct PiArgs {
    provider: String,
    model: String,
    thinking: String,
}

impl PiArgs {
    pub(crate) fn argv(&self) -> Vec<String> {
        vec![
            "--provider".into(),
            self.provider.clone(),
            "--model".into(),
            self.model.clone(),
            "--thinking".into(),
            self.thinking.clone(),
            "--no-skills".into(),
        ]
    }
}

/// Parse only the four admitted options, once each. Session, trust and
/// extension flags remain excluded, as do print mode and all other options.
pub(crate) fn parse_args(args: &[String]) -> Result<PiArgs> {
    let (mut provider, mut model, mut thinking) = (None, None, None);
    let mut no_skills = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
        if FORBIDDEN_ARGS.contains(&flag) {
            bail!("pi_args_forbidden: `{flag}` is not allowed on a pi row");
        }
        if flag == "--no-skills" && inline.is_none() && !no_skills {
            no_skills = true;
            continue;
        }
        let slot = match flag {
            "--provider" => &mut provider,
            "--model" => &mut model,
            "--thinking" => &mut thinking,
            _ => bail!("pi_args_forbidden: `{arg}` is not an interactive recipe option"),
        };
        if slot.is_some() {
            bail!("pi_args_forbidden: duplicate `{flag}`");
        }
        let value = inline.or_else(|| iter.next().map(String::as_str));
        let value = value
            .filter(|v| !v.is_empty() && !v.starts_with('-'))
            .ok_or_else(|| anyhow::anyhow!("pi_args_forbidden: `{flag}` requires a value"))?;
        *slot = Some(value.to_string());
    }
    if !no_skills {
        bail!("pi_args_forbidden: `--no-skills` is required on a pi row");
    }
    let required = |value: Option<String>, flag| {
        value.ok_or_else(|| anyhow::anyhow!("pi_args_forbidden: `{flag}` is required"))
    };
    Ok(PiArgs {
        provider: required(provider, "--provider")?,
        model: required(model, "--model")?,
        thinking: required(thinking, "--thinking")?,
    })
}

/// Check the provider column against the parsed interactive recipe.
pub(crate) fn validate_provider_column(provider: &str, args: &[String]) -> Result<()> {
    let parsed = parse_args(args)?;
    if provider != parsed.provider {
        bail!(
            "pi_args_forbidden: provider `{provider}` does not equal --provider `{}`",
            parsed.provider
        );
    }
    Ok(())
}

/// Validate one row from the canonical TOML recipe catalog.
pub(crate) fn validate_recipe(
    id: &str,
    provider: &str,
    args: &[String],
    env: &[String],
) -> Result<String> {
    let parsed = parse_args(args)?;
    if provider != parsed.provider {
        bail!(
            "pi_args_forbidden: provider `{provider}` does not equal --provider `{}`",
            parsed.provider
        );
    }
    validate_thinking(provider, &parsed.model, &parsed.thinking)?;
    if !env.is_empty() {
        bail!("pi_env_forbidden: `{id}` must leave the pi environment empty");
    }
    Ok(parsed.model)
}

/// The value of `--flag value` or `--flag=value`.
pub(crate) fn flag_value(args: &[String], flag: &str) -> Option<String> {
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
pub(crate) fn validate_thinking(provider: &str, model: &str, level: &str) -> Result<()> {
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
    fn forbidden_flags_are_refused() {
        for flag in FORBIDDEN_ARGS {
            let bad = args(&["--provider", "kimi-coding", "--model", "x", flag]);
            let error = parse_args(&bad).unwrap_err().to_string();
            assert!(error.contains("pi_args_forbidden"), "{flag}: {error}");
        }
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
        assert!(parse_args(&mock).is_ok());
        for bad in [
            args(&["--provider", "mock-provider", "--no-skills"]),
            args(&["--provider", "mock-provider", "--model", "m"]),
            args(&["--provider", "kimi-coding", "--model", "", "--no-skills"]),
        ] {
            let error = parse_args(&bad).unwrap_err().to_string();
            assert!(error.contains("pi_args_forbidden"), "{bad:?}: {error}");
        }
    }

    #[test]
    fn the_provider_column_must_match() {
        let row = args(&[
            "--provider",
            "kimi-coding",
            "--model",
            "x",
            "--thinking",
            "low",
            "--no-skills",
        ]);
        assert!(validate_recipe("row", "kimi-coding", &row, &[]).is_ok());
        assert!(validate_recipe("row", "opencode-go", &row, &[]).is_err());
    }

    #[test]
    fn equals_style_cannot_smuggle_noninteractive_options() {
        let base = args(&[
            "--provider=kimi-coding",
            "--model=k3",
            "--thinking=high",
            "--no-skills",
        ]);
        assert!(validate_recipe("row", "kimi-coding", &base, &[]).is_ok());
        assert_eq!(
            parse_args(&base).unwrap().argv(),
            args(&[
                "--provider",
                "kimi-coding",
                "--model",
                "k3",
                "--thinking",
                "high",
                "--no-skills"
            ])
        );
        for extra in [
            args(&["--print", "hello", "--no-tools"]),
            args(&["--model=other"]),
            args(&["--no-skills"]),
            args(&["--no-skills=true"]),
            args(&["--extension=x"]),
        ] {
            let bad = [base.clone(), extra].concat();
            assert!(
                validate_recipe("row", "kimi-coding", &bad, &[]).is_err(),
                "{bad:?}"
            );
        }
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
}
