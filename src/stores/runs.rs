use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use tokio::sync::Mutex;

use crate::api::{DaemonEvent, DaemonRunEvent, DaemonRunEventKind, DaemonServiceEvent, DaemonServiceEventKind};
use crate::manifest::{RunLifecycle, ServiceState};
use crate::model::{RunRecord, ServiceRecord};

pub struct RunStore {
    inner: Mutex<BTreeMap<String, RunRecord>>,
}

impl RunStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn from_runs(runs: BTreeMap<String, RunRecord>) -> Self {
        Self {
            inner: Mutex::new(runs),
        }
    }

    pub async fn create_run(&self, run: RunRecord) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let run_id = run.run_id.as_str().to_string();
        if guard.contains_key(&run_id) {
            return Err(anyhow!("run {run_id} already exists"));
        }
        guard.insert(run_id, run);
        Ok(())
    }

    pub async fn get_run(&self, run_id: &str) -> Option<RunRecord> {
        let guard = self.inner.lock().await;
        guard.get(run_id).cloned()
    }

    pub async fn list_runs(&self) -> Vec<RunRecord> {
        let guard = self.inner.lock().await;
        guard.values().cloned().collect()
    }

    pub async fn remove_run(&self, run_id: &str) -> Option<RunRecord> {
        let mut guard = self.inner.lock().await;
        guard.remove(run_id)
    }

    pub async fn contains_run(&self, run_id: &str) -> bool {
        let guard = self.inner.lock().await;
        guard.contains_key(run_id)
    }

    pub async fn with_run_mut<F, R>(&self, run_id: &str, f: F) -> Result<R>
    where
        F: FnOnce(&mut RunRecord) -> R,
    {
        let mut guard = self.inner.lock().await;
        let run = guard
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run {run_id} not found"))?;
        Ok(f(run))
    }

    pub async fn with_run<F, R>(&self, run_id: &str, f: F) -> Result<R>
    where
        F: FnOnce(&RunRecord) -> R,
    {
        let guard = self.inner.lock().await;
        let run = guard
            .get(run_id)
            .ok_or_else(|| anyhow!("run {run_id} not found"))?;
        Ok(f(run))
    }

    pub async fn with_runs_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut BTreeMap<String, RunRecord>) -> R,
    {
        let mut guard = self.inner.lock().await;
        f(&mut guard)
    }

    pub async fn with_runs<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&BTreeMap<String, RunRecord>) -> R,
    {
        let guard = self.inner.lock().await;
        f(&guard)
    }
}

impl Default for RunStore {
    fn default() -> Self {
        Self::new()
    }
}

pub fn set_service_state(
    run_id: &str,
    service: &str,
    record: &mut ServiceRecord,
    state: ServiceState,
) -> Option<DaemonEvent> {
    if record.runtime.state == state {
        return None;
    }
    record.set_state(state.clone());
    Some(service_state_changed_event(run_id, service, state))
}

pub fn recompute_run_state(run: &mut RunRecord) -> Option<DaemonEvent> {
    if matches!(run.state, RunLifecycle::Stopped) {
        return None;
    }

    let previous = run.state.clone();
    let mut all_ready = true;
    let mut any_degraded = false;
    for service in run.services.values() {
        match service.runtime.state {
            ServiceState::Ready => {}
            ServiceState::Starting => all_ready = false,
            ServiceState::Degraded | ServiceState::Failed => {
                any_degraded = true;
                all_ready = false;
            }
            ServiceState::Stopped => all_ready = false,
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

pub fn service_state_changed_event(
    run_id: &str,
    service: &str,
    state: ServiceState,
) -> DaemonEvent {
    DaemonEvent::Service(DaemonServiceEvent {
        kind: DaemonServiceEventKind::StateChanged,
        run_id: run_id.to_string(),
        service: service.to_string(),
        state,
    })
}

pub fn run_state_changed_event(run: &RunRecord) -> DaemonEvent {
    DaemonEvent::Run(DaemonRunEvent {
        kind: DaemonRunEventKind::StateChanged,
        run_id: run.run_id.as_str().to_string(),
        state: Some(run.state.clone()),
        stack: None,
        project_dir: None,
    })
}
