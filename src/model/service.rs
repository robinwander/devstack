use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

use crate::manifest::ServiceState;
use crate::services::readiness::ReadinessSpec;

#[derive(Clone, Debug)]
pub struct ServiceSpec {
    pub name: String,
    pub deps: Vec<String>,
    pub readiness: ReadinessSpec,
    pub auto_restart: bool,
    pub watch_patterns: Vec<String>,
    pub ignore_patterns: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ServiceLaunchPlan {
    pub unit_name: String,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub cmd: String,
    pub log_path: PathBuf,
    pub port: Option<u16>,
    pub scheme: String,
    pub url: Option<String>,
    pub watch_hash: String,
    pub watch_fingerprint: Vec<u8>,
    pub watch_extra_files: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct ServiceRuntimeState {
    pub state: ServiceState,
    pub last_failure: Option<String>,
    pub last_started_at: Option<String>,
    pub watch_paused: bool,
}

#[derive(Clone)]
pub struct HealthHandle {
    pub stop_flag: Arc<AtomicBool>,
    pub stats: Arc<std::sync::Mutex<HealthSnapshot>>,
}

impl std::fmt::Debug for HealthHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HealthHandle").finish_non_exhaustive()
    }
}

#[derive(Clone, Default, Debug)]
pub struct HealthSnapshot {
    pub passes: u64,
    pub failures: u64,
    pub consecutive_failures: u32,
    pub last_check_at: Option<String>,
    pub last_ok: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct ServiceWatchHandle {
    pub stop_flag: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
}

#[derive(Clone, Debug, Default)]
pub struct ServiceHandles {
    pub health: Option<HealthHandle>,
    pub watch: Option<ServiceWatchHandle>,
}

#[derive(Clone, Debug)]
pub struct ServiceRecord {
    pub spec: ServiceSpec,
    pub launch: ServiceLaunchPlan,
    pub runtime: ServiceRuntimeState,
    pub handles: ServiceHandles,
}

impl ServiceRecord {
    pub fn new(spec: ServiceSpec, launch: ServiceLaunchPlan) -> Self {
        Self {
            spec,
            launch,
            runtime: ServiceRuntimeState::default(),
            handles: ServiceHandles::default(),
        }
    }

    pub fn set_state(&mut self, state: ServiceState) {
        if matches!(state, ServiceState::Ready) {
            self.runtime.last_started_at = Some(crate::util::now_rfc3339());
        }
        self.runtime.state = state;
    }

    pub fn set_failure(&mut self, reason: String) {
        self.runtime.state = ServiceState::Failed;
        self.runtime.last_failure = Some(reason);
    }

    pub fn stop_health_monitor(&mut self) {
        if let Some(health) = self.handles.health.take() {
            health.stop_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    pub fn stop_watch(&mut self) {
        if let Some(handle) = self.handles.watch.take() {
            handle.stop_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    pub fn watch_active(&self) -> bool {
        self.spec.auto_restart && self.handles.watch.is_some() && !self.runtime.watch_paused
    }
}

impl Default for ServiceRuntimeState {
    fn default() -> Self {
        Self {
            state: ServiceState::Starting,
            last_failure: None,
            last_started_at: None,
            watch_paused: false,
        }
    }
}
