use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::app::launch::{build_base_env, prepare_service};
use crate::manifest::RunLifecycle;
use crate::model::{GlobalRecord, InstanceScope, RunRecord};
use crate::paths;
use crate::persistence::{PersistedGlobal, PersistedRun};
use crate::util::atomic_write;

#[derive(Serialize, Deserialize)]
pub struct DaemonStateFile {
    pub runs: Vec<String>,
    pub updated_at: String,
}

pub fn load_state_from_disk() -> Result<BTreeMap<String, RunRecord>> {
    let mut runs = BTreeMap::new();
    let runs_dir = paths::runs_dir()?;
    if !runs_dir.exists() {
        return Ok(runs);
    }

    for entry in std::fs::read_dir(runs_dir)? {
        let entry = entry?;
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }

        let manifest = match PersistedRun::load_from_path(&manifest_path) {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        if manifest.state == RunLifecycle::Stopped || manifest.stopped_at.is_some() {
            continue;
        }

        let snapshot_path = paths::run_snapshot_path(&crate::ids::RunId::new(&manifest.run_id))?;
        if !snapshot_path.exists() {
            continue;
        }
        let config = crate::config::ConfigFile::load_from_path(&snapshot_path)
            .with_context(|| format!("load run snapshot {}", snapshot_path.display()))?;
        let record = convert_manifest_to_run_record(manifest, &config)?;
        runs.insert(record.run_id.as_str().to_string(), record);
    }

    Ok(runs)
}

pub fn load_globals_from_disk() -> Result<BTreeMap<String, GlobalRecord>> {
    let mut globals = BTreeMap::new();
    let globals_root = paths::globals_root()?;
    if !globals_root.exists() {
        return Ok(globals);
    }

    for entry in std::fs::read_dir(globals_root)? {
        let entry = entry?;
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }

        let manifest = match PersistedGlobal::load_from_path(&manifest_path) {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        if manifest.state == RunLifecycle::Stopped || manifest.stopped_at.is_some() {
            continue;
        }

        let config_path = PathBuf::from(&manifest.config_path);
        if !config_path.exists() {
            continue;
        }
        let config = crate::config::ConfigFile::load_from_path(&config_path)
            .with_context(|| format!("load global config {}", config_path.display()))?;
        let record = convert_manifest_to_global_record(manifest, &config)?;
        globals.insert(record.key.clone(), record);
    }

    Ok(globals)
}

fn convert_manifest_to_run_record(
    manifest: PersistedRun,
    config: &crate::config::ConfigFile,
) -> Result<RunRecord> {
    let PersistedRun {
        run_id,
        project_dir,
        config_dir,
        manifest_path: _,
        stack,
        services,
        env,
        state,
        created_at,
        stopped_at,
    } = manifest;

    let run_id = crate::ids::RunId::new(run_id);
    let project_dir = PathBuf::from(project_dir);
    let config_dir = PathBuf::from(config_dir);
    let scope = InstanceScope::run(run_id.clone(), stack.clone());
    let stack_plan = config.stack_plan(&stack)?;

    let mut port_map = services
        .iter()
        .map(|(name, service)| (name.clone(), service.port))
        .collect::<BTreeMap<_, _>>();
    let globals = config.globals_map();
    for (name, port) in global_port_map(&project_dir, &globals)? {
        port_map.entry(name).or_insert(port);
    }

    let mut service_schemes = stack_plan
        .services
        .iter()
        .map(|(name, service)| (name.clone(), service.scheme()))
        .collect::<BTreeMap<_, _>>();
    for (name, service) in &globals {
        service_schemes.insert(name.clone(), service.scheme());
    }

    let mut record = RunRecord::new(
        run_id.clone(),
        stack,
        project_dir.clone(),
        config_dir.clone(),
        env.clone(),
    );
    record.state = state;
    record.created_at = created_at;
    record.stopped_at = stopped_at;

    for service_name in &stack_plan.order {
        let Some(saved_service) = services.get(service_name) else {
            continue;
        };
        let Some(service_config) = stack_plan.services.get(service_name) else {
            continue;
        };

        let mut prepared = prepare_service(
            &scope,
            &project_dir,
            &config_dir,
            service_name,
            service_config,
            &port_map,
            &service_schemes,
            &env,
        )?;
        if let Some(watch_hash) = &saved_service.watch_hash {
            prepared.watch_hash = watch_hash.clone();
        }

        let mut service_record = prepared.into_service_record(
            saved_service.state.clone(),
            saved_service.last_failure.clone(),
            saved_service.last_started_at.clone(),
        );
        service_record.runtime.watch_paused = saved_service.watch_paused;
        record.insert_service(service_name.clone(), service_record);
    }

    Ok(record)
}

fn convert_manifest_to_global_record(
    manifest: PersistedGlobal,
    config: &crate::config::ConfigFile,
) -> Result<GlobalRecord> {
    let PersistedGlobal {
        key,
        name,
        project_dir,
        config_path,
        manifest_path: _,
        service,
        env: _,
        state,
        created_at,
        stopped_at,
    } = manifest;

    let project_dir = PathBuf::from(project_dir);
    let config_path = PathBuf::from(config_path);
    let Some(service_config) = config.globals_map().get(&name).cloned() else {
        anyhow::bail!("global {name} missing from {}", config_path.display());
    };
    let tasks_map = config
        .tasks
        .as_ref()
        .map(|tasks| tasks.as_map().clone())
        .unwrap_or_default();
    let scope = InstanceScope::global(key.clone(), project_dir.clone(), name.clone());
    let port_map = BTreeMap::from([(name.clone(), service.port)]);
    let service_schemes = BTreeMap::from([(name.clone(), service_config.scheme())]);
    let base_env = build_base_env(&scope, &project_dir, &port_map, &service_schemes)?;
    let mut prepared = prepare_service(
        &scope,
        &project_dir,
        config_path.parent().unwrap_or(&project_dir),
        &name,
        &service_config,
        &port_map,
        &service_schemes,
        &base_env,
    )?;
    if let Some(watch_hash) = &service.watch_hash {
        prepared.watch_hash = watch_hash.clone();
    }

    let mut service_record =
        prepared.into_service_record(service.state, service.last_failure, service.last_started_at);
    service_record.runtime.watch_paused = service.watch_paused;

    Ok(GlobalRecord {
        key,
        name,
        project_dir,
        config_path,
        service_config,
        tasks_map,
        service: service_record,
        state,
        created_at,
        stopped_at,
    })
}

fn global_port_map(
    project_dir: &Path,
    globals: &BTreeMap<String, crate::config::ServiceConfig>,
) -> Result<BTreeMap<String, Option<u16>>> {
    let mut ports = BTreeMap::new();
    for (name, service) in globals {
        let manifest_path = paths::global_manifest_path(project_dir, name)?;
        let port = if manifest_path.exists() {
            PersistedGlobal::load_from_path(&manifest_path)
                .ok()
                .map(|manifest| manifest.service.port)
                .unwrap_or_else(|| service.port.as_ref().and_then(|port| port.fixed()))
        } else {
            service.port.as_ref().and_then(|port| port.fixed())
        };
        ports.insert(name.clone(), port);
    }
    Ok(ports)
}

pub fn write_daemon_state_file(runs: &BTreeMap<String, RunRecord>) -> Result<()> {
    let daemon_state = DaemonStateFile {
        runs: runs.keys().cloned().collect(),
        updated_at: crate::util::now_rfc3339(),
    };

    let state_path = paths::daemon_state_path()?;
    let json = serde_json::to_vec_pretty(&daemon_state).context("serialize daemon state")?;
    atomic_write(&state_path, &json).context("write daemon state")
}
