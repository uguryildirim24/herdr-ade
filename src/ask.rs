//! Read-only decoding of historical asks, answers and withdrawals.
//! Old `ask:<id>@<revision>` authority still resolves; nothing writes these records.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::Ask;
use crate::project::Project;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) id: String,
    pub(crate) revision: u32,
    pub(crate) choice: u32,
    pub(crate) text: String,
    pub(crate) not_understood: bool,
    pub(crate) answered: String,
    pub(crate) by: String,
}

fn ask_dir(project: &Project, id: &str) -> Result<PathBuf> {
    validate_ask_id(id)?;
    Ok(project.record_dir("asks").join(id))
}

pub(crate) fn validate_ask_id(id: &str) -> Result<()> {
    let digits = id.strip_prefix("a-").unwrap_or("");
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        bail!("ask_unknown: `{id}` is not an ask id (expected the form a-1)");
    }
    Ok(())
}

// Kept for historical records, not a live workflow.
pub(crate) fn load_revision(project: &Project, id: &str, revision: u32) -> Result<Option<Ask>> {
    let path = ask_dir(project, id)?.join(format!("r{revision}.toml"));
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(toml::from_str(&text).with_context(|| {
            format!("ask record {} does not parse", path.display())
        })?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn latest_revision(project: &Project, id: &str) -> u32 {
    let Ok(dir) = ask_dir(project, id) else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| {
            n.strip_prefix('r')?
                .strip_suffix(".toml")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0)
}

pub(crate) fn answer_of(project: &Project, id: &str, revision: u32) -> Option<Answer> {
    let path = ask_dir(project, id)
        .ok()?
        .join(format!("r{revision}.answer.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

pub(crate) fn answered_revision(project: &Project, id: &str, revision: u32) -> Result<Answer> {
    let ask = load_revision(project, id, revision)?.context("ask revision is missing")?;
    if revision == 0 || latest_revision(project, id) != revision {
        bail!("ask revision is not current");
    }
    let answer = answer_of(project, id, revision).context("ask is not answered")?;
    if ask.id != id || ask.revision != revision || answer.id != id || answer.revision != revision {
        bail!("ask revision and answer do not match their authority reference");
    }
    Ok(answer)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Withdrawal {
    pub(crate) id: String,
    pub(crate) revision: u32,
    pub(crate) reason: String,
    pub(crate) by: String,
    pub(crate) at: String,
}

// Withdrawals remain readable even though no live projection consumes them.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn withdrawal_of(project: &Project, id: &str, revision: u32) -> Option<Withdrawal> {
    let path = ask_dir(project, id)
        .ok()?
        .join(format!("r{revision}.withdrawn.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}
