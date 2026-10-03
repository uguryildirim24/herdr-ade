//! Install evidence contains counts and readability only, never project prose.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ProjectCheck {
    pub(crate) project: String,
    pub(crate) done: usize,
    pub(crate) total: usize,
    pub(crate) records_load: bool,
}

pub(crate) fn snapshot(root: &Path) -> Vec<ProjectCheck> {
    // A landing may run inside the ticker's cached pass. This check must read
    // disk, just as the newly installed process will.
    crate::events::set_cache(false);
    crate::review::set_cache(false);
    crate::thread::set_cache(false);
    crate::project::list_slugs(root)
        .into_iter()
        .map(|slug| {
            let mut check = ProjectCheck {
                project: slug.clone(),
                done: 0,
                total: 0,
                records_load: false,
            };
            if let Ok(project) = crate::project::Project::load(root, &slug) {
                let counts = crate::plan::counts(&project);
                if let Ok((done, total)) = counts {
                    check.done = done;
                    check.total = total;
                    check.records_load = crate::thread::list_with_errors(&project).1.is_empty()
                        && crate::task::list_with_errors(&project).1.is_empty()
                        && crate::review::list(&project).is_ok()
                        && crate::events::list_checked(&project).1
                        && project.records_load();
                }
            }
            check
        })
        .collect()
}

pub(crate) fn regression(before: &[ProjectCheck], after: &[ProjectCheck]) -> Option<String> {
    for old in before {
        let Some(new) = after.iter().find(|new| new.project == old.project) else {
            return Some(format!(
                "REGRESSION {}: records no longer load",
                old.project
            ));
        };
        if old.records_load && !new.records_load {
            return Some(format!(
                "REGRESSION {}: records no longer load",
                old.project
            ));
        }
        if new.done < old.done {
            return Some(format!(
                "REGRESSION {}: done {}→{}",
                old.project, old.done, new.done
            ));
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct InstallCheck {
    pub(crate) before: Vec<ProjectCheck>,
    pub(crate) after: Vec<ProjectCheck>,
    pub(crate) result: String,
}

impl InstallCheck {
    pub(super) fn record(&self, config: &Path) -> Result<()> {
        crate::project::write_json(&config.join("harness-install.json"), self)
    }

    pub(super) fn summary(&self) -> &'static str {
        if !self.after.iter().all(|check| check.records_load) {
            "plan counts checked; pre-existing unreadable records"
        } else if self.before == self.after {
            "plan counts unchanged; records load"
        } else {
            "plan counts checked (no drops); records load"
        }
    }
}
