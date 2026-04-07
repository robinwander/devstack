use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::app::context::AppContext;
use crate::app::launch::pipeline::wait_for_prepared_service;
use crate::app::launch::{
    PreparedService, build_base_env, build_post_init_context, prepare_service,
    start_prepared_service, sync_global_auto_restart_watcher, unit_name_for_scope,
};
use crate::app::runtime::{global_port_owner, global_state_changed_event, persist_global_manifest};
use crate::config::{ServiceConfig, TaskConfig};
use crate::model::{GlobalRecord, InstanceScope};
use crate::model::{RunLifecycle, ServiceState};
use crate::paths;
use crate::persistence::PersistedGlobal;
use crate::port::{allocate_ports, reserve_available_port, reserve_port};
use crate::util::now_rfc3339;

pub async fn ensure_globals(
    app: &AppContext,
    globals: &BTreeMap<String, ServiceConfig>,
    tasks_map: &BTreeMap<String, TaskConfig>,
    project_dir: &Path,
    config_path: &Path,
    config_dir: &Path,
) -> Result<BTreeMap<String, Option<u16>>> {
    let mut ports = BTreeMap::new();

    for (name, service_config) in globals {
        let global_dir = paths::global_dir(project_dir, name)?;
        std::fs::create_dir_all(&global_dir)?;
        std::fs::create_dir_all(paths::global_logs_dir(project_dir, name)?)?;

        let key = paths::global_key(project_dir, name)?;
        let scope = InstanceScope::global(key.clone(), project_dir.to_path_buf(), name.clone());
        let unit_name = unit_name_for_scope(&scope, name);
        let manifest_path = paths::global_manifest_path(project_dir, name)?;
        let existing = if manifest_path.exists() {
            PersistedGlobal::load_from_path(&manifest_path).ok()
        } else {
            None
        };

        let status = app.systemd.unit_status(&unit_name).await.ok().flatten();
        let active_existing = status
            .as_ref()
            .is_some_and(|status| status.active_state == "active");

        let owner = global_port_owner(&key, name);
        let port = resolve_global_port(
            name,
            service_config,
            &owner,
            existing.as_ref(),
            active_existing,
        )?;
        let prepared =
            prepare_global_service(&scope, service_config, project_dir, config_dir, port)?;

        let existing_created_at = existing
            .as_ref()
            .map(|manifest| manifest.created_at.clone())
            .unwrap_or_else(now_rfc3339);
        let previous_state = existing.as_ref().map(|manifest| manifest.state.clone());

        if active_existing {
            app.globals
                .upsert_global(build_global_record(
                    key.clone(),
                    name.clone(),
                    project_dir.to_path_buf(),
                    config_path.to_path_buf(),
                    service_config.clone(),
                    tasks_map.clone(),
                    prepared.clone(),
                    ServiceState::Ready,
                    None,
                    existing
                        .as_ref()
                        .and_then(|manifest| manifest.service.last_started_at.clone())
                        .or_else(|| Some(now_rfc3339())),
                    existing
                        .as_ref()
                        .map(|manifest| manifest.service.watch_paused)
                        .unwrap_or(false),
                    RunLifecycle::Running,
                    existing_created_at,
                    None,
                ))
                .await;
            persist_global_manifest(app, &key).await?;
            sync_global_auto_restart_watcher(app, &key).await?;
            if previous_state.as_ref() != Some(&RunLifecycle::Running) {
                app.emit_event(global_state_changed_event(&key, RunLifecycle::Running));
            }
            ports.insert(name.clone(), prepared.port);
            continue;
        }

        let mut service_state = ServiceState::Ready;
        let mut lifecycle = RunLifecycle::Running;
        let mut last_failure = None;
        let mut last_started_at = Some(now_rfc3339());

        match start_prepared_service(app, &scope, &prepared, false).await {
            Ok(()) => {
                if let Err(err) = wait_for_prepared_service(
                    app,
                    name,
                    &prepared,
                    build_post_init_context(
                        service_config,
                        tasks_map,
                        project_dir,
                        None,
                        prepared.env.clone(),
                        None,
                    ),
                )
                .await
                {
                    service_state = ServiceState::Failed;
                    lifecycle = RunLifecycle::Degraded;
                    last_failure = Some(
                        err.context(format!("launch global service '{name}'"))
                            .to_string(),
                    );
                    last_started_at = None;
                }
            }
            Err(err) => {
                service_state = ServiceState::Failed;
                lifecycle = RunLifecycle::Degraded;
                last_failure = Some(
                    err.context(format!("start global service '{name}'"))
                        .to_string(),
                );
                last_started_at = None;
            }
        }

        app.globals
            .upsert_global(build_global_record(
                key.clone(),
                name.clone(),
                project_dir.to_path_buf(),
                config_path.to_path_buf(),
                service_config.clone(),
                tasks_map.clone(),
                prepared.clone(),
                service_state,
                last_failure.clone(),
                last_started_at.clone(),
                false,
                lifecycle,
                existing_created_at,
                None,
            ))
            .await;
        persist_global_manifest(app, &key).await?;
        sync_global_auto_restart_watcher(app, &key).await?;

        let manifest = app
            .globals
            .get_global(&key)
            .await
            .expect("global record persisted");
        if previous_state.as_ref() != Some(&manifest.state) {
            app.emit_event(global_state_changed_event(&key, manifest.state.clone()));
        }

        if let Some(last_failure) = last_failure {
            return Err(anyhow!(last_failure));
        }

        ports.insert(name.clone(), prepared.port);
    }

    Ok(ports)
}

pub async fn restart_global_no_wait(app: &AppContext, key: &str) -> Result<()> {
    let (prepared, service_config, tasks_map, project_dir, unit_name) = app
        .globals
        .with_global_mut(key, |global| {
            global.service.runtime.state = ServiceState::Starting;
            global.service.runtime.last_failure = None;
            global.state = RunLifecycle::Starting;
            global.service.runtime.last_started_at = Some(now_rfc3339());
            (
                prepared_service_from_global(global),
                global.service_config.clone(),
                global.tasks_map.clone(),
                global.project_dir.clone(),
                global.service.launch.unit_name.clone(),
            )
        })
        .await?;

    persist_global_manifest(app, key).await?;
    app.emit_event(global_state_changed_event(key, RunLifecycle::Starting));

    app.systemd.restart_unit(&unit_name).await?;

    let app = app.clone();
    let key = key.to_string();
    let service_name = prepared.name.clone();
    tokio::spawn(async move {
        let result = wait_for_prepared_service(
            &app,
            &service_name,
            &prepared,
            build_post_init_context(
                &service_config,
                &tasks_map,
                &project_dir,
                None,
                prepared.env.clone(),
                None,
            ),
        )
        .await;

        let _ = app
            .globals
            .with_global_mut(&key, |global| match result {
                Ok(()) => {
                    global.service.runtime.state = ServiceState::Ready;
                    global.service.runtime.last_failure = None;
                    global.state = RunLifecycle::Running;
                }
                Err(err) => {
                    global.service.runtime.state = ServiceState::Failed;
                    global.service.runtime.last_failure = Some(err.to_string());
                    global.state = RunLifecycle::Degraded;
                }
            })
            .await;
        let _ = persist_global_manifest(&app, &key).await;
        if let Some(global) = app.globals.get_global(&key).await {
            app.emit_event(global_state_changed_event(&key, global.state));
        }
    });

    Ok(())
}

fn resolve_global_port(
    name: &str,
    service: &ServiceConfig,
    owner: &str,
    existing: Option<&PersistedGlobal>,
    active_existing: bool,
) -> Result<Option<u16>> {
    let reuse_port = existing.and_then(|manifest| manifest.service.port);
    match &service.port {
        Some(config) if config.is_none() => Ok(None),
        Some(crate::config::PortConfig::Fixed(value)) => {
            if active_existing && reuse_port == Some(*value) {
                reserve_port(*value, owner)?;
            } else {
                reserve_available_port(*value, owner)?;
            }
            Ok(Some(*value))
        }
        Some(crate::config::PortConfig::None(_)) => Ok(None),
        None => {
            if let Some(port) = reuse_port {
                reserve_port(port, owner)?;
                Ok(Some(port))
            } else {
                let mut service_map = BTreeMap::new();
                service_map.insert(name.to_string(), service.clone());
                let allocated = allocate_ports(&service_map, |_| owner.to_string())?;
                Ok(*allocated.get(name).unwrap_or(&None))
            }
        }
    }
}

fn prepare_global_service(
    scope: &InstanceScope,
    service_config: &ServiceConfig,
    project_dir: &Path,
    config_dir: &Path,
    port: Option<u16>,
) -> Result<PreparedService> {
    let name = match scope {
        InstanceScope::Global { name, .. } => name.clone(),
        InstanceScope::Run { .. } => unreachable!("global scope required"),
    };
    let port_map = BTreeMap::from([(name.clone(), port)]);
    let service_schemes = BTreeMap::from([(name.clone(), service_config.scheme())]);
    let base_env = build_base_env(scope, project_dir, &port_map, &service_schemes)?;
    prepare_service(
        scope,
        project_dir,
        config_dir,
        &name,
        service_config,
        &port_map,
        &service_schemes,
        &base_env,
        &BTreeMap::new(),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_global_record(
    key: String,
    name: String,
    project_dir: PathBuf,
    config_path: PathBuf,
    service_config: ServiceConfig,
    tasks_map: BTreeMap<String, TaskConfig>,
    prepared: PreparedService,
    state: ServiceState,
    last_failure: Option<String>,
    last_started_at: Option<String>,
    watch_paused: bool,
    lifecycle: RunLifecycle,
    created_at: String,
    stopped_at: Option<String>,
) -> GlobalRecord {
    let mut service = prepared.into_service_record(state, last_failure, last_started_at);
    service.runtime.watch_paused = watch_paused;
    GlobalRecord {
        key,
        name,
        project_dir,
        config_path,
        service_config,
        tasks_map,
        service,
        state: lifecycle,
        created_at,
        stopped_at,
    }
}

fn prepared_service_from_global(global: &GlobalRecord) -> PreparedService {
    PreparedService {
        name: global.service.spec.name.clone(),
        unit_name: global.service.launch.unit_name.clone(),
        port: global.service.launch.port,
        scheme: global.service.launch.scheme.clone(),
        url: global.service.launch.url.clone(),
        deps: global.service.spec.deps.clone(),
        readiness: global.service.spec.readiness.clone(),
        log_path: global.service.launch.log_path.clone(),
        cwd: global.service.launch.cwd.clone(),
        env: global.service.launch.env.clone(),
        cmd: global.service.launch.cmd.clone(),
        watch_hash: global.service.launch.watch_hash.clone(),
        watch_patterns: global.service.spec.watch_patterns.clone(),
        ignore_patterns: global.service.spec.ignore_patterns.clone(),
        watch_extra_files: global.service.launch.watch_extra_files.clone(),
        watch_fingerprint: global.service.launch.watch_fingerprint.clone(),
        auto_restart: global.service.spec.auto_restart,
    }
}
