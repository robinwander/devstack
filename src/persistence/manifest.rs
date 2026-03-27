use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{RunLifecycle, ServiceState};
use crate::util::atomic_write;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedService {
    pub port: Option<u16>,
    pub url: Option<String>,
    pub state: ServiceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_started_at: Option<String>,
    #[serde(default)]
    pub watch_paused: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedRun {
    pub run_id: String,
    pub project_dir: String,
    pub config_dir: String,
    pub manifest_path: String,
    pub stack: String,
    pub services: BTreeMap<String, PersistedService>,
    pub env: BTreeMap<String, String>,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

impl PersistedRun {
    pub fn write_to_path(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).context("serialize run manifest")?;
        atomic_write(path, &json)
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read run manifest {path:?}"))?;
        let manifest = serde_json::from_slice(&data).context("parse run manifest")?;
        Ok(manifest)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedGlobal {
    pub key: String,
    pub name: String,
    pub project_dir: String,
    pub config_path: String,
    pub manifest_path: String,
    pub service: PersistedService,
    pub env: BTreeMap<String, String>,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

impl PersistedGlobal {
    pub fn write_to_path(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).context("serialize global manifest")?;
        atomic_write(path, &json)
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read global manifest {path:?}"))?;
        let manifest = serde_json::from_slice(&data).context("parse global manifest")?;
        Ok(manifest)
    }
}
