use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, anyhow};

use crate::app::commands::tasks::run_post_init_tasks_blocking;
use crate::app::context::AppContext;
use crate::app::launch::{
    build_base_env, build_post_init_context, build_template_context, render_env, render_template,
    resolve_cwd_path, resolve_env_file_path, unit_name_for_run,
};
use crate::app::runtime::{global_state_changed_event, port_owner};
use crate::config::{ServiceConfig, TaskConfig};
use crate::ids::RunId;
use crate::manifest::{RunLifecycle, RunManifest, ServiceManifest, ServiceState};
use crate::paths;
use crate::port::{allocate_ports, reserve_available_port, reserve_port};
use crate::services::readiness::{ReadinessContext, ReadinessKind, readiness_url};
use crate::systemd::{ExecStart, UnitProperties};
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
        let log_path = paths::global_log_path(project_dir, name)?;
        std::fs::create_dir_all(paths::global_logs_dir(project_dir, name)?)?;

        let key = paths::global_key(project_dir, name)?;
        let run_id = RunId::new(format!("global-{key}"));
        let unit_name = unit_name_for_run(run_id.as_str(), name);
        let manifest_path = paths::global_manifest_path(project_dir, name)?;
        let mut reuse_port = None;
        let mut previous_lifecycle = None;

        if manifest_path.exists()
            && let Ok(existing) = RunManifest::load_from_path(&manifest_path)
            && let Some(existing_service) = existing.services.get(name)
        {
            reuse_port = existing_service.port;
            previous_lifecycle = Some(existing.state.clone());
            let status = app.systemd.unit_status(&unit_name).await.ok().flatten();
            if let Some(status) = status
                && status.active_state == "active"
            {
                if let Some(port) = existing_service.port {
                    reserve_port(port, &port_owner(run_id.as_str(), name))?;
                }
                ports.insert(name.clone(), existing_service.port);
                continue;
            }
        }

        let owner = port_owner(run_id.as_str(), name);
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
        let scheme = service.scheme();
        let url = port.map(|value| readiness_url(&scheme, value));

        let template_context = build_template_context(
            &run_id,
            "globals",
            project_dir,
            &BTreeMap::from([(name.clone(), port)]),
            &BTreeMap::from([(name.clone(), scheme.clone())]),
        )?;
        let rendered_cwd = resolve_cwd_path(
            &service.cwd_or(project_dir).to_string_lossy(),
            &template_context,
            config_dir,
        )?;
        let env_file_path = resolve_env_file_path(service, &rendered_cwd, &template_context)?;
        let mut env = build_base_env(
            &run_id,
            "globals",
            project_dir,
            &BTreeMap::from([(name.clone(), port)]),
            &BTreeMap::from([(name.clone(), scheme.clone())]),
        )?;
        let file_env = super::super::launch::load_env_file(&env_file_path)?;
        super::super::launch::merge_env_file(&mut env, file_env);
        if let Some(port) = port {
            env.insert(service.port_env(), port.to_string());
        }
        let rendered_env = render_env(&service.env, &template_context)?;
        env.extend(rendered_env);
        env = crate::config::resolve_env_map(&env);
        env.insert("DEV_GRACE_MS".to_string(), "2000".to_string());

        let binary = app.binary_path.to_string_lossy().to_string();
        let exec = ExecStart {
            path: binary.clone(),
            argv: vec![
                binary.clone(),
                "__shim".to_string(),
                "--run-id".to_string(),
                run_id.as_str().to_string(),
                "--service".to_string(),
                name.clone(),
                "--cmd".to_string(),
                render_template(&service.cmd, &template_context)?,
                "--cwd".to_string(),
                rendered_cwd.to_string_lossy().to_string(),
                "--log-file".to_string(),
                log_path.to_string_lossy().to_string(),
            ],
            ignore_failure: false,
        };
        let readiness = service.readiness_spec(port.is_some())?;
        let properties = UnitProperties::new(
            format!("devstack global {}", name),
            &rendered_cwd,
            env.iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
            exec,
        )
        .with_restart("no")
        .with_remain_after_exit(matches!(readiness.kind, ReadinessKind::Exit));
        let context = ReadinessContext {
            port,
            scheme: scheme.clone(),
            log_path: log_path.clone(),
            cwd: rendered_cwd.clone(),
            env: env.clone(),
            unit_name: Some(unit_name.clone()),
            systemd: Some(app.systemd.clone()),
        };
        let post_init = build_post_init_context(service, tasks_map, project_dir, &run_id);

        let mut service_state = ServiceState::Ready;
        let mut lifecycle = RunLifecycle::Running;
        let mut startup_error: Option<anyhow::Error> = None;

        match app
            .systemd
            .start_transient_service(&unit_name, properties)
            .await
        {
            Ok(()) => {
                if let Err(err) = crate::readiness::wait_for_ready(&readiness, &context).await {
                    service_state = ServiceState::Failed;
                    lifecycle = RunLifecycle::Degraded;
                    startup_error =
                        Some(anyhow!("global service '{name}' failed readiness: {err}"));
                } else if let Some(post_init) = post_init
                    && let Err(err) = run_post_init_tasks_blocking(
                        post_init.tasks_map,
                        post_init.post_init_tasks,
                        post_init.project_dir,
                        post_init.run_id,
                    )
                    .await
                {
                    service_state = ServiceState::Failed;
                    lifecycle = RunLifecycle::Degraded;
                    startup_error =
                        Some(anyhow!("global service '{name}' post_init failed: {err}"));
                }
            }
            Err(err) => {
                service_state = ServiceState::Failed;
                lifecycle = RunLifecycle::Degraded;
                startup_error = Some(err.context(format!("start global service '{name}'")));
            }
        }

        let manifest = RunManifest {
            run_id: run_id.as_str().to_string(),
            project_dir: project_dir.to_string_lossy().to_string(),
            stack: "globals".to_string(),
            manifest_path: manifest_path.to_string_lossy().to_string(),
            services: BTreeMap::from([(
                name.clone(),
                ServiceManifest {
                    port,
                    url: url.clone(),
                    state: service_state,
                    watch_hash: None,
                },
            )]),
            env: env.clone(),
            state: lifecycle,
            created_at: now_rfc3339(),
            stopped_at: None,
        };
        manifest.write_to_path(&manifest_path)?;
        if previous_lifecycle.as_ref() != Some(&manifest.state) {
            app.emit_event(global_state_changed_event(&key, manifest.state.clone()));
        }

        if let Some(err) = startup_error {
            return Err(err);
        }

        ports.insert(name.clone(), port);
    }

    Ok(ports)
}
