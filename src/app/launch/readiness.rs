use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::{Result, anyhow};

use crate::app::commands::restart::restart_service_no_wait;
use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;
use crate::app::runtime::persist_manifest;
use crate::config::{ConfigFile, ServiceConfig, TaskConfig};
use crate::ids::RunId;
use crate::logfmt::extract_log_content;
use crate::manifest::ServiceState;
use crate::model::{HealthHandle, HealthSnapshot};
use crate::paths;
use crate::services::readiness::{ReadinessContext, ReadinessKind, ReadinessSpec};
use crate::stores::{recompute_run_state, set_service_state};

use super::pipeline::wait_for_prepared_service;
use super::prepare::PreparedService;

#[derive(Clone)]
pub struct PostInitContext {
    pub tasks_map: BTreeMap<String, TaskConfig>,
    pub post_init_tasks: Vec<String>,
    pub project_dir: PathBuf,
    pub run_id: RunId,
}

pub fn build_post_init_context(
    service: &ServiceConfig,
    tasks_map: &BTreeMap<String, TaskConfig>,
    project_dir: &Path,
    run_id: &RunId,
) -> Option<PostInitContext> {
    let post_init = service.post_init.as_ref()?;
    if post_init.is_empty() {
        return None;
    }
    Some(PostInitContext {
        tasks_map: tasks_map.clone(),
        post_init_tasks: post_init.clone(),
        project_dir: project_dir.to_path_buf(),
        run_id: run_id.clone(),
    })
}

pub fn load_post_init_context_for_run_service(
    run_id: &str,
    stack: &str,
    project_dir: &Path,
    service: &str,
) -> Result<Option<PostInitContext>> {
    let snapshot_path = paths::run_snapshot_path(&RunId::new(run_id))?;
    if !snapshot_path.exists() {
        return Ok(None);
    }

    let config = ConfigFile::load_from_path(&snapshot_path)?;
    let tasks_map = config
        .tasks
        .as_ref()
        .map(|tasks| tasks.as_map().clone())
        .unwrap_or_default();
    let service_config = if stack == "globals" {
        config.globals_map().get(service).cloned()
    } else {
        config.stack_plan(stack)?.services.get(service).cloned()
    };

    Ok(service_config.and_then(|service_config| {
        build_post_init_context(
            &service_config,
            &tasks_map,
            project_dir,
            &RunId::new(run_id),
        )
    }))
}

pub async fn handle_readiness(
    app: AppContext,
    run_id: &str,
    prepared: &PreparedService,
    no_wait: bool,
    post_init: Option<PostInitContext>,
) -> AppResult<()> {
    if no_wait {
        spawn_readiness_task(
            app.clone(),
            run_id.to_string(),
            prepared.name.clone(),
            prepared.readiness.clone(),
            prepared.port,
            prepared.scheme.clone(),
            prepared.log_path.clone(),
            prepared.cwd.clone(),
            prepared.env.clone(),
            prepared.unit_name.clone(),
            post_init,
        );
        return Ok(());
    }

    match wait_for_prepared_service(&app, &prepared.name, prepared, post_init).await {
        Ok(()) => {
            mark_service_ready(&app, run_id, &prepared.name)
                .await
                .map_err(AppError::from)?;
        }
        Err(err) => {
            let detailed = enrich_readiness_error(
                &app,
                &prepared.name,
                &prepared.unit_name,
                &prepared.log_path,
                err,
            )
            .await;
            mark_service_failed(&app, run_id, &prepared.name, &detailed.to_string())
                .await
                .map_err(AppError::from)?;
            return Err(AppError::Internal(detailed));
        }
    }

    Ok(())
}

pub async fn mark_service_ready(app: &AppContext, run_id: &str, service: &str) -> Result<()> {
    let (start_monitor, events) = app
        .runs
        .with_run_mut(run_id, |run| {
            let mut start_monitor = false;
            let mut events = Vec::new();
            if let Some(record) = run.services.get_mut(service) {
                if let Some(event) = set_service_state(run_id, service, record, ServiceState::Ready)
                {
                    events.push(event);
                }
                record.runtime.last_failure = None;
                if record.handles.health.is_none()
                    && !matches!(record.spec.readiness.kind, ReadinessKind::Exit)
                {
                    record.handles.health = Some(HealthHandle {
                        stop_flag: Arc::new(AtomicBool::new(false)),
                        stats: Arc::new(std::sync::Mutex::new(HealthSnapshot::default())),
                    });
                    start_monitor = true;
                }
            }
            if let Some(event) = recompute_run_state(run) {
                events.push(event);
            }
            (start_monitor, events)
        })
        .await?;

    app.emit_events(events);
    if start_monitor {
        start_health_monitor(app.clone(), run_id.to_string(), service.to_string());
    }
    Ok(())
}

pub async fn mark_service_failed(
    app: &AppContext,
    run_id: &str,
    service: &str,
    reason: &str,
) -> Result<()> {
    let events = app
        .runs
        .with_run_mut(run_id, |run| {
            let mut events = Vec::new();
            if let Some(record) = run.services.get_mut(service) {
                if let Some(event) =
                    set_service_state(run_id, service, record, ServiceState::Failed)
                {
                    events.push(event);
                }
                record.runtime.last_failure = Some(reason.to_string());
            }
            if let Some(event) = recompute_run_state(run) {
                events.push(event);
            }
            events
        })
        .await?;
    app.emit_events(events);
    Ok(())
}

pub fn start_health_monitor(app: AppContext, run_id: String, service: String) {
    tokio::spawn(async move {
        let Some((readiness, context, stop_flag, stats)) = app
            .runs
            .with_run(&run_id, |run| {
                let record = run.services.get(&service)?;
                let handle = record.handles.health.as_ref()?;
                Some((
                    record.spec.readiness.clone(),
                    ReadinessContext {
                        port: record.launch.port,
                        scheme: record.launch.scheme.clone(),
                        log_path: record.launch.log_path.clone(),
                        cwd: record.launch.cwd.clone(),
                        env: record.launch.env.clone(),
                        unit_name: Some(record.launch.unit_name.clone()),
                        systemd: Some(app.systemd.clone()),
                    },
                    handle.stop_flag.clone(),
                    handle.stats.clone(),
                ))
            })
            .await
            .ok()
            .flatten()
        else {
            return;
        };

        let mut restart_count = 0_u32;
        let mut consecutive_failures = 0_u32;

        loop {
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }

            let ok = crate::readiness::check_ready_once(&readiness, &context)
                .await
                .unwrap_or(false);
            let checked_at = crate::util::now_rfc3339();

            if ok {
                consecutive_failures = 0;
                restart_count = 0;
            } else {
                consecutive_failures = consecutive_failures.saturating_add(1);
            }

            {
                let mut snapshot = stats.lock().unwrap_or_else(|err| err.into_inner());
                snapshot.last_check_at = Some(checked_at);
                snapshot.last_ok = Some(ok);
                snapshot.consecutive_failures = consecutive_failures;
                if ok {
                    snapshot.passes += 1;
                } else {
                    snapshot.failures += 1;
                }
            }

            enum Transition {
                Healthy,
                RecoverToReady,
                MarkDegraded,
            }

            let transition = if ok && consecutive_failures == 0 {
                Transition::RecoverToReady
            } else if consecutive_failures >= 3 {
                Transition::MarkDegraded
            } else {
                Transition::Healthy
            };

            let (events, should_restart) = match transition {
                Transition::Healthy => (Vec::new(), false),
                Transition::RecoverToReady => {
                    let events = app
                        .runs
                        .with_run_mut(&run_id, |run| {
                            let mut events = Vec::new();
                            if let Some(record) = run.services.get_mut(&service)
                                && record.runtime.state == ServiceState::Degraded
                                && record.runtime.last_failure.as_deref()
                                    == Some("health checks failing")
                            {
                                if let Some(event) = set_service_state(
                                    &run_id,
                                    &service,
                                    record,
                                    ServiceState::Ready,
                                ) {
                                    events.push(event);
                                }
                                record.runtime.last_failure = None;
                            }
                            if let Some(event) = recompute_run_state(run) {
                                events.push(event);
                            }
                            events
                        })
                        .await
                        .unwrap_or_default();
                    (events, false)
                }
                Transition::MarkDegraded => {
                    let events = app
                        .runs
                        .with_run_mut(&run_id, |run| {
                            let mut events = Vec::new();
                            if let Some(record) = run.services.get_mut(&service)
                                && record.runtime.state == ServiceState::Ready
                            {
                                if let Some(event) = set_service_state(
                                    &run_id,
                                    &service,
                                    record,
                                    ServiceState::Degraded,
                                ) {
                                    events.push(event);
                                }
                                record.runtime.last_failure =
                                    Some("health checks failing".to_string());
                            }
                            if let Some(event) = recompute_run_state(run) {
                                events.push(event);
                            }
                            events
                        })
                        .await
                        .unwrap_or_default();
                    let should_restart = !events.is_empty() && restart_count < 3;
                    (events, should_restart)
                }
            };

            let changed = !events.is_empty();
            app.emit_events(events);

            if changed && let Err(err) = persist_manifest(&app, &run_id).await {
                eprintln!(
                    "devstack: failed to persist manifest after health transition for {run_id}/{service}: {err}"
                );
            }

            if should_restart {
                restart_count += 1;
                let backoff = match restart_count {
                    1 => Duration::from_secs(0),
                    2 => Duration::from_secs(5),
                    _ => Duration::from_secs(30),
                };

                if backoff.as_secs() > 0 {
                    eprintln!(
                        "devstack: restarting {} in {}s (attempt {})",
                        service,
                        backoff.as_secs(),
                        restart_count
                    );
                    tokio::time::sleep(backoff).await;
                } else {
                    eprintln!(
                        "devstack: restarting {} (attempt {})",
                        service, restart_count
                    );
                }

                if let Err(err) = restart_service_no_wait(&app, &run_id, &service).await {
                    eprintln!("devstack: failed to restart {}: {:?}", service, err);
                } else {
                    eprintln!("devstack: restarted service {}", service);
                }

                if restart_count >= 3 {
                    let events = app
                        .runs
                        .with_run_mut(&run_id, |run| {
                            let mut events = Vec::new();
                            if let Some(record) = run.services.get_mut(&service) {
                                if let Some(event) = set_service_state(
                                    &run_id,
                                    &service,
                                    record,
                                    ServiceState::Failed,
                                ) {
                                    events.push(event);
                                }
                                record.runtime.last_failure =
                                    Some("health restart limit exceeded".to_string());
                            }
                            if let Some(event) = recompute_run_state(run) {
                                events.push(event);
                            }
                            events
                        })
                        .await
                        .unwrap_or_default();
                    app.emit_events(events);
                    if let Err(err) = persist_manifest(&app, &run_id).await {
                        eprintln!(
                            "devstack: failed to persist manifest after restart limit for {run_id}/{service}: {err}"
                        );
                    }
                    break;
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_readiness_task(
    app: AppContext,
    run_id: String,
    service: String,
    readiness: ReadinessSpec,
    port: Option<u16>,
    scheme: String,
    log_path: PathBuf,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    unit_name: String,
    post_init: Option<PostInitContext>,
) {
    tokio::spawn(async move {
        let prepared = PreparedService {
            name: service.clone(),
            unit_name: unit_name.clone(),
            port,
            scheme,
            url: None,
            deps: Vec::new(),
            readiness,
            log_path: log_path.clone(),
            cwd,
            env,
            cmd: String::new(),
            watch_hash: String::new(),
            watch_patterns: Vec::new(),
            ignore_patterns: Vec::new(),
            watch_extra_files: Vec::new(),
            watch_fingerprint: Vec::new(),
            auto_restart: false,
        };
        match wait_for_prepared_service(&app, &service, &prepared, post_init).await {
            Ok(()) => {
                if let Err(err) = mark_service_ready(&app, &run_id, &service).await {
                    eprintln!(
                        "devstack: failed to mark service {service} ready for run {run_id}: {err}"
                    );
                }
                if let Err(err) = persist_manifest(&app, &run_id).await {
                    eprintln!(
                        "devstack: failed to persist manifest after readiness success for {run_id}/{service}: {err}"
                    );
                }
            }
            Err(err) => {
                let detailed =
                    enrich_readiness_error(&app, &service, &unit_name, &log_path, err).await;
                if let Err(mark_err) =
                    mark_service_failed(&app, &run_id, &service, &detailed.to_string()).await
                {
                    eprintln!(
                        "devstack: failed to mark service {service} failed for run {run_id}: {mark_err}"
                    );
                }
                if let Err(err) = persist_manifest(&app, &run_id).await {
                    eprintln!(
                        "devstack: failed to persist manifest after readiness failure for {run_id}/{service}: {err}"
                    );
                }
            }
        }
    });
}

fn format_terminal_unit_status(status: &crate::systemd::UnitStatus) -> Option<String> {
    let failed = status.active_state == "failed"
        || (status.active_state == "inactive"
            && status
                .result
                .as_deref()
                .map(|result| result != "success")
                .unwrap_or(false));
    if !failed {
        return None;
    }

    let result = status
        .result
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    Some(format!(
        "exited before readiness (active_state={}, sub_state={}, result={result})",
        status.active_state, status.sub_state
    ))
}

fn tail_log_messages(log_path: &Path, limit: usize) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(log_path) else {
        return Vec::new();
    };

    content
        .lines()
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter_map(|line| {
            let (_, message) = extract_log_content(line);
            let trimmed = message.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect()
}

async fn enrich_readiness_error(
    app: &AppContext,
    service: &str,
    unit_name: &str,
    log_path: &Path,
    err: anyhow::Error,
) -> anyhow::Error {
    let mut message = err.to_string();

    if let Ok(Some(status)) = app.systemd.unit_status(unit_name).await
        && let Some(reason) = format_terminal_unit_status(&status)
    {
        message = format!("service '{service}' {reason}");
    }

    let recent_logs = tail_log_messages(log_path, 10);
    if !recent_logs.is_empty() {
        message.push_str("\nlast log lines:\n");
        message.push_str(&recent_logs.join("\n"));
    }

    anyhow!(message)
}
