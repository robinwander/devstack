use crate::api::RunWatchResponse;
use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;
use crate::app::launch::sync_service_auto_restart_watcher;
use crate::app::queries::watch::build_watch_status;

pub async fn pause_watch(
    app: &AppContext,
    run_id: &str,
    service: Option<&str>,
) -> AppResult<RunWatchResponse> {
    app.runs
        .with_run_mut(run_id, |run| {
            if let Some(service) = service {
                let record = run
                    .services
                    .get_mut(service)
                    .ok_or_else(|| AppError::not_found(format!("service {service} not found")))?;
                if !record.spec.auto_restart {
                    return Err(AppError::bad_request(format!(
                        "service {service} does not have auto_restart enabled"
                    )));
                }
                record.runtime.watch_paused = true;
                if let Some(handle) = record.handles.watch.as_ref() {
                    handle
                        .paused
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                return Ok(());
            }

            for record in run.services.values_mut() {
                if !record.spec.auto_restart {
                    continue;
                }
                record.runtime.watch_paused = true;
                if let Some(handle) = record.handles.watch.as_ref() {
                    handle
                        .paused
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| AppError::not_found(format!("run {run_id} not found")))??;

    build_watch_status(app, run_id).await
}

pub async fn resume_watch(
    app: &AppContext,
    run_id: &str,
    service: Option<&str>,
) -> AppResult<RunWatchResponse> {
    let targets =
        app.runs
            .with_run_mut(run_id, |run| {
                let mut targets = Vec::new();
                if let Some(service) = service {
                    let record = run.services.get_mut(service).ok_or_else(|| {
                        AppError::not_found(format!("service {service} not found"))
                    })?;
                    if !record.spec.auto_restart {
                        return Err(AppError::bad_request(format!(
                            "service {service} does not have auto_restart enabled"
                        )));
                    }
                    record.runtime.watch_paused = false;
                    if let Some(handle) = record.handles.watch.as_ref() {
                        handle
                            .paused
                            .store(false, std::sync::atomic::Ordering::SeqCst);
                    }
                    targets.push(service.to_string());
                } else {
                    for (name, record) in &mut run.services {
                        if !record.spec.auto_restart {
                            continue;
                        }
                        record.runtime.watch_paused = false;
                        if let Some(handle) = record.handles.watch.as_ref() {
                            handle
                                .paused
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                        }
                        targets.push(name.clone());
                    }
                }
                Ok(targets)
            })
            .await
            .map_err(|_| AppError::not_found(format!("run {run_id} not found")))??;

    for target in targets {
        if let Err(err) = sync_service_auto_restart_watcher(app, run_id, &target).await {
            eprintln!("devstack: failed to resume watcher for {target}: {err}");
        }
    }

    build_watch_status(app, run_id).await
}
