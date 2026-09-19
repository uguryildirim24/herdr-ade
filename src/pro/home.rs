//! The v2 shared Pro Codex home (SPEC-pro-bridge v2, Design "Codex home").
//!
//! `herdr-pro init` creates `<state dir>/codex-home`, writes its `config.toml`
//! and the two short instruction files, and `start` trusts the exact lane cwd
//! there before Codex launches. It is the only home a Pro lane uses. Rolf's
//! `~/.codex` is never written.
//!
//! The config deliberately adds no prompt text: memories, multi-agent, plugins,
//! apps, skills instructions and sub-agents are off, and
//! `model_instructions_file` replaces Codex's long base instructions with
//! [`INSTRUCTIONS`].

use std::path::Path;

use anyhow::{Context, Result};
use toml::Value;

use super::Layout;
use super::state;

/// The short base-instructions file `model_instructions_file` points at.
pub const INSTRUCTIONS: &str = "\
You are Pro, an answer-only worker on the codex-chatgpt-web bridge.
Read the packet and reply with the full answer in markdown.
You have no tools and you never ask for one.
";

/// The home's `AGENTS.md`: the packet and TURN contract, nothing of Rolf's.
pub const AGENTS_MD: &str = "\
# Pro

The plugin feeds you one packet and collects one answer.

## Packet

A turn starts with a `!cat` command whose output is the packet: the brief, then
each attached file under a `=== <absolute path> ===` header. Read all of it.

## TURN

The packet is followed by one line:

    TURN <id>: the packet above holds the brief and files. Reply with the full answer in markdown only.

Reply with the full answer in markdown and nothing else. Your whole reply is
written verbatim to an output file. Do not narrate, apologize, or ask questions.
";

/// Replace the owned config with the pinned keys, retaining only `[projects]`.
/// In particular, a stale MCP or plugin entry must not survive a later init.
fn apply_pins(table: &mut toml::Table, home: &Path) {
    let projects = table.remove("projects").filter(Value::is_table);
    table.clear();

    table.insert("approval_policy".into(), "never".into());
    table.insert("sandbox_mode".into(), "read-only".into());
    table.insert(
        "model_instructions_file".into(),
        Value::String(home.join("instructions.md").display().to_string()),
    );
    table.insert("include_permissions_instructions".into(), false.into());
    table.insert("include_apps_instructions".into(), false.into());
    table.insert(
        "include_collaboration_mode_instructions".into(),
        false.into(),
    );
    table.insert("include_environment_context".into(), false.into());

    let mut features = toml::Table::new();
    features.insert("memories".into(), false.into());
    features.insert("multi_agent".into(), false.into());
    features.insert("multi_agent_v2".into(), false.into());
    features.insert("plugins".into(), false.into());
    features.insert("apps".into(), false.into());
    table.insert("features".into(), Value::Table(features));

    let mut skills = toml::Table::new();
    skills.insert("include_instructions".into(), false.into());
    table.insert("skills".into(), Value::Table(skills));

    let mut agents = toml::Table::new();
    agents.insert("enabled".into(), false.into());
    agents.insert("max_depth".into(), 0.into());
    table.insert("agents".into(), Value::Table(agents));

    if let Some(projects) = projects {
        table.insert("projects".into(), projects);
    }
}

/// Read the home's config, or a fresh pinned table when it does not exist yet.
fn read_config(path: &Path, home: &Path) -> Result<toml::Table> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse::<toml::Table>()
            .with_context(|| format!("{} does not parse", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut table = toml::Table::new();
            apply_pins(&mut table, home);
            Ok(table)
        }
        Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
    }
}

fn render(table: &toml::Table) -> Result<String> {
    let body = toml::to_string(table).context("could not serialize the Pro home config")?;
    Ok(format!(
        "# Written by herdr-pro. The plugin owns this file.\n{body}"
    ))
}

/// `herdr-pro init`: create the home and write its config and instruction
/// files. Idempotent; an existing `[projects]` trust table is kept.
pub fn init(layout: &Layout) -> Result<()> {
    let home = layout.codex_home();
    std::fs::create_dir_all(&home)
        .with_context(|| format!("could not create {}", home.display()))?;

    let config = home.join("config.toml");
    let mut table = read_config(&config, &home)?;
    apply_pins(&mut table, &home);
    state::write_atomic(&config, &render(&table)?)?;
    state::write_atomic(&home.join("instructions.md"), INSTRUCTIONS)?;
    state::write_atomic(&home.join("AGENTS.md"), AGENTS_MD)?;
    Ok(())
}

/// Trust exactly `cwd` in the Pro home, so Codex never shows its trust prompt
/// for a lane. Codex trusts exact project paths only.
pub fn trust(layout: &Layout, cwd: &Path) -> Result<()> {
    let home = layout.codex_home();
    std::fs::create_dir_all(&home)
        .with_context(|| format!("could not create {}", home.display()))?;

    let config = home.join("config.toml");
    let mut table = read_config(&config, &home)?;
    apply_pins(&mut table, &home);
    let mut projects = match table.remove("projects") {
        Some(Value::Table(projects)) => projects,
        _ => toml::Table::new(),
    };
    let mut entry = toml::Table::new();
    entry.insert("trust_level".into(), "trusted".into());
    projects.insert(cwd.display().to_string(), Value::Table(entry));
    table.insert("projects".into(), Value::Table(projects));
    state::write_atomic(&config, &render(&table)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> (tempfile::TempDir, Layout) {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::for_test(dir.path().join("pro"));
        layout.ensure().unwrap();
        (dir, layout)
    }

    #[test]
    fn init_writes_the_pinned_config_and_the_two_files() {
        let (_dir, layout) = layout();
        init(&layout).unwrap();
        let home = layout.codex_home();
        let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
        let table: toml::Table = config.parse().unwrap();
        let expected = home.join("instructions.md").display().to_string();
        assert_eq!(
            table.get("model_instructions_file").and_then(Value::as_str),
            Some(expected.as_str())
        );
        assert_eq!(table["features"]["memories"].as_bool(), Some(false));
        assert_eq!(table["features"]["multi_agent"].as_bool(), Some(false));
        assert_eq!(
            table["skills"]["include_instructions"].as_bool(),
            Some(false)
        );
        assert_eq!(table["agents"]["enabled"].as_bool(), Some(false));
        assert_eq!(
            std::fs::read_to_string(home.join("instructions.md")).unwrap(),
            INSTRUCTIONS
        );
        assert_eq!(
            std::fs::read_to_string(home.join("AGENTS.md")).unwrap(),
            AGENTS_MD
        );
    }

    #[test]
    fn trust_adds_the_exact_cwd_and_init_keeps_it() {
        let (_dir, layout) = layout();
        let cwd = Path::new("/work/project");
        trust(&layout, cwd).unwrap();
        let home = layout.codex_home();
        let table: toml::Table = std::fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            table["projects"]["/work/project"]["trust_level"].as_str(),
            Some("trusted")
        );
        // A later init keeps trust, but removes everything else: this owned
        // home must never retain an MCP server from a manual or stale edit.
        let config = home.join("config.toml");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("\n[mcp_servers.stale]\ncommand = \"not-allowed\"\n");
        std::fs::write(&config, text).unwrap();
        init(&layout).unwrap();
        let table: toml::Table = std::fs::read_to_string(&config).unwrap().parse().unwrap();
        assert_eq!(
            table["projects"]["/work/project"]["trust_level"].as_str(),
            Some("trusted")
        );
        assert!(!table.contains_key("mcp_servers"));
        assert_eq!(table["features"]["memories"].as_bool(), Some(false));
    }
}
