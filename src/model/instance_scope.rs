use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use super::RunId;

/// Represents the scope of a devstack instance - either a run or a global service
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InstanceScope {
    /// A regular run with run_id and stack name
    Run { run_id: RunId, stack: String },
    /// A global service with key, project directory, and service name
    Global {
        key: String,
        project_dir: PathBuf,
        name: String,
    },
}

impl InstanceScope {
    pub fn run(run_id: impl Into<RunId>, stack: impl Into<String>) -> Self {
        Self::Run {
            run_id: run_id.into(),
            stack: stack.into(),
        }
    }

    pub fn global(
        key: impl Into<String>,
        project_dir: impl Into<PathBuf>,
        name: impl Into<String>,
    ) -> Self {
        Self::Global {
            key: key.into(),
            project_dir: project_dir.into(),
            name: name.into(),
        }
    }

    /// Returns true if this is a run scope
    pub fn is_run(&self) -> bool {
        matches!(self, Self::Run { .. })
    }

    /// Returns true if this is a global scope
    pub fn is_global(&self) -> bool {
        matches!(self, Self::Global { .. })
    }

    /// Get the run_id if this is a run scope
    pub fn run_id(&self) -> Option<&RunId> {
        match self {
            Self::Run { run_id, .. } => Some(run_id),
            Self::Global { .. } => None,
        }
    }

    /// Get the project directory
    pub fn project_dir(&self) -> Option<&PathBuf> {
        match self {
            Self::Run { .. } => None, // Run scopes don't store project_dir in this type
            Self::Global { project_dir, .. } => Some(project_dir),
        }
    }
}