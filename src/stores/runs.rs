use std::collections::BTreeMap;
use anyhow::{anyhow, Result};
use tokio::sync::Mutex;

use crate::api::DaemonEvent;
use crate::manifest::{RunLifecycle, ServiceState};
use crate::model::{RunRecord, ServiceRecord};

/// Store for managing run state with intention-revealing methods
pub struct RunStore {
    inner: Mutex<BTreeMap<String, RunRecord>>,
}

impl RunStore {
    /// Create a new run store
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    /// Create a run store from existing data
    pub fn from_runs(runs: BTreeMap<String, RunRecord>) -> Self {
        Self {
            inner: Mutex::new(runs),
        }
    }

    /// Create a new run
    pub async fn create_run(&self, run: RunRecord) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let run_id = run.run_id.as_str().to_string();
        if guard.contains_key(&run_id) {
            return Err(anyhow!("run {} already exists", run_id));
        }
        guard.insert(run_id, run);
        Ok(())
    }

    /// Get a run by ID
    pub async fn get_run(&self, run_id: &str) -> Option<RunRecord> {
        let guard = self.inner.lock().await;
        guard.get(run_id).cloned()
    }

    /// List all runs
    pub async fn list_runs(&self) -> Vec<RunRecord> {
        let guard = self.inner.lock().await;
        guard.values().cloned().collect()
    }

    /// Remove a run
    pub async fn remove_run(&self, run_id: &str) -> Option<RunRecord> {
        let mut guard = self.inner.lock().await;
        guard.remove(run_id)
    }

    /// Check if a run exists
    pub async fn contains_run(&self, run_id: &str) -> bool {
        let guard = self.inner.lock().await;
        guard.contains_key(run_id)
    }

    /// Insert a service into a run
    pub async fn insert_service(
        &self,
        run_id: &str,
        service: String,
        record: ServiceRecord,
    ) -> Result<Vec<DaemonEvent>> {
        let mut guard = self.inner.lock().await;
        let run = guard
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run {} not found", run_id))?;
            
        run.insert_service(service.clone(), record);
        
        // Generate event
        let event = service_state_changed_event(run_id, &service, ServiceState::Starting);
        Ok(vec![event])
    }

    /// Mark a service as starting
    pub async fn mark_service_starting(
        &self,
        run_id: &str,
        service: &str,
    ) -> Result<Vec<DaemonEvent>> {
        let mut events = Vec::new();
        let mut guard = self.inner.lock().await;
        let run = guard
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run {} not found", run_id))?;

        if let Some(svc) = run.get_service_mut(service) {
            if let Some(event) = set_service_state(run_id, service, svc, ServiceState::Starting) {
                events.push(event);
            }
        }

        if let Some(event) = recompute_run_state(run) {
            events.push(event);
        }
        
        Ok(events)
    }

    /// Mark a service as ready
    pub async fn mark_service_ready(
        &self,
        run_id: &str,
        service: &str,
    ) -> Result<Vec<DaemonEvent>> {
        let mut events = Vec::new();
        let mut guard = self.inner.lock().await;
        let run = guard
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run {} not found", run_id))?;

        if let Some(svc) = run.get_service_mut(service) {
            if let Some(event) = set_service_state(run_id, service, svc, ServiceState::Ready) {
                events.push(event);
            }
        }

        if let Some(event) = recompute_run_state(run) {
            events.push(event);
        }
        
        Ok(events)
    }

    /// Mark a service as failed
    pub async fn mark_service_failed(
        &self,
        run_id: &str,
        service: &str,
        reason: String,
    ) -> Result<Vec<DaemonEvent>> {
        let mut events = Vec::new();
        let mut guard = self.inner.lock().await;
        let run = guard
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run {} not found", run_id))?;

        if let Some(svc) = run.get_service_mut(service) {
            svc.set_failure(reason);
            let event = service_state_changed_event(run_id, service, ServiceState::Failed);
            events.push(event);
        }

        if let Some(event) = recompute_run_state(run) {
            events.push(event);
        }
        
        Ok(events)
    }

    /// Mark a run as stopped
    pub async fn mark_run_stopped(&self, run_id: &str) -> Result<Vec<DaemonEvent>> {
        let mut guard = self.inner.lock().await;
        let run = guard
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run {} not found", run_id))?;

        run.mark_stopped();
        let event = run_state_changed_event(run);
        Ok(vec![event])
    }

    /// Execute a closure with mutable access to a run (for complex operations)
    pub async fn with_run_mut<F, R>(&self, run_id: &str, f: F) -> Result<R>
    where
        F: FnOnce(&mut RunRecord) -> R,
    {
        let mut guard = self.inner.lock().await;
        let run = guard
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run {} not found", run_id))?;
        Ok(f(run))
    }

    /// Execute a closure with read access to a run
    pub async fn with_run<F, R>(&self, run_id: &str, f: F) -> Result<R>
    where
        F: FnOnce(&RunRecord) -> R,
    {
        let guard = self.inner.lock().await;
        let run = guard
            .get(run_id)
            .ok_or_else(|| anyhow!("run {} not found", run_id))?;
        Ok(f(run))
    }
}

impl Default for RunStore {
    fn default() -> Self {
        Self::new()
    }
}

// Helper functions extracted from daemon.rs

fn set_service_state(
    run_id: &str,
    service: &str,
    svc: &mut ServiceRecord,
    state: ServiceState,
) -> Option<DaemonEvent> {
    if svc.runtime.state == state {
        return None;
    }
    svc.set_state(state.clone());
    Some(service_state_changed_event(run_id, service, state))
}

fn recompute_run_state(run: &mut RunRecord) -> Option<DaemonEvent> {
    if matches!(run.state, RunLifecycle::Stopped) {
        return None;
    }
    
    let previous = run.state.clone();
    let mut all_ready = true;
    let mut any_degraded = false;
    
    for svc in run.services.values() {
        match svc.runtime.state {
            ServiceState::Ready => {}
            ServiceState::Starting => {
                all_ready = false;
            }
            ServiceState::Degraded | ServiceState::Failed => {
                any_degraded = true;
                all_ready = false;
            }
            ServiceState::Stopped => {
                all_ready = false;
            }
        }
    }
    
    run.state = if any_degraded {
        RunLifecycle::Degraded
    } else if all_ready {
        RunLifecycle::Running
    } else {
        RunLifecycle::Starting
    };
    
    (run.state != previous).then(|| run_state_changed_event(run))
}

fn service_state_changed_event(run_id: &str, service: &str, state: ServiceState) -> DaemonEvent {
    DaemonEvent::Service(crate::api::DaemonServiceEvent {
        kind: crate::api::DaemonServiceEventKind::StateChanged,
        run_id: run_id.to_string(),
        service: service.to_string(),
        state,
    })
}

fn run_state_changed_event(run: &RunRecord) -> DaemonEvent {
    DaemonEvent::Run(crate::api::DaemonRunEvent {
        kind: crate::api::DaemonRunEventKind::StateChanged,
        run_id: run.run_id.as_str().to_string(),
        state: Some(run.state.clone()),
        stack: None,
        project_dir: None,
    })
}