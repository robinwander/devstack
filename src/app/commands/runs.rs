use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rand::Rng;

use crate::api::UpRequest;
use crate::app::commands::ensure_globals::ensure_globals;
use crate::app::commands::tasks::run_init_tasks_blocking;
use crate::app::context::{AppContext, AppResult};
use crate::app::error::AppError;
use crate::app::launch::{
    ExistingServiceSnapshot, apply_prepared_to_runtime, build_base_env, build_post_init_context,
    prepare_service, resolve_ports_for_refresh, start_prepared_service,
    sync_service_auto_restart_watcher,
};
use crate::app::runtime::{
    find_latest_run_for_project_stack, persist_manifest, release_service_port, run_created_event,
    run_removed_event, run_response, run_state_changed_event,
};
use crate::config::{ConfigFile, StackPlan};
use crate::ids::RunId;
use crate::manifest::{RunLifecycle, ServiceState};
use crate::model::{InstanceScope, RunRecord};
use crate::paths;
use crate::port::allocate_ports;
use crate::projects::ProjectsLedger;
use crate::stores::{recompute_run_state, service_state_changed_event, set_service_state};
use crate::util::{atomic_write, now_rfc3339};

pub async fn up(app: &AppContext, request: UpRequest) -> AppResult<crate::api::RunResponse> {
    let project_dir = PathBuf::from(&request.project_dir);

    if let Ok(mut ledger) = ProjectsLedger::load() {
        let _ = ledger.touch(&project_dir);
    }

    let config_path = request
        .file
        .map(PathBuf::from)
        .unwrap_or_else(|| ConfigFile::default_path(&project_dir));
    let config = ConfigFile::load_from_path(&config_path)
        .map_err(|err| AppError::bad_request(err.to_string()))?;
    let stack_plan = config
        .stack_plan(&request.stack)
        .map_err(|err| AppError::bad_request(err.to_string()))?;
    let config_dir = config_path.parent().unwrap_or(&project_dir).to_path_buf();

    if !request.new_run
        && request.run_id.is_none()
        && let Some(existing) = find_latest_run_for_project_stack(app, &project_dir, &request.stack)
            .await
            .map_err(AppError::from)?
    {
        return refresh_run(
            app,
            &existing,
            &config,
            &stack_plan,
            &project_dir,
            &config_path,
            request.no_wait,
            request.force,
        )
        .await;
    }

    let run_id = RunId::new(
        request
            .run_id
            .unwrap_or_else(|| generate_run_id(&request.stack)),
    );

    paths::ensure_base_layout().map_err(AppError::from)?;
    let run_dir = paths::run_dir(&run_id).map_err(AppError::from)?;
    let logs_dir = paths::run_logs_dir(&run_id).map_err(AppError::from)?;
    std::fs::create_dir_all(&logs_dir).map_err(AppError::from)?;
    std::fs::create_dir_all(&run_dir).map_err(AppError::from)?;

    let snapshot_path = paths::run_snapshot_path(&run_id).map_err(AppError::from)?;
    let raw = std::fs::read(&config_path).map_err(AppError::from)?;
    atomic_write(&snapshot_path, &raw).map_err(AppError::from)?;

    let mut port_map = allocate_ports(&stack_plan.services, |service| {
        crate::app::runtime::port_owner(run_id.as_str(), service)
    })
    .map_err(|err| AppError::bad_request(err.to_string()))?;
    let tasks_map = config
        .tasks
        .as_ref()
        .map(|tasks| tasks.as_map().clone())
        .unwrap_or_default();

    let globals = config.globals_map();
    let global_ports = ensure_globals(app, &globals, &tasks_map, &project_dir, &config_dir)
        .await
        .map_err(AppError::from)?;

    let mut service_schemes = BTreeMap::new();
    for (name, service) in &stack_plan.services {
        service_schemes.insert(name.clone(), service.scheme());
    }
    for (name, service) in &globals {
        service_schemes.insert(name.clone(), service.scheme());
    }
    for (name, port) in &global_ports {
        port_map.entry(name.clone()).or_insert(*port);
    }

    let scope = InstanceScope::run(run_id.clone(), stack_plan.name.clone());
    let base_env = build_base_env(&scope, &project_dir, &port_map, &service_schemes)
        .map_err(AppError::from)?;
    let run_record = RunRecord::new(
        run_id.clone(),
        stack_plan.name.clone(),
        project_dir.clone(),
        config_dir.clone(),
        base_env.clone(),
    );
    app.runs
        .create_run(run_record)
        .await
        .map_err(AppError::from)?;
    if let Some(run) = app.runs.get_run(run_id.as_str()).await {
        app.emit_event(run_created_event(&run));
    }

    launch_services(
        app,
        &run_id,
        &stack_plan,
        &project_dir,
        &config_dir,
        &tasks_map,
        &port_map,
        &service_schemes,
        &base_env,
        request.no_wait,
        false,
        &BTreeMap::new(),
    )
    .await?;

    let events = app
        .runs
        .with_run_mut(run_id.as_str(), recompute_run_state)
        .await
        .map_err(AppError::from)?
        .into_iter()
        .collect::<Vec<_>>();
    app.emit_events(events);

    persist_manifest(app, run_id.as_str())
        .await
        .map_err(AppError::from)?;
    run_response(app, run_id.as_str())
        .await
        .map_err(AppError::from)
}

#[allow(clippy::too_many_arguments)]
pub async fn refresh_run(
    app: &AppContext,
    run_id: &str,
    config: &ConfigFile,
    stack_plan: &StackPlan,
    project_dir: &Path,
    config_path: &Path,
    no_wait: bool,
    force: bool,
) -> AppResult<crate::api::RunResponse> {
    let run_id = RunId::new(run_id.to_string());
    let config_dir = config_path.parent().unwrap_or(project_dir);

    let (existing, removed, reuse_ports) = app
        .runs
        .with_run(run_id.as_str(), |run| {
            let reuse_ports = run.stopped_at.is_none();
            let mut existing = BTreeMap::new();
            for (name, service) in &run.services {
                existing.insert(
                    name.clone(),
                    ExistingServiceSnapshot {
                        watch_hash: Some(service.launch.watch_hash.clone()),
                        state: service.runtime.state.clone(),
                        port: service.launch.port,
                    },
                );
            }
            let removed = run
                .services
                .keys()
                .filter(|name| !stack_plan.services.contains_key(*name))
                .cloned()
                .collect::<Vec<_>>();
            (existing, removed, reuse_ports)
        })
        .await
        .map_err(|_| AppError::not_found(format!("run {} not found", run_id.as_str())))?;

    if !removed.is_empty() {
        for service_name in &removed {
            let unit_name = crate::app::launch::unit_name_for_run(run_id.as_str(), service_name);
            let _ = app.systemd.stop_unit(&unit_name).await;
        }

        let events = app
            .runs
            .with_run_mut(run_id.as_str(), |run| {
                for service_name in &removed {
                    if let Some(mut service) = run.services.remove(service_name) {
                        service.stop_health_monitor();
                        service.stop_watch();
                    }
                }
                recompute_run_state(run).into_iter().collect::<Vec<_>>()
            })
            .await
            .map_err(AppError::from)?;
        for service_name in &removed {
            release_service_port(
                run_id.as_str(),
                service_name,
                existing
                    .get(service_name)
                    .and_then(|snapshot| snapshot.port),
            )
            .map_err(AppError::from)?;
        }
        app.emit_events(events);
    }

    let mut port_map = resolve_ports_for_refresh(
        run_id.as_str(),
        &stack_plan.services,
        &existing,
        reuse_ports,
    )
    .map_err(AppError::from)?;
    let tasks_map = config
        .tasks
        .as_ref()
        .map(|tasks| tasks.as_map().clone())
        .unwrap_or_default();
    let globals = config.globals_map();
    let global_ports = ensure_globals(app, &globals, &tasks_map, project_dir, config_dir)
        .await
        .map_err(AppError::from)?;

    let mut service_schemes = BTreeMap::new();
    for (name, service) in &stack_plan.services {
        service_schemes.insert(name.clone(), service.scheme());
    }
    for (name, service) in &globals {
        service_schemes.insert(name.clone(), service.scheme());
    }
    for (name, port) in &global_ports {
        port_map.entry(name.clone()).or_insert(*port);
    }

    let scope = InstanceScope::run(run_id.clone(), stack_plan.name.clone());
    let base_env =
        build_base_env(&scope, project_dir, &port_map, &service_schemes).map_err(AppError::from)?;

    let events = app
        .runs
        .with_run_mut(run_id.as_str(), |run| {
            run.base_env = base_env.clone();
            let mut events = Vec::new();
            if matches!(run.state, RunLifecycle::Stopped) {
                run.state = RunLifecycle::Starting;
                run.stopped_at = None;
                events.push(run_state_changed_event(run));
            }
            events
        })
        .await
        .map_err(AppError::from)?;
    app.emit_events(events);

    let snapshot_path = paths::run_snapshot_path(&run_id).map_err(AppError::from)?;
    if let Ok(raw) = std::fs::read(config_path) {
        let _ = atomic_write(&snapshot_path, &raw);
    }

    launch_services(
        app,
        &run_id,
        stack_plan,
        project_dir,
        config_dir,
        &tasks_map,
        &port_map,
        &service_schemes,
        &base_env,
        no_wait,
        force,
        &existing,
    )
    .await?;

    let events = app
        .runs
        .with_run_mut(run_id.as_str(), recompute_run_state)
        .await
        .map_err(AppError::from)?
        .into_iter()
        .collect::<Vec<_>>();
    app.emit_events(events);

    persist_manifest(app, run_id.as_str())
        .await
        .map_err(AppError::from)?;
    run_response(app, run_id.as_str())
        .await
        .map_err(AppError::from)
}

pub async fn down(
    app: &AppContext,
    run_id: &str,
    purge: bool,
) -> AppResult<crate::api::RunResponse> {
    stop_service_handles(app, run_id).await;
    let services = app
        .runs
        .with_run(run_id, |run| {
            run.services
                .iter()
                .map(|(name, service)| (name.clone(), service.launch.port))
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|_| AppError::not_found(format!("run {run_id} not found")))?;

    for (service, _) in &services {
        let unit_name = crate::app::launch::unit_name_for_run(run_id, service);
        let _ = app.systemd.stop_unit(&unit_name).await;
    }
    for (service, port) in &services {
        release_service_port(run_id, service, *port).map_err(AppError::from)?;
    }

    let events = app
        .runs
        .with_run_mut(run_id, |run| {
            let mut events = Vec::new();
            run.state = RunLifecycle::Stopped;
            run.stopped_at = Some(now_rfc3339());
            for (service_name, service) in &mut run.services {
                if let Some(event) =
                    set_service_state(run_id, service_name, service, ServiceState::Stopped)
                {
                    events.push(event);
                }
            }
            events.push(run_state_changed_event(run));
            events
        })
        .await
        .map_err(AppError::from)?;
    app.emit_events(events);

    persist_manifest(app, run_id)
        .await
        .map_err(AppError::from)?;
    let response = run_response(app, run_id).await.map_err(AppError::from)?;
    if purge {
        let run_dir = paths::run_dir(&RunId::new(run_id)).map_err(AppError::from)?;
        let _ = std::fs::remove_dir_all(run_dir);
        let removed = app.runs.remove_run(run_id).await.is_some();
        if removed {
            app.emit_event(run_removed_event(run_id.to_string()));
        }
        let index = app.log_index.clone();
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || index.delete_run(&run_id))
            .await
            .ok();
        let _ = crate::app::runtime::write_daemon_state(app).await;
    }
    Ok(response)
}

pub async fn kill(app: &AppContext, run_id: &str) -> AppResult<crate::api::RunResponse> {
    stop_service_handles(app, run_id).await;
    let services = app
        .runs
        .with_run(run_id, |run| {
            run.services
                .iter()
                .map(|(name, service)| (name.clone(), service.launch.port))
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|_| AppError::not_found(format!("run {run_id} not found")))?;

    for (service, _) in &services {
        let unit_name = crate::app::launch::unit_name_for_run(run_id, service);
        let _ = app.systemd.kill_unit(&unit_name, 9).await;
        let _ = app.systemd.stop_unit(&unit_name).await;
    }
    for (service, port) in &services {
        release_service_port(run_id, service, *port).map_err(AppError::from)?;
    }

    let events = app
        .runs
        .with_run_mut(run_id, |run| {
            let mut events = Vec::new();
            run.state = RunLifecycle::Stopped;
            run.stopped_at = Some(now_rfc3339());
            for (service_name, service) in &mut run.services {
                if let Some(event) =
                    set_service_state(run_id, service_name, service, ServiceState::Stopped)
                {
                    events.push(event);
                }
            }
            events.push(run_state_changed_event(run));
            events
        })
        .await
        .map_err(AppError::from)?;
    app.emit_events(events);

    persist_manifest(app, run_id)
        .await
        .map_err(AppError::from)?;
    run_response(app, run_id).await.map_err(AppError::from)
}

#[allow(clippy::too_many_arguments)]
async fn launch_services(
    app: &AppContext,
    run_id: &RunId,
    stack_plan: &StackPlan,
    project_dir: &Path,
    config_dir: &Path,
    tasks_map: &BTreeMap<String, crate::config::TaskConfig>,
    port_map: &BTreeMap<String, Option<u16>>,
    service_schemes: &BTreeMap<String, String>,
    base_env: &BTreeMap<String, String>,
    no_wait: bool,
    force: bool,
    existing: &BTreeMap<String, ExistingServiceSnapshot>,
) -> AppResult<()> {
    let scope = InstanceScope::run(run_id.clone(), stack_plan.name.clone());

    for service_name in &stack_plan.order {
        let service = stack_plan
            .services
            .get(service_name)
            .ok_or_else(|| AppError::bad_request(format!("service {service_name} missing")))?;
        let prepared = prepare_service(
            &scope,
            project_dir,
            config_dir,
            service_name,
            service,
            port_map,
            service_schemes,
            base_env,
        )
        .map_err(AppError::from)?;

        let needs_restart = force
            || existing.get(service_name).map_or(true, |snapshot| {
                snapshot.watch_hash.as_deref() != Some(prepared.watch_hash.as_str())
                    || matches!(
                        snapshot.state,
                        ServiceState::Stopped | ServiceState::Failed | ServiceState::Degraded
                    )
            });

        if !existing.is_empty() && !needs_restart {
            app.runs
                .with_run_mut(run_id.as_str(), |run| {
                    if let Some(record) = run.services.get_mut(service_name) {
                        apply_prepared_to_runtime(record, &prepared, false);
                    }
                })
                .await
                .map_err(AppError::from)?;
            if let Err(err) =
                sync_service_auto_restart_watcher(app, run_id.as_str(), service_name).await
            {
                eprintln!("devstack: failed to sync watcher for {service_name}: {err}");
            }
            continue;
        }

        if let Some(init_tasks) = &service.init
            && !init_tasks.is_empty()
            && let Err(err) = run_init_tasks_blocking(
                tasks_map.clone(),
                init_tasks.clone(),
                project_dir.to_path_buf(),
                run_id.clone(),
            )
            .await
        {
            eprintln!("[{service_name}] init failed: {err}");
            let record = prepared.clone().into_service_record(
                ServiceState::Failed,
                Some(format!("init task failed: {err}")),
                None,
            );
            let events = app
                .runs
                .with_run_mut(run_id.as_str(), |run| {
                    run.services.insert(service_name.clone(), record);
                    vec![service_state_changed_event(
                        run_id.as_str(),
                        service_name,
                        ServiceState::Failed,
                    )]
                })
                .await
                .map_err(AppError::from)?;
            app.emit_events(events);
            let _ = sync_service_auto_restart_watcher(app, run_id.as_str(), service_name).await;
            continue;
        }

        let restart_existing = existing.contains_key(service_name);
        let start_result = start_prepared_service(app, &scope, &prepared, restart_existing).await;
        if let Some(previous_port) = existing
            .get(service_name)
            .and_then(|snapshot| snapshot.port)
            && Some(previous_port) != prepared.port
        {
            release_service_port(run_id.as_str(), service_name, Some(previous_port))
                .map_err(AppError::from)?;
        }

        let initial_state = if start_result.is_ok() {
            ServiceState::Starting
        } else {
            ServiceState::Failed
        };
        let failure_reason = start_result.as_ref().err().map(|err| err.to_string());
        let last_started_at = start_result.as_ref().ok().map(|_| now_rfc3339());

        let record = prepared.clone().into_service_record(
            initial_state.clone(),
            failure_reason,
            last_started_at,
        );
        let events = app
            .runs
            .with_run_mut(run_id.as_str(), |run| {
                let previous_state = run
                    .services
                    .get(service_name)
                    .map(|record| record.runtime.state.clone());
                run.services.insert(service_name.clone(), record);
                if previous_state.as_ref() != Some(&initial_state) {
                    vec![service_state_changed_event(
                        run_id.as_str(),
                        service_name,
                        initial_state.clone(),
                    )]
                } else {
                    Vec::new()
                }
            })
            .await
            .map_err(AppError::from)?;
        app.emit_events(events);

        if let Err(err) =
            sync_service_auto_restart_watcher(app, run_id.as_str(), service_name).await
        {
            eprintln!("devstack: failed to start watcher for {service_name}: {err}");
        }

        if start_result.is_ok()
            && let Err(err) = crate::app::launch::handle_readiness(
                app.clone(),
                run_id.as_str(),
                &prepared,
                no_wait,
                build_post_init_context(service, tasks_map, project_dir, Some(run_id.clone())),
            )
            .await
        {
            let _ = persist_manifest(app, run_id.as_str()).await;
            return Err(err);
        }
    }

    Ok(())
}

async fn stop_service_handles(app: &AppContext, run_id: &str) {
    let _ = app
        .runs
        .with_run_mut(run_id, |run| {
            for service in run.services.values_mut() {
                service.stop_health_monitor();
                service.stop_watch();
            }
        })
        .await;
}

fn generate_run_id(stack: &str) -> String {
    let mut rng = rand::rng();
    let suffix: String = (0..8)
        .map(|_| format!("{:x}", rng.random_range(0..16)))
        .collect();
    format!("{}-{}", stack, suffix)
}
