use anyhow::anyhow;

use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;
use crate::app::launch::{
    load_post_init_context_for_run_service, mark_service_failed, mark_service_ready,
    spawn_readiness_task,
};
use crate::app::runtime::{persist_manifest, run_response};
use crate::model::ServiceState;
use crate::services::readiness::ReadinessContext;
use crate::stores::{recompute_run_state, set_service_state};

pub async fn restart_service(
    app: &AppContext,
    run_id: &str,
    service: &str,
    no_wait: bool,
) -> AppResult<crate::api::RunResponse> {
    restart_service_inner(app, run_id, service, no_wait).await
}

pub async fn restart_service_no_wait(
    app: &AppContext,
    run_id: &str,
    service: &str,
) -> AppResult<crate::api::RunResponse> {
    restart_service_inner(app, run_id, service, true).await
}

async fn restart_service_inner(
    app: &AppContext,
    run_id: &str,
    service: &str,
    no_wait: bool,
) -> AppResult<crate::api::RunResponse> {
    let (unit_name, readiness, port, scheme, log_path, cwd, env, stack, project_dir, events) = app
        .runs
        .with_run_mut(run_id, |run| -> Result<_, AppError> {
            let mut events = Vec::new();
            let stack = run.stack.clone();
            let project_dir = run.project_dir.clone();
            let record = run
                .services
                .get_mut(service)
                .ok_or_else(|| AppError::not_found(format!("service {service} not found")))?;
            if let Some(event) = set_service_state(run_id, service, record, ServiceState::Starting)
            {
                events.push(event);
            }
            record.runtime.last_failure = None;
            let snapshot = (
                record.launch.unit_name.clone(),
                record.spec.readiness.clone(),
                record.launch.port,
                record.launch.scheme.clone(),
                record.launch.log_path.clone(),
                record.launch.cwd.clone(),
                record.launch.env.clone(),
            );
            if let Some(event) = recompute_run_state(run) {
                events.push(event);
            }
            Ok((
                snapshot.0,
                snapshot.1,
                snapshot.2,
                snapshot.3,
                snapshot.4,
                snapshot.5,
                snapshot.6,
                stack,
                project_dir,
                events,
            ))
        })
        .await
        .map_err(|_| AppError::not_found(format!("run {run_id} not found")))??;
    app.emit_events(events);

    let post_init =
        load_post_init_context_for_run_service(run_id, &stack, &project_dir, service, env.clone())
            .map_err(AppError::from)?;

    app.systemd
        .restart_unit(&unit_name)
        .await
        .map_err(AppError::from)?;

    let _ = app
        .runs
        .with_run_mut(run_id, |run| {
            if let Some(record) = run.services.get_mut(service) {
                record.runtime.last_started_at = Some(crate::util::now_rfc3339());
            }
        })
        .await;

    if no_wait {
        spawn_readiness_task(
            app.clone(),
            run_id.to_string(),
            service.to_string(),
            readiness,
            port,
            scheme,
            log_path,
            cwd,
            env,
            unit_name.clone(),
            post_init,
        );
        persist_manifest(app, run_id)
            .await
            .map_err(AppError::from)?;
        return run_response(app, run_id).await.map_err(AppError::from);
    }

    let context = ReadinessContext {
        port,
        scheme,
        log_path: log_path.clone(),
        cwd,
        env,
        unit_name: Some(unit_name.clone()),
        systemd: Some(app.systemd.clone()),
    };
    match crate::services::readiness::wait_for_ready(&readiness, &context).await {
        Ok(()) => {
            if let Some(post_init) = post_init
                && let Err(err) = crate::app::commands::tasks::run_post_init_tasks_blocking(
                    post_init.tasks_map,
                    post_init.post_init_tasks,
                    post_init.project_dir,
                    post_init.run_id,
                    post_init.base_env,
                )
                .await
            {
                let reason = format!("post_init task failed: {err}");
                eprintln!("[{service}] {reason}");
                mark_service_failed(app, run_id, service, &reason)
                    .await
                    .map_err(AppError::from)?;
                let _ = persist_manifest(app, run_id).await;
                return Err(AppError::Internal(anyhow!(reason)));
            }
            mark_service_ready(app, run_id, service)
                .await
                .map_err(AppError::from)?;
        }
        Err(err) => {
            let message = err.to_string();
            mark_service_failed(app, run_id, service, &message)
                .await
                .map_err(AppError::from)?;
            let _ = persist_manifest(app, run_id).await;
            return Err(AppError::Internal(err));
        }
    }

    persist_manifest(app, run_id)
        .await
        .map_err(AppError::from)?;
    run_response(app, run_id).await.map_err(AppError::from)
}
