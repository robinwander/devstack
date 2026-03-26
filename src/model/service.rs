use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

use crate::manifest::ServiceState;
use crate::services::readiness::ReadinessSpec;

/// Immutable service specification from config
#[derive(Clone, Debug)]
pub struct ServiceSpec {
    pub name: String,
    pub deps: Vec<String>,
    pub readiness: ReadinessSpec,
    pub auto_restart: bool,
    pub watch_patterns: Vec<String>,
    pub ignore_patterns: Vec<String>,
}

/// Computed launch plan for a service (immutable after preparation)
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

/// Mutable runtime state that changes during execution
#[derive(Clone, Debug)]
pub struct ServiceRuntimeState {
    pub state: ServiceState,
    pub last_failure: Option<String>,
    pub last_started_at: Option<String>,
    pub watch_paused: bool,
}

/// Background task handles (not cloneable)
pub struct ServiceHandles {
    pub health: Option<HealthHandle>,
    pub watch: Option<ServiceWatchHandle>,
}

/// Complete service record combining all aspects
#[derive(Clone, Debug)]
pub struct ServiceRecord {
    pub spec: ServiceSpec,
    pub launch: ServiceLaunchPlan,
    pub runtime: ServiceRuntimeState,
    // Note: handles are stored separately as they're not Clone
}

/// Per-service health monitor handle (extracted from daemon.rs)
#[derive(Clone)]
pub struct HealthHandle {
    pub stop_flag: Arc<AtomicBool>,
    pub stats: Arc<std::sync::Mutex<HealthSnapshot>>,
}

/// Health monitor statistics
#[derive(Clone, Default)]
pub struct HealthSnapshot {
    pub passes: u64,
    pub failures: u64,
    pub consecutive_failures: u32,
    pub last_check_at: Option<String>,
    pub last_ok: Option<bool>,
}

/// Service watch handle 
#[derive(Clone)]
pub struct ServiceWatchHandle {
    pub stop_flag: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
}

impl ServiceRecord {
    /// Create a new service record
    pub fn new(spec: ServiceSpec, launch: ServiceLaunchPlan) -> Self {
        Self {
            spec,
            launch,
            runtime: ServiceRuntimeState {
                state: ServiceState::Starting,
                last_failure: None,
                last_started_at: None,
                watch_paused: false,
            },
        }
    }

    /// Check if service is ready
    pub fn is_ready(&self) -> bool {
        matches!(self.runtime.state, ServiceState::Ready)
    }

    /// Check if service is stopped
    pub fn is_stopped(&self) -> bool {
        matches!(self.runtime.state, ServiceState::Stopped)
    }

    /// Check if service is failed
    pub fn is_failed(&self) -> bool {
        matches!(self.runtime.state, ServiceState::Failed)
    }

    /// Update the service state
    pub fn set_state(&mut self, state: ServiceState) {
        if matches!(state, ServiceState::Ready) {
            self.runtime.last_started_at = Some(crate::util::now_rfc3339());
        }
        self.runtime.state = state;
    }

    /// Set failure reason
    pub fn set_failure(&mut self, reason: String) {
        self.runtime.state = ServiceState::Failed;
        self.runtime.last_failure = Some(reason);
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