use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::time::Instant;

use crate::app::commands::ensure_globals::restart_global_no_wait;
use crate::app::commands::restart::restart_service_no_wait;
use crate::app::context::AppContext;
use crate::manifest::{RunLifecycle, ServiceState};
use crate::model::{GlobalRecord, ServiceRecord, ServiceWatchHandle};
use crate::watch::compute_watch_hash;

use super::prepare::PreparedService;

pub type WatchStartArgs = (
    PathBuf,
    Vec<String>,
    Vec<String>,
    Vec<PathBuf>,
    Vec<u8>,
    bool,
);

pub type GlobalWatchStartArgs = (
    String,
    PathBuf,
    Vec<String>,
    Vec<String>,
    Vec<PathBuf>,
    Vec<u8>,
    bool,
);

pub fn stop_health_monitor_for_service(service: &mut ServiceRecord) {
    service.stop_health_monitor();
}

pub fn stop_watch_for_service(service: &mut ServiceRecord) {
    service.stop_watch();
}

pub fn stop_watch_for_global(global: &mut GlobalRecord) {
    global.service.stop_watch();
}

pub fn apply_prepared_to_runtime(
    service: &mut ServiceRecord,
    prepared: &PreparedService,
    reset_state: bool,
) {
    if reset_state {
        stop_health_monitor_for_service(service);
        stop_watch_for_service(service);
        service.runtime.state = ServiceState::Starting;
        service.runtime.last_failure = None;
    }

    service.spec.name = prepared.name.clone();
    service.spec.deps = prepared.deps.clone();
    service.spec.readiness = prepared.readiness.clone();
    service.spec.auto_restart = prepared.auto_restart;
    service.spec.watch_patterns = prepared.watch_patterns.clone();
    service.spec.ignore_patterns = prepared.ignore_patterns.clone();

    service.launch.unit_name = prepared.unit_name.clone();
    service.launch.cwd = prepared.cwd.clone();
    service.launch.env = prepared.env.clone();
    service.launch.cmd = prepared.cmd.clone();
    service.launch.log_path = prepared.log_path.clone();
    service.launch.port = prepared.port;
    service.launch.scheme = prepared.scheme.clone();
    service.launch.url = prepared.url.clone();
    service.launch.watch_hash = prepared.watch_hash.clone();
    service.launch.watch_fingerprint = prepared.watch_fingerprint.clone();
    service.launch.watch_extra_files = prepared.watch_extra_files.clone();

    if service.spec.auto_restart != prepared.auto_restart {
        service.runtime.watch_paused = false;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_service_auto_restart_watcher(
    app: AppContext,
    run_id: String,
    service: String,
    cwd: PathBuf,
    watch_patterns: Vec<String>,
    ignore_patterns: Vec<String>,
    watch_extra_files: Vec<PathBuf>,
    watch_fingerprint: Vec<u8>,
    paused: bool,
) -> Result<ServiceWatchHandle> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = event_tx.send(event);
    })
    .context("create filesystem watcher")?;
    watcher
        .watch(&cwd, RecursiveMode::Recursive)
        .with_context(|| format!("watch directory {}", cwd.to_string_lossy()))?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let paused_flag = Arc::new(AtomicBool::new(paused));
    let stop_flag_task = stop_flag.clone();
    let paused_flag_task = paused_flag.clone();

    tokio::spawn(async move {
        let _watcher = watcher;
        let debounce = Duration::from_millis(500);
        let mut pending = false;
        let mut last_event_at = Instant::now();

        loop {
            if stop_flag_task.load(Ordering::SeqCst) {
                break;
            }

            match tokio::time::timeout(Duration::from_millis(100), event_rx.recv()).await {
                Ok(Some(Ok(event))) => {
                    if matches!(
                        event.kind,
                        EventKind::Any
                            | EventKind::Create(_)
                            | EventKind::Modify(_)
                            | EventKind::Remove(_)
                    ) {
                        pending = true;
                        last_event_at = Instant::now();
                    }
                }
                Ok(Some(Err(err))) => {
                    eprintln!("devstack: watch error for {}.{}: {}", run_id, service, err);
                }
                Ok(None) => break,
                Err(_) => {}
            }

            if !pending || last_event_at.elapsed() < debounce {
                continue;
            }
            pending = false;

            if paused_flag_task.load(Ordering::SeqCst) {
                continue;
            }

            let watch_patterns = if watch_patterns.is_empty() {
                None
            } else {
                Some(watch_patterns.as_slice())
            };
            let next_hash = match compute_watch_hash(
                &cwd,
                watch_patterns,
                &ignore_patterns,
                &watch_extra_files,
                &watch_fingerprint,
            ) {
                Ok(hash) => hash,
                Err(err) => {
                    eprintln!(
                        "devstack: failed to compute watch hash for {}.{}: {}",
                        run_id, service, err
                    );
                    continue;
                }
            };

            let should_restart = match app
                .runs
                .with_run_mut(&run_id, |run| {
                    let Some(record) = run.services.get_mut(&service) else {
                        return None;
                    };
                    paused_flag_task.store(record.runtime.watch_paused, Ordering::SeqCst);
                    if !record.spec.auto_restart
                        || record.runtime.watch_paused
                        || record.launch.watch_hash == next_hash
                    {
                        return Some(false);
                    }
                    record.launch.watch_hash = next_hash.clone();
                    Some(true)
                })
                .await
            {
                Ok(Some(should_restart)) => should_restart,
                _ => break,
            };

            if should_restart
                && let Err(err) = restart_service_no_wait(&app, &run_id, &service).await
            {
                eprintln!(
                    "devstack: auto-restart failed for {}.{}: {:?}",
                    run_id, service, err
                );
            }
        }
    });

    Ok(ServiceWatchHandle {
        stop_flag,
        paused: paused_flag,
    })
}

pub async fn sync_service_auto_restart_watcher(
    app: &AppContext,
    run_id: &str,
    service: &str,
) -> Result<()> {
    let start_args: Option<WatchStartArgs> = app
        .runs
        .with_run_mut(run_id, |run| {
            let Some(record) = run.services.get_mut(service) else {
                return None;
            };

            if !record.spec.auto_restart || record.runtime.last_started_at.is_none() {
                stop_watch_for_service(record);
                return None;
            }

            if let Some(handle) = record.handles.watch.as_ref() {
                handle
                    .paused
                    .store(record.runtime.watch_paused, Ordering::SeqCst);
                return None;
            }

            Some((
                record.launch.cwd.clone(),
                record.spec.watch_patterns.clone(),
                record.spec.ignore_patterns.clone(),
                record.launch.watch_extra_files.clone(),
                record.launch.watch_fingerprint.clone(),
                record.runtime.watch_paused,
            ))
        })
        .await
        .ok()
        .flatten();

    if let Some((
        cwd,
        watch_patterns,
        ignore_patterns,
        watch_extra_files,
        watch_fingerprint,
        paused,
    )) = start_args
    {
        let handle = spawn_service_auto_restart_watcher(
            app.clone(),
            run_id.to_string(),
            service.to_string(),
            cwd,
            watch_patterns,
            ignore_patterns,
            watch_extra_files,
            watch_fingerprint,
            paused,
        )?;

        let keep_handle = app
            .runs
            .with_run_mut(run_id, |run| {
                let Some(record) = run.services.get_mut(service) else {
                    return false;
                };
                if record.spec.auto_restart
                    && record.runtime.last_started_at.is_some()
                    && record.handles.watch.is_none()
                {
                    handle
                        .paused
                        .store(record.runtime.watch_paused, Ordering::SeqCst);
                    record.handles.watch = Some(handle.clone());
                    true
                } else {
                    false
                }
            })
            .await
            .unwrap_or(false);

        if !keep_handle {
            handle.stop_flag.store(true, Ordering::SeqCst);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_global_auto_restart_watcher(
    app: AppContext,
    key: String,
    service: String,
    cwd: PathBuf,
    watch_patterns: Vec<String>,
    ignore_patterns: Vec<String>,
    watch_extra_files: Vec<PathBuf>,
    watch_fingerprint: Vec<u8>,
    paused: bool,
) -> Result<ServiceWatchHandle> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = event_tx.send(event);
    })
    .context("create filesystem watcher")?;
    watcher
        .watch(&cwd, RecursiveMode::Recursive)
        .with_context(|| format!("watch directory {}", cwd.to_string_lossy()))?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let paused_flag = Arc::new(AtomicBool::new(paused));
    let stop_flag_task = stop_flag.clone();
    let paused_flag_task = paused_flag.clone();

    tokio::spawn(async move {
        let _watcher = watcher;
        let debounce = Duration::from_millis(500);
        let mut pending = false;
        let mut last_event_at = Instant::now();

        loop {
            if stop_flag_task.load(Ordering::SeqCst) {
                break;
            }

            match tokio::time::timeout(Duration::from_millis(100), event_rx.recv()).await {
                Ok(Some(Ok(event))) => {
                    if matches!(
                        event.kind,
                        EventKind::Any
                            | EventKind::Create(_)
                            | EventKind::Modify(_)
                            | EventKind::Remove(_)
                    ) {
                        pending = true;
                        last_event_at = Instant::now();
                    }
                }
                Ok(Some(Err(err))) => {
                    eprintln!(
                        "devstack: watch error for global {}.{}: {}",
                        key, service, err
                    );
                }
                Ok(None) => break,
                Err(_) => {}
            }

            if !pending || last_event_at.elapsed() < debounce {
                continue;
            }
            pending = false;

            if paused_flag_task.load(Ordering::SeqCst) {
                continue;
            }

            let watch_patterns = if watch_patterns.is_empty() {
                None
            } else {
                Some(watch_patterns.as_slice())
            };
            let next_hash = match compute_watch_hash(
                &cwd,
                watch_patterns,
                &ignore_patterns,
                &watch_extra_files,
                &watch_fingerprint,
            ) {
                Ok(hash) => hash,
                Err(err) => {
                    eprintln!(
                        "devstack: failed to compute watch hash for global {}.{}: {}",
                        key, service, err
                    );
                    continue;
                }
            };

            let should_restart = match app
                .globals
                .with_global_mut(&key, |global| {
                    paused_flag_task.store(global.service.runtime.watch_paused, Ordering::SeqCst);
                    if !global.service.spec.auto_restart
                        || global.service.runtime.watch_paused
                        || global.service.launch.watch_hash == next_hash
                    {
                        return false;
                    }
                    global.service.launch.watch_hash = next_hash.clone();
                    global.state = RunLifecycle::Starting;
                    true
                })
                .await
            {
                Ok(should_restart) => should_restart,
                Err(_) => break,
            };

            if should_restart && let Err(err) = restart_global_no_wait(&app, &key).await {
                eprintln!(
                    "devstack: auto-restart failed for global {}.{}: {:?}",
                    key, service, err
                );
            }
        }
    });

    Ok(ServiceWatchHandle {
        stop_flag,
        paused: paused_flag,
    })
}

pub async fn sync_global_auto_restart_watcher(app: &AppContext, key: &str) -> Result<()> {
    let start_args: Option<GlobalWatchStartArgs> = app
        .globals
        .with_global_mut(key, |global| {
            if !global.service.spec.auto_restart || global.service.runtime.last_started_at.is_none()
            {
                stop_watch_for_global(global);
                return None;
            }

            if let Some(handle) = global.service.handles.watch.as_ref() {
                handle
                    .paused
                    .store(global.service.runtime.watch_paused, Ordering::SeqCst);
                return None;
            }

            Some((
                global.name.clone(),
                global.service.launch.cwd.clone(),
                global.service.spec.watch_patterns.clone(),
                global.service.spec.ignore_patterns.clone(),
                global.service.launch.watch_extra_files.clone(),
                global.service.launch.watch_fingerprint.clone(),
                global.service.runtime.watch_paused,
            ))
        })
        .await
        .ok()
        .flatten();

    if let Some((
        service,
        cwd,
        watch_patterns,
        ignore_patterns,
        watch_extra_files,
        watch_fingerprint,
        paused,
    )) = start_args
    {
        let handle = spawn_global_auto_restart_watcher(
            app.clone(),
            key.to_string(),
            service,
            cwd,
            watch_patterns,
            ignore_patterns,
            watch_extra_files,
            watch_fingerprint,
            paused,
        )?;

        let keep_handle = app
            .globals
            .with_global_mut(key, |global| {
                if global.service.spec.auto_restart
                    && global.service.runtime.last_started_at.is_some()
                    && global.service.handles.watch.is_none()
                {
                    handle
                        .paused
                        .store(global.service.runtime.watch_paused, Ordering::SeqCst);
                    global.service.handles.watch = Some(handle.clone());
                    true
                } else {
                    false
                }
            })
            .await
            .unwrap_or(false);

        if !keep_handle {
            handle.stop_flag.store(true, Ordering::SeqCst);
        }
    }

    Ok(())
}
