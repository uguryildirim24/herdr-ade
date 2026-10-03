//! The single read and parse boundary for ADE's `config.toml`.
//!
//! A missing file is an empty document. Every other read error is reported.
//! Callers decode only the section or small view they own.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

#[derive(Debug, Clone)]
pub(crate) struct Document {
    path: PathBuf,
    table: toml::Table,
}

impl Document {
    pub(crate) fn read(config_dir: &Path) -> Result<Self> {
        let path = config_dir.join("config.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(error).with_context(|| format!("could not read {}", path.display()));
            }
        };
        let table = text
            .parse::<toml::Table>()
            .with_context(|| format!("{} does not parse", path.display()))?;
        Ok(Self { path, table })
    }

    pub(crate) fn decode<T: DeserializeOwned>(&self) -> Result<T> {
        toml::Value::Table(self.table.clone())
            .try_into()
            .with_context(|| format!("{} does not parse", self.path.display()))
    }

    pub(crate) fn section<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T> {
        match self.table.get(name) {
            Some(value) => value
                .clone()
                .try_into()
                .with_context(|| format!("{} [{name}] does not parse", self.path.display())),
            None => Ok(T::default()),
        }
    }

    pub(crate) fn value(&self, name: &str) -> Option<&toml::Value> {
        self.table.get(name)
    }
}

/// `$XDG_CONFIG_HOME/herdr-ade`, else `~/.config/herdr-ade`.
pub(crate) fn dir(home: &Path, xdg_config_home: Option<&Path>) -> PathBuf {
    xdg_config_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".config"))
        .join("herdr-ade")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absence_is_empty_but_a_read_failure_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(Document::read(&missing).unwrap().value("root").is_none());

        std::fs::create_dir_all(dir.path().join("config.toml")).unwrap();
        let error = Document::read(dir.path()).unwrap_err().to_string();
        assert!(!error.is_empty());
    }
}
