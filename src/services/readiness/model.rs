use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::systemd::SystemdManager;

#[derive(Clone)]
pub struct ReadinessContext {
    pub port: Option<u16>,
    pub scheme: String,
    pub log_path: PathBuf,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub unit_name: Option<String>,
    pub systemd: Option<Arc<dyn SystemdManager>>,
}
