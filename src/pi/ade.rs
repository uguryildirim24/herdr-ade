//! ADE readiness uses the same pi checks as `herdr-pi`.

use std::fmt;
use std::path::Path;

use crate::pi::Layout;

pub(crate) fn layout(root: &Path) -> Layout {
    Layout {
        root: root.join("pi"),
    }
}

#[derive(Debug)]
pub(crate) struct ReadinessError {
    pub(crate) class: crate::contracts::FailureClass,
    pub(crate) message: String,
}

impl fmt::Display for ReadinessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for ReadinessError {}

pub(crate) fn failure_class(error: &anyhow::Error) -> crate::contracts::FailureClass {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ReadinessError>())
        .map_or(crate::contracts::FailureClass::Unknown, |error| error.class)
}
