use std::path::Path;

use anyhow::{Result, anyhow};

use crate::api::{
    DaemonEvent, DaemonGlobalEvent, DaemonGlobalEventKind, DaemonRunEvent, DaemonRunEventKind,
    DaemonServiceEvent, DaemonServiceEventKind, DaemonTaskEvent, DaemonTaskEventKind, RunResponse,
    ServiceResponse, TaskExecutionState, TaskExecutionSummary, TaskStatusResponse,
};
use crate::manifest::{RunLifecycle, ServiceState};
use crate::model::{GlobalRecord, RunRecord};
use crate::paths;
use crate::persistence::{PersistedGlobal, PersistedRun, PersistedService};
use crate::stores::DetachedTaskExecution;
use crate::util::{atomic_write, now_rfc3339};

use super::context::AppContext;

pub fn run_created_event(run: &RunRecord) -> DaemonEvent {
    DaemonEvent::Run(DaemonRunEvent {
        kind: DaemonRunEventKind::Created,
        run_id: run.run_id.as_str().to_string(),
        state: Some(run.state.clone()),
        stack: Some(run.stack.clone()),
        project_dir: Some(run.project_dir.to_string_lossy().to_string()),
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

pub fn run_removed_event(run_id: impl Into<String>) -> DaemonEvent {
    DaemonEvent::Run(DaemonRunEvent {
        kind: DaemonRunEventKind::Removed,
        run_id: run_id.into(),
        state: None,
        stack: None,
        project_dir: None,
    })
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

pub fn global_state_changed_event(key: &str, state: RunLifecycle) -> DaemonEvent {
    DaemonEvent::Global(DaemonGlobalEvent {
        kind: DaemonGlobalEventKind::StateChanged,
        key: key.to_string(),
        state,
    })
}

pub fn task_event(task: &DetachedTaskExecution, kind: DaemonTaskEventKind) -> DaemonEvent {
    DaemonEvent::Task(DaemonTaskEvent {
        kind,
        execution_id: task.execution_id.clone(),
        task: task.task.clone(),
        run_id: task.run_id.clone(),
        state: task.state.clone(),
        started_at: task.started_at.clone(),
        finished_at: task.finished_at.clone(),
        exit_code: task.exit_code,
        duration_ms: task.duration_ms,
    })
}

pub fn task_summary_from_history(
    execution: &crate::services::tasks::TaskExecution,
) -> TaskExecutionSummary {
    TaskExecutionSummary {
        task: execution.task.clone(),
        execution_id: None,
        state: if execution.exit_code == 0 {
            TaskExecutionState::Completed
        } else {
            TaskExecutionState::Failed
        },
        started_at: execution.started_at.clone(),
        finished_at: Some(execution.finished_at.clone()),
        exit_code: Some(execution.exit_code),
        duration_ms: Some(execution.duration_ms),
    }
}

pub fn task_summary_from_detached(task: &DetachedTaskExecution) -> TaskExecutionSummary {
    TaskExecutionSummary {
        task: task.task.clone(),
        execution_id: Some(task.execution_id.clone()),
        state: task.state.clone(),
        started_at: task.started_at.clone(),
        finished_at: task.finished_at.clone(),
        exit_code: task.exit_code,
        duration_ms: Some(task_duration_ms(task)),
    }
}

pub fn task_status_response(task: &DetachedTaskExecution) -> TaskStatusResponse {
    TaskStatusResponse {
        execution_id: task.execution_id.clone(),
        task: task.task.clone(),
        state: task.state.clone(),
        project_dir: task.project_dir.to_string_lossy().to_string(),
        run_id: task.run_id.clone(),
        started_at: task.started_at.clone(),
        finished_at: task.finished_at.clone(),
        exit_code: task.exit_code,
        duration_ms: task_duration_ms(task),
    }
}

pub fn task_duration_ms(task: &DetachedTaskExecution) -> u64 {
    task.duration_ms.unwrap_or_else(|| {
        task.started_at_instant
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    })
}

pub async fn persist_manifest(app: &AppContext, run_id: &str) -> Result<()> {
    let (manifest, path) = app
        .runs
        .with_run(run_id, |run| {
            let services = run
                .services
                .iter()
                .map(|(name, service)| {
                    (
                        name.clone(),
                        PersistedService {
                            port: service.launch.port,
                            url: service.launch.url.clone(),
                            state: service.runtime.state.clone(),
                            watch_hash: Some(service.launch.watch_hash.clone()),
                            last_failure: service.runtime.last_failure.clone(),
                            last_started_at: service.runtime.last_started_at.clone(),
                            watch_paused: service.runtime.watch_paused,
                        },
                    )
                })
                .collect();
            let path = paths::run_manifest_path(&run.run_id).unwrap();
            (
                PersistedRun {
                    run_id: run.run_id.as_str().to_string(),
                    project_dir: run.project_dir.to_string_lossy().to_string(),
                    config_dir: run.config_dir.to_string_lossy().to_string(),
                    manifest_path: path.to_string_lossy().to_string(),
                    stack: run.stack.clone(),
                    services,
                    env: run.base_env.clone(),
                    state: run.state.clone(),
                    created_at: run.created_at.clone(),
                    stopped_at: run.stopped_at.clone(),
                },
                path,
            )
        })
        .await
        .map_err(|err| anyhow!(err))?;

    manifest.write_to_path(&path)?;
    write_daemon_state(app).await?;
    Ok(())
}

pub async fn run_response(app: &AppContext, run_id: &str) -> Result<RunResponse> {
    app.runs
        .with_run(run_id, run_response_from_record)
        .await
        .map_err(|err| anyhow!(err))
}

pub async fn persist_global_manifest(app: &AppContext, key: &str) -> Result<()> {
    let (manifest, path) = app
        .globals
        .with_global_mut(key, |global| {
            let path = paths::global_manifest_path(&global.project_dir, &global.name).unwrap();
            (persisted_global_from_record(global, &path), path)
        })
        .await
        .map_err(|err| anyhow!(err))?;

    manifest.write_to_path(&path)?;
    Ok(())
}

pub fn persisted_global_from_record(global: &GlobalRecord, path: &Path) -> PersistedGlobal {
    PersistedGlobal {
        key: global.key.clone(),
        name: global.name.clone(),
        project_dir: global.project_dir.to_string_lossy().to_string(),
        config_path: global.config_path.to_string_lossy().to_string(),
        manifest_path: path.to_string_lossy().to_string(),
        service: PersistedService {
            port: global.service.launch.port,
            url: global.service.launch.url.clone(),
            state: global.service.runtime.state.clone(),
            watch_hash: Some(global.service.launch.watch_hash.clone()),
            last_failure: global.service.runtime.last_failure.clone(),
            last_started_at: global.service.runtime.last_started_at.clone(),
            watch_paused: global.service.runtime.watch_paused,
        },
        env: global.service.launch.env.clone(),
        state: global.state.clone(),
        created_at: global.created_at.clone(),
        stopped_at: global.stopped_at.clone(),
    }
}

pub fn run_response_from_record(run: &RunRecord) -> RunResponse {
    let manifest_path = paths::run_manifest_path(&run.run_id).unwrap();
    let services = run
        .services
        .iter()
        .map(|(name, service)| {
            (
                name.clone(),
                ServiceResponse {
                    port: service.launch.port,
                    url: service.launch.url.clone(),
                    state: service.runtime.state.clone(),
                },
            )
        })
        .collect();

    RunResponse {
        run_id: run.run_id.as_str().to_string(),
        project_dir: run.project_dir.to_string_lossy().to_string(),
        stack: run.stack.clone(),
        manifest_path: manifest_path.to_string_lossy().to_string(),
        services,
        env: run.base_env.clone(),
        state: run.state.clone(),
        created_at: run.created_at.clone(),
        stopped_at: run.stopped_at.clone(),
    }
}

pub async fn write_daemon_state(app: &AppContext) -> Result<()> {
    let runs = app
        .runs
        .with_runs(|runs| runs.keys().cloned().collect::<Vec<_>>())
        .await;
    let state_file = serde_json::json!({
        "runs": runs,
        "updated_at": now_rfc3339(),
    });
    let data = serde_json::to_vec_pretty(&state_file)?;
    atomic_write(&paths::daemon_state_path()?, &data)?;
    Ok(())
}

pub fn same_project_dir(run_project_dir: &Path, project_dir: &Path) -> bool {
    if run_project_dir == project_dir {
        return true;
    }
    let run_canon =
        std::fs::canonicalize(run_project_dir).unwrap_or_else(|_| run_project_dir.to_path_buf());
    let project_canon =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    run_canon == project_canon
}

pub async fn find_latest_active_run_for_project(
    app: &AppContext,
    project_dir: &Path,
) -> Result<Option<String>> {
    let mut candidates = app
        .runs
        .with_runs(|runs| {
            runs.values()
                .filter(|run| same_project_dir(&run.project_dir, project_dir))
                .filter(|run| run.state != RunLifecycle::Stopped && run.stopped_at.is_none())
                .map(|run| (run.created_at.clone(), run.run_id.as_str().to_string()))
                .collect::<Vec<_>>()
        })
        .await;
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(candidates.into_iter().next().map(|(_, run_id)| run_id))
}

pub async fn find_latest_run_for_project_stack(
    app: &AppContext,
    project_dir: &Path,
    stack: &str,
) -> Result<Option<String>> {
    let mut candidates = app
        .runs
        .with_runs(|runs| {
            runs.values()
                .filter(|run| run.stack == stack)
                .filter(|run| same_project_dir(&run.project_dir, project_dir))
                .filter(|run| run.state != RunLifecycle::Stopped && run.stopped_at.is_none())
                .map(|run| (run.created_at.clone(), run.run_id.as_str().to_string()))
                .collect::<Vec<_>>()
        })
        .await;
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(candidates.into_iter().next().map(|(_, run_id)| run_id))
}

pub fn port_owner(run_id: &str, service: &str) -> String {
    format!("run:{run_id}:{service}")
}

pub fn global_port_owner(key: &str, service: &str) -> String {
    format!("global:{key}:{service}")
}

pub async fn sync_port_reservations_from_disk(app: &AppContext) -> Result<()> {
    let runs_dir = paths::runs_dir()?;
    if runs_dir.exists() {
        for entry in std::fs::read_dir(&runs_dir)? {
            let entry = entry?;
            let manifest_path = entry.path().join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            let manifest = PersistedRun::load_from_path(&manifest_path)?;
            if manifest.state == RunLifecycle::Stopped || manifest.stopped_at.is_some() {
                continue;
            }
            for (service, record) in manifest.services {
                if let Some(port) = record.port {
                    crate::port::reserve_port(port, &port_owner(&manifest.run_id, &service))?;
                }
            }
        }
    }

    let globals_root = paths::globals_root()?;
    if globals_root.exists() {
        for entry in std::fs::read_dir(&globals_root)? {
            let entry = entry?;
            let manifest_path = entry.path().join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            let manifest = PersistedGlobal::load_from_path(&manifest_path)?;
            if manifest.state == RunLifecycle::Stopped || manifest.stopped_at.is_some() {
                continue;
            }
            if let Some(port) = manifest.service.port {
                crate::port::reserve_port(port, &global_port_owner(&manifest.key, &manifest.name))?;
            }
        }
    }

    let _ = app;
    Ok(())
}

pub fn release_service_port(run_id: &str, service: &str, port: Option<u16>) -> Result<()> {
    if let Some(port) = port {
        crate::port::release_port(port, &port_owner(run_id, service))?;
    }
    Ok(())
}
