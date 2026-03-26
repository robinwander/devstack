use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;

use crate::app::context::AppContext;
use crate::app::launch::pipeline::wait_for_prepared_service;
use crate::app::launch::{
    build_base_env, build_post_init_context, prepare_service, start_prepared_service,
    unit_name_for_scope,
};
use crate::app::runtime::{global_port_owner, global_state_changed_event};
use crate::config::{ServiceConfig, TaskConfig};
use crate::manifest::{RunLifecycle, ServiceState};
use crate::model::InstanceScope;
use crate::paths;
use crate::persistence::{PersistedGlobal, PersistedService};
use crate::port::{allocate_ports, reserve_available_port, reserve_port};
use crate::util::now_rfc3339;

pub async fn ensure_globals(
    app: &AppContext,
    globals: &BTreeMap<String, ServiceConfig>,
    tasks_map: &BTreeMap<String, TaskConfig>,
    project_dir: &Path,
    config_dir: &Path,
) -> Result<BTreeMap<String, Option<u16>>> {
    let mut ports = BTreeMap::new();

    for (name, service) in globals {
        let global_dir = paths::global_dir(project_dir, name)?;
        std::fs::create_dir_all(&global_dir)?;
        std::fs::create_dir_all(paths::global_logs_dir(project_dir, name)?)?;

        let key = paths::global_key(project_dir, name)?;
        let scope = InstanceScope::global(key.clone(), project_dir.to_path_buf(), name.clone());
        let unit_name = unit_name_for_scope(&scope, name);
        let manifest_path = paths::global_manifest_path(project_dir, name)?;
        let mut reuse_port = None;
        let mut existing_created_at = None;
        let mut previous_state = None;

        if manifest_path.exists()
            && let Ok(existing) = PersistedGlobal::load_from_path(&manifest_path)
        {
            reuse_port = existing.service.port;
            existing_created_at = Some(existing.created_at.clone());
            previous_state = Some(existing.state.clone());
            let status = app.systemd.unit_status(&unit_name).await.ok().flatten();
            if let Some(status) = status
                && status.active_state == "active"
            {
                if let Some(port) = existing.service.port {
                    reserve_port(port, &global_port_owner(&key, name))?;
                }
                ports.insert(name.clone(), existing.service.port);
                continue;
            }
        }

        let owner = global_port_owner(&key, name);
        let port = match &service.port {
            Some(config) if config.is_none() => None,
            Some(crate::config::PortConfig::Fixed(value)) => {
                reserve_available_port(*value, &owner)?;
                Some(*value)
            }
            Some(crate::config::PortConfig::None(_)) => None,
            None => {
                if let Some(port) = reuse_port {
                    reserve_port(port, &owner)?;
                    Some(port)
                } else {
                    let mut service_map = BTreeMap::new();
                    service_map.insert(name.clone(), service.clone());
                    let allocated = allocate_ports(&service_map, |_| owner.clone())?;
                    *allocated.get(name).unwrap_or(&None)
                }
            }
        };

        let port_map = BTreeMap::from([(name.clone(), port)]);
        let service_schemes = BTreeMap::from([(name.clone(), service.scheme())]);
        let base_env = build_base_env(&scope, project_dir, &port_map, &service_schemes)?;
        let prepared = prepare_service(
            &scope,
            project_dir,
            config_dir,
            name,
            service,
            &port_map,
            &service_schemes,
            &base_env,
        )?;

        let mut service_state = ServiceState::Ready;
        let mut lifecycle = RunLifecycle::Running;
        let mut last_failure = None;
        let mut last_started_at = None;

        match start_prepared_service(app, &scope, &prepared, false).await {
            Ok(()) => {
                last_started_at = Some(now_rfc3339());
                if let Err(err) = wait_for_prepared_service(
                    app,
                    name,
                    &prepared,
                    build_post_init_context(service, tasks_map, project_dir, None),
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
            }
        }

        let manifest = PersistedGlobal {
            key: key.clone(),
            name: name.clone(),
            project_dir: project_dir.to_string_lossy().to_string(),
            manifest_path: manifest_path.to_string_lossy().to_string(),
            service: PersistedService {
                port: prepared.port,
                url: prepared.url.clone(),
                state: service_state,
                watch_hash: Some(prepared.watch_hash.clone()),
                last_failure: last_failure.clone(),
                last_started_at,
                watch_paused: false,
            },
            env: prepared.env.clone(),
            state: lifecycle,
            created_at: existing_created_at.unwrap_or_else(now_rfc3339),
            stopped_at: None,
        };
        manifest.write_to_path(&manifest_path)?;
        if previous_state.as_ref() != Some(&manifest.state) {
            app.emit_event(global_state_changed_event(&key, manifest.state.clone()));
        }

        if let Some(last_failure) = last_failure {
            return Err(anyhow::anyhow!(last_failure));
        }

        ports.insert(name.clone(), prepared.port);
    }

    Ok(ports)
}
