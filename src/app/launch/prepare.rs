use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::config::ServiceConfig;
use crate::ids::ServiceName;
use crate::model::{
    InstanceScope, ReadinessSpec, ServiceLaunchPlan, ServiceRecord, ServiceSpec, ServiceState,
};
use crate::paths;
use crate::port::{allocate_ports, reserve_available_port, reserve_port};
use crate::services::readiness::readiness_url;
use crate::util::sanitize_env_key;
use crate::watch::compute_watch_hash;

use super::context::{
    build_template_context, build_watch_fingerprint, inject_dep_env, load_env_file, merge_env_file,
    render_env, render_patterns, render_template, resolve_cwd_path, resolve_env_file_path,
};

#[derive(Clone, Debug)]
pub struct PreparedService {
    pub name: String,
    pub unit_name: String,
    pub port: Option<u16>,
    pub scheme: String,
    pub url: Option<String>,
    pub deps: Vec<String>,
    pub readiness: ReadinessSpec,
    pub log_path: PathBuf,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub cmd: String,
    pub watch_hash: String,
    pub watch_patterns: Vec<String>,
    pub ignore_patterns: Vec<String>,
    pub watch_extra_files: Vec<PathBuf>,
    pub watch_fingerprint: Vec<u8>,
    pub auto_restart: bool,
}

impl PreparedService {
    pub fn into_service_record(
        self,
        state: ServiceState,
        last_failure: Option<String>,
        last_started_at: Option<String>,
    ) -> ServiceRecord {
        let spec = ServiceSpec {
            name: self.name,
            deps: self.deps,
            readiness: self.readiness,
            auto_restart: self.auto_restart,
            watch_patterns: self.watch_patterns,
            ignore_patterns: self.ignore_patterns,
        };
        let launch = ServiceLaunchPlan {
            unit_name: self.unit_name,
            cwd: self.cwd,
            env: self.env,
            cmd: self.cmd,
            log_path: self.log_path,
            port: self.port,
            scheme: self.scheme,
            url: self.url,
            watch_hash: self.watch_hash,
            watch_fingerprint: self.watch_fingerprint,
            watch_extra_files: self.watch_extra_files,
        };
        let mut record = ServiceRecord::new(spec, launch);
        record.runtime.state = state;
        record.runtime.last_failure = last_failure;
        record.runtime.last_started_at = last_started_at;
        record
    }
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_service(
    scope: &InstanceScope,
    project_dir: &Path,
    config_dir: &Path,
    svc_name: &str,
    svc: &ServiceConfig,
    port_map: &BTreeMap<String, Option<u16>>,
    service_schemes: &BTreeMap<String, String>,
    base_env: &BTreeMap<String, String>,
    global_env: &BTreeMap<String, String>,
    global_env_file: Option<&Path>,
) -> Result<PreparedService> {
    let port = *port_map.get(svc_name).unwrap_or(&None);
    let scheme = svc.scheme();
    let url = port.map(|value| readiness_url(&scheme, value));

    let template_context = build_template_context(scope, project_dir, port_map, service_schemes)?;
    let rendered_cwd = resolve_cwd_path(
        &svc.cwd_or(project_dir).to_string_lossy(),
        &template_context,
        config_dir,
    )?;
    let env_file_path = resolve_env_file_path(svc, &rendered_cwd, &template_context)?;

    let mut env = base_env.clone();

    if let Some(global_env_file_path) = global_env_file {
        let global_file_env = load_env_file(global_env_file_path)?;
        merge_env_file(&mut env, global_file_env);
    }
    let rendered_global_env = render_env(global_env, &template_context)?;
    env.extend(rendered_global_env);

    let file_env = load_env_file(&env_file_path)?;
    merge_env_file(&mut env, file_env);
    inject_dep_env(&mut env, svc, port_map, service_schemes);
    if let Some(port) = port {
        env.insert(svc.port_env(), port.to_string());
    }
    let rendered_env = render_env(&svc.env, &template_context)?;
    env.extend(rendered_env);
    env = crate::config::resolve_env_map(&env);
    env.insert("DEV_GRACE_MS".to_string(), "2000".to_string());

    let readiness = svc.readiness_spec(port.is_some())?;
    let unit_name = unit_name_for_scope(scope, svc_name);
    let log_path = log_path_for_scope(scope, project_dir, svc_name)?;
    let cmd = render_template(&svc.cmd, &template_context)?;

    let rendered_watch = render_patterns(&svc.watch, &template_context)?;
    let rendered_ignore = render_patterns(&svc.ignore, &template_context)?;
    if svc.auto_restart
        && rendered_watch
            .iter()
            .all(|pattern| pattern.trim().is_empty())
    {
        return Err(anyhow!(
            "service {svc_name} sets auto_restart=true but has no watch patterns"
        ));
    }

    let watch_fingerprint = build_watch_fingerprint(
        svc,
        &cmd,
        &rendered_cwd,
        port,
        &scheme,
        &readiness,
        &env_file_path,
        &env,
        &rendered_watch,
        &rendered_ignore,
    )?;
    let watch_patterns = if rendered_watch.is_empty() {
        None
    } else {
        Some(rendered_watch.as_slice())
    };
    let watch_extra_files = vec![env_file_path];
    let watch_hash = compute_watch_hash(
        &rendered_cwd,
        watch_patterns,
        &rendered_ignore,
        &watch_extra_files,
        &watch_fingerprint,
    )?;

    Ok(PreparedService {
        name: svc_name.to_string(),
        unit_name,
        port,
        scheme,
        url,
        deps: svc.deps.clone(),
        readiness,
        log_path,
        cwd: rendered_cwd,
        env,
        cmd,
        watch_hash,
        watch_patterns: rendered_watch,
        ignore_patterns: rendered_ignore,
        watch_extra_files,
        watch_fingerprint,
        auto_restart: svc.auto_restart,
    })
}

#[derive(Clone, Debug)]
pub struct ExistingServiceSnapshot {
    pub watch_hash: Option<String>,
    pub state: ServiceState,
    pub port: Option<u16>,
}

pub fn resolve_ports_for_refresh(
    run_id: &str,
    services: &BTreeMap<String, ServiceConfig>,
    existing: &BTreeMap<String, ExistingServiceSnapshot>,
    reuse_ports: bool,
) -> Result<BTreeMap<String, Option<u16>>> {
    let mut port_map = BTreeMap::new();
    let mut needs_alloc = BTreeMap::new();

    for (name, service) in services {
        let owner = crate::app::runtime::port_owner(run_id, name);
        let port = match &service.port {
            Some(config) if config.is_none() => None,
            Some(crate::config::PortConfig::Fixed(value)) => {
                let existing_port = existing.get(name).and_then(|record| record.port);
                if existing_port == Some(*value) {
                    reserve_port(*value, &owner)?;
                } else {
                    reserve_available_port(*value, &owner)?;
                }
                Some(*value)
            }
            Some(crate::config::PortConfig::None(_)) => None,
            None => {
                if reuse_ports {
                    if let Some(existing_port) = existing.get(name).and_then(|record| record.port) {
                        reserve_port(existing_port, &owner)?;
                        Some(existing_port)
                    } else {
                        needs_alloc.insert(name.clone(), service.clone());
                        None
                    }
                } else {
                    needs_alloc.insert(name.clone(), service.clone());
                    None
                }
            }
        };
        port_map.insert(name.clone(), port);
    }

    if !needs_alloc.is_empty() {
        let allocated = allocate_ports(&needs_alloc, |service| {
            crate::app::runtime::port_owner(run_id, service)
        })?;
        for (name, port) in allocated {
            port_map.insert(name, port);
        }
    }

    Ok(port_map)
}

pub fn unit_name_for_run(run_id: &str, service: &str) -> String {
    let run = sanitize_env_key(run_id);
    let svc = sanitize_env_key(service);
    format!("devstack-run-{run}-{svc}.service")
}

pub fn unit_name_for_global(key: &str, service: &str) -> String {
    let key = sanitize_env_key(key);
    let svc = sanitize_env_key(service);
    format!("devstack-global-{key}-{svc}.service")
}

pub fn unit_name_for_scope(scope: &InstanceScope, service: &str) -> String {
    match scope {
        InstanceScope::Run { run_id, .. } => unit_name_for_run(run_id.as_str(), service),
        InstanceScope::Global { key, .. } => unit_name_for_global(key, service),
    }
}

fn log_path_for_scope(scope: &InstanceScope, project_dir: &Path, service: &str) -> Result<PathBuf> {
    match scope {
        InstanceScope::Run { run_id, .. } => {
            paths::run_log_path(run_id, &ServiceName::new(service.to_string()))
        }
        InstanceScope::Global { name, .. } => paths::global_log_path(project_dir, name),
    }
}
