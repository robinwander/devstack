use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::ServiceConfig;
use crate::model::InstanceScope;
use crate::services::readiness::{ReadinessSpec, readiness_url};
use crate::util::{expand_home, sanitize_env_key};

pub fn build_base_env(
    scope: &InstanceScope,
    project_dir: &Path,
    port_map: &BTreeMap<String, Option<u16>>,
    schemes: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    match scope {
        InstanceScope::Run { run_id, stack } => {
            env.insert("DEV_RUN_ID".to_string(), run_id.as_str().to_string());
            env.insert("DEV_STACK".to_string(), stack.clone());
        }
        InstanceScope::Global { key, name, .. } => {
            env.insert("DEV_STACK".to_string(), "globals".to_string());
            env.insert("DEV_GLOBAL_KEY".to_string(), key.clone());
            env.insert("DEV_GLOBAL_NAME".to_string(), name.clone());
        }
    }
    env.insert(
        "DEV_PROJECT_DIR".to_string(),
        project_dir.to_string_lossy().to_string(),
    );

    for (service, port) in port_map {
        if let Some(port) = port {
            let key = sanitize_env_key(service);
            env.insert(format!("DEV_PORT_{key}"), port.to_string());
            let scheme = schemes
                .get(service)
                .cloned()
                .unwrap_or_else(|| "http".to_string());
            env.insert(format!("DEV_URL_{key}"), readiness_url(&scheme, *port));
        }
    }

    Ok(env)
}

pub fn inject_dep_env(
    env: &mut BTreeMap<String, String>,
    svc: &ServiceConfig,
    port_map: &BTreeMap<String, Option<u16>>,
    schemes: &BTreeMap<String, String>,
) {
    for dep in &svc.deps {
        if let Some(Some(port)) = port_map.get(dep) {
            let key = sanitize_env_key(dep);
            env.insert(format!("DEV_DEP_{key}_PORT"), port.to_string());
            let scheme = schemes
                .get(dep)
                .cloned()
                .unwrap_or_else(|| "http".to_string());
            env.insert(format!("DEV_DEP_{key}_URL"), readiness_url(&scheme, *port));
        }
    }
}

pub fn build_template_context(
    scope: &InstanceScope,
    project_dir: &Path,
    port_map: &BTreeMap<String, Option<u16>>,
    schemes: &BTreeMap<String, String>,
) -> Result<serde_json::Value> {
    let mut services = serde_json::Map::new();
    for (service, port) in port_map {
        let mut entry = serde_json::Map::new();
        if let Some(port) = port {
            entry.insert("port".to_string(), serde_json::json!(port));
            let scheme = schemes
                .get(service)
                .cloned()
                .unwrap_or_else(|| "http".to_string());
            entry.insert(
                "url".to_string(),
                serde_json::json!(readiness_url(&scheme, *port)),
            );
        } else {
            entry.insert("port".to_string(), serde_json::Value::Null);
            entry.insert("url".to_string(), serde_json::Value::Null);
        }
        services.insert(service.clone(), serde_json::Value::Object(entry));
    }

    let (run_id, stack_name, global) = match scope {
        InstanceScope::Run { run_id, stack } => (
            serde_json::json!(run_id.as_str()),
            stack.clone(),
            serde_json::Value::Null,
        ),
        InstanceScope::Global { key, name, .. } => (
            serde_json::Value::Null,
            "globals".to_string(),
            serde_json::json!({ "key": key, "name": name }),
        ),
    };

    Ok(serde_json::json!({
        "run": { "id": run_id },
        "global": global,
        "project": { "dir": project_dir.to_string_lossy() },
        "stack": { "name": stack_name },
        "services": services,
    }))
}

pub fn render_env(
    env: &BTreeMap<String, String>,
    ctx: &serde_json::Value,
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (key, value) in env {
        let rendered = render_template(value, ctx)?;
        out.insert(key.clone(), rendered);
    }
    Ok(out)
}

pub fn render_template(template: &str, ctx: &serde_json::Value) -> Result<String> {
    let mut env = minijinja::Environment::new();
    env.set_trim_blocks(true);
    let tmpl = env.template_from_str(template)?;
    Ok(tmpl.render(ctx)?)
}

pub fn render_patterns(patterns: &[String], ctx: &serde_json::Value) -> Result<Vec<String>> {
    let mut rendered = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        rendered.push(render_template(pattern, ctx)?);
    }
    Ok(rendered)
}

pub fn resolve_rendered_path(template: &str, ctx: &serde_json::Value) -> Result<PathBuf> {
    let rendered = render_template(template, ctx)?;
    Ok(expand_home(&PathBuf::from(rendered)))
}

pub fn resolve_cwd_path(
    template: &str,
    ctx: &serde_json::Value,
    base_dir: &Path,
) -> Result<PathBuf> {
    let rendered = resolve_rendered_path(template, ctx)?;
    if rendered.is_absolute() {
        Ok(rendered)
    } else {
        Ok(base_dir.join(rendered))
    }
}

pub fn resolve_env_file_path(
    svc: &ServiceConfig,
    cwd: &Path,
    ctx: &serde_json::Value,
) -> Result<PathBuf> {
    if let Some(env_file) = &svc.env_file {
        let rendered = resolve_rendered_path(&env_file.to_string_lossy(), ctx)?;
        if rendered.is_absolute() {
            return Ok(rendered);
        }
        return Ok(cwd.join(rendered));
    }
    Ok(cwd.join(".env"))
}

pub fn load_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let iter = dotenvy::from_path_iter(path)
        .with_context(|| format!("read env file {}", path.to_string_lossy()))?;
    let mut env = BTreeMap::new();
    for item in iter {
        let (key, value) = item?;
        env.insert(key, value);
    }
    Ok(env)
}

pub fn merge_env_file(into: &mut BTreeMap<String, String>, file_env: BTreeMap<String, String>) {
    for (key, value) in file_env {
        if key.starts_with("DEV_") {
            continue;
        }
        into.insert(key, value);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_watch_fingerprint(
    svc: &ServiceConfig,
    rendered_cmd: &str,
    rendered_cwd: &Path,
    port: Option<u16>,
    scheme: &str,
    readiness: &ReadinessSpec,
    env_file_path: &Path,
    env: &BTreeMap<String, String>,
    watch: &[String],
    ignore: &[String],
) -> Result<Vec<u8>> {
    let payload = serde_json::json!({
        "cmd": rendered_cmd,
        "cwd": rendered_cwd.to_string_lossy(),
        "port": port,
        "scheme": scheme,
        "deps": svc.deps,
        "port_env": svc.port_env(),
        "readiness": format!("{:?}", readiness),
        "env_file": env_file_path.to_string_lossy(),
        "env": env,
        "watch": watch,
        "ignore": ignore,
    });
    Ok(serde_json::to_vec(&payload)?)
}
