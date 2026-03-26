use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{RunId, ServiceRecord};
use crate::manifest::RunLifecycle;

/// Represents a run's state in memory (renamed from RunState in daemon.rs)
#[derive(Clone, Debug)]
pub struct RunRecord {
    pub run_id: RunId,
    pub stack: String,
    pub project_dir: PathBuf,
    pub config_dir: PathBuf,
    pub base_env: BTreeMap<String, String>,
    pub services: BTreeMap<String, ServiceRecord>,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

impl RunRecord {
    pub fn new(
        run_id: RunId,
        stack: String,
        project_dir: PathBuf,
        config_dir: PathBuf,
        base_env: BTreeMap<String, String>,
    ) -> Self {
        Self {
            run_id,
            stack,
            project_dir,
            config_dir,
            base_env,
            services: BTreeMap::new(),
            state: RunLifecycle::Starting,
            created_at: crate::util::now_rfc3339(),
            stopped_at: None,
        }
    }

    /// Check if this run is stopped
    pub fn is_stopped(&self) -> bool {
        matches!(self.state, RunLifecycle::Stopped)
    }

    /// Check if this run is running (ready or degraded but not stopped)
    pub fn is_active(&self) -> bool {
        !self.is_stopped()
    }

    /// Get service by name
    pub fn get_service(&self, name: &str) -> Option<&ServiceRecord> {
        self.services.get(name)
    }

    /// Get mutable service by name
    pub fn get_service_mut(&mut self, name: &str) -> Option<&mut ServiceRecord> {
        self.services.get_mut(name)
    }

    /// Add or update a service
    pub fn insert_service(&mut self, name: String, service: ServiceRecord) {
        self.services.insert(name, service);
    }

    /// Remove a service
    pub fn remove_service(&mut self, name: &str) -> Option<ServiceRecord> {
        self.services.remove(name)
    }

    /// Mark the run as stopped
    pub fn mark_stopped(&mut self) {
        self.state = RunLifecycle::Stopped;
        self.stopped_at = Some(crate::util::now_rfc3339());
    }
}
