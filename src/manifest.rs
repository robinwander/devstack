use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::util::atomic_write;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Starting,
    Ready,
    Degraded,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunLifecycle {
    Starting,
    Running,
    Degraded,
    Stopped,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ServiceManifest {
    pub port: Option<u16>,
    pub url: Option<String>,
    pub state: ServiceState,
    #[serde(default, skip_serializing)]
    pub watch_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct RunManifest {
    pub run_id: String,
    pub project_dir: String,
    pub stack: String,
    pub manifest_path: String,
    pub services: BTreeMap<String, ServiceManifest>,
    pub env: BTreeMap<String, String>,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

impl RunManifest {
    pub fn write_to_path(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).context("serialize manifest")?;
        atomic_write(path, &json)
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read manifest {path:?}"))?;
        let manifest = serde_json::from_slice(&data).context("parse manifest")?;
        Ok(manifest)
    }
}
