use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::{ServiceConfig, TaskConfig};
use crate::manifest::RunLifecycle;

use super::ServiceRecord;

#[derive(Clone, Debug)]
pub struct GlobalRecord {
    pub key: String,
    pub name: String,
    pub project_dir: PathBuf,
    pub config_path: PathBuf,
    pub service_config: ServiceConfig,
    pub tasks_map: BTreeMap<String, TaskConfig>,
    pub service: ServiceRecord,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}
