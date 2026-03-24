use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::readiness::ReadinessKind;
use crate::util::validate_name_for_path_component;

#[derive(Clone, Debug)]
pub struct UniqueMap<K, V>(pub BTreeMap<K, V>);

impl<K, V> UniqueMap<K, V> {
    pub fn into_map(self) -> BTreeMap<K, V> {
        self.0
    }

    pub fn as_map(&self) -> &BTreeMap<K, V> {
        &self.0
    }
}

impl<'de, K, V> Deserialize<'de> for UniqueMap<K, V>
where
    K: Deserialize<'de> + Ord + std::fmt::Debug,
    V: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UniqueMapVisitor<K, V> {
            marker: std::marker::PhantomData<(K, V)>,
        }

        impl<'de, K, V> Visitor<'de> for UniqueMapVisitor<K, V>
        where
            K: Deserialize<'de> + Ord + std::fmt::Debug,
            V: Deserialize<'de>,
        {
            type Value = UniqueMap<K, V>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a map with unique keys")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<K, V>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate key {:?}", key)));
                    }
                    values.insert(key, value);
                }
                Ok(UniqueMap(values))
            }
        }

        deserializer.deserialize_map(UniqueMapVisitor {
            marker: std::marker::PhantomData,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ConfigFile {
    pub version: u8,
    pub stacks: UniqueMap<String, StackConfig>,
    #[serde(default)]
    pub default_stack: Option<String>,
    #[serde(default)]
    pub globals: Option<UniqueMap<String, ServiceConfig>>,
    #[serde(default)]
    pub tasks: Option<UniqueMap<String, TaskConfig>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StackConfig {
    pub services: UniqueMap<String, ServiceConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServiceConfig {
    pub cmd: String,
    #[serde(default)]
    pub deps: Vec<String>,
    pub scheme: Option<String>,
    pub port_env: Option<String>,
    pub port: Option<PortConfig>,
    pub readiness: Option<ReadinessConfig>,
    pub env_file: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub watch: Vec<String>,
    #[serde(default)]
    pub ignore: Vec<String>,
    #[serde(default)]
    pub auto_restart: bool,
    #[serde(default)]
    pub init: Option<Vec<String>>,
    #[serde(default)]
    pub post_init: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum TaskConfig {
    Command(String),
    Structured(TaskDefinition),
}

#[derive(Clone, Debug, Deserialize)]
pub struct TaskDefinition {
    pub cmd: String,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub watch: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub env_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum PortConfig {
    None(String),
    Fixed(u16),
}

impl PortConfig {
    pub fn is_none(&self) -> bool {
        matches!(self, PortConfig::None(value) if value == "none")
    }

    pub fn fixed(&self) -> Option<u16> {
        match self {
            PortConfig::Fixed(value) => Some(*value),
            PortConfig::None(value) if value == "none" => None,
            PortConfig::None(_) => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReadinessConfig {
    pub tcp: Option<ReadinessTcp>,
    pub http: Option<ReadinessHttp>,
    pub log_regex: Option<String>,
    pub cmd: Option<String>,
    pub delay_ms: Option<u64>,
    pub exit: Option<ReadinessExit>,
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReadinessTcp {}

#[derive(Clone, Debug, Deserialize)]
pub struct ReadinessHttp {
    pub path: String,
    pub expect_status: Option<Vec<u16>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReadinessExit {}

#[derive(Clone, Debug)]
pub struct StackPlan {
    pub name: String,
    pub services: BTreeMap<String, ServiceConfig>,
    pub order: Vec<String>,
}

impl ConfigFile {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = std::fs::read(path).with_context(|| format!("read config {path:?}"))?;
        let config: ConfigFile = match path.extension().and_then(|s| s.to_str()) {
            Some("toml") => {
                let text = std::str::from_utf8(&raw).context("read toml as utf-8")?;
                toml::from_str(text).context("parse toml")?
            }
            Some("yaml") | Some("yml") => serde_yaml::from_slice(&raw).context("parse yaml")?,
            _ => {
                // Try YAML first, then TOML for backwards compatibility.
                match serde_yaml::from_slice(&raw) {
                    Ok(cfg) => cfg,
                    Err(_) => {
                        let text = std::str::from_utf8(&raw).context("read toml as utf-8")?;
                        toml::from_str(text).context("parse toml")?
                    }
                }
            }
        };
        if config.version != 1 {
            return Err(anyhow!("unsupported config version {}", config.version));
        }
        config.validate()?;
        Ok(config)
    }

    pub fn stack_plan(&self, name: &str) -> Result<StackPlan> {
        let stack = self
            .stacks
            .as_map()
            .get(name)
            .ok_or_else(|| anyhow!("stack {name} not found"))?;
        let services = stack.services.as_map().clone();
        let order = topo_sort(&services)?;
        Ok(StackPlan {
            name: name.to_string(),
            services,
            order,
        })
    }

    pub fn globals_map(&self) -> BTreeMap<String, ServiceConfig> {
        self.globals
            .as_ref()
            .map(|map| map.as_map().clone())
            .unwrap_or_default()
    }

    pub fn default_path(project_dir: &Path) -> PathBuf {
        let toml = project_dir.join("devstack.toml");
        if toml.exists() {
            return toml;
        }
        let yaml = project_dir.join("devstack.yml");
        if yaml.exists() {
            return yaml;
        }
        let yaml_alt = project_dir.join("devstack.yaml");
        if yaml_alt.exists() {
            return yaml_alt;
        }
        toml
    }

    pub fn find_nearest_path(start_dir: &Path) -> Option<PathBuf> {
        let mut current = Some(start_dir);
        while let Some(dir) = current {
            let toml = dir.join("devstack.toml");
            if toml.is_file() {
                return Some(toml);
            }
            let yaml = dir.join("devstack.yml");
            if yaml.is_file() {
                return Some(yaml);
            }
            let yaml_alt = dir.join("devstack.yaml");
            if yaml_alt.is_file() {
                return Some(yaml_alt);
            }
            current = dir.parent();
        }
        None
    }

    fn validate(&self) -> Result<()> {
        if let Some(default_stack) = &self.default_stack
            && !self.stacks.as_map().contains_key(default_stack)
        {
            return Err(anyhow!(
                "default_stack '{default_stack}' not found in stacks"
            ));
        }
        for (stack_name, stack) in self.stacks.as_map() {
            let services = stack.services.as_map();
            for (svc_name, svc) in services {
                validate_name_for_path_component("service", svc_name).map_err(|err| {
                    anyhow!("invalid service name in stack '{stack_name}': {err}")
                })?;
                validate_service_port(stack_name, svc_name, svc)?;
                validate_service_readiness(stack_name, svc_name, svc)?;
                validate_service_init_tasks(stack_name, svc_name, svc, self.tasks.as_ref())?;
                validate_service_post_init_tasks(stack_name, svc_name, svc, self.tasks.as_ref())?;
                validate_service_auto_restart(stack_name, svc_name, svc)?;
                // Validate deps reference existing services in this stack.
                for dep in &svc.deps {
                    if !services.contains_key(dep) {
                        return Err(anyhow!(
                            "service '{svc_name}' in stack '{stack_name}' depends on \
                             unknown service '{dep}'"
                        ));
                    }
                }
            }
            // Validate no circular dependencies via topological sort.
            topo_sort(services).map_err(|err| anyhow!("stack '{stack_name}': {err}"))?;
        }
        if let Some(globals) = &self.globals {
            for (svc_name, svc) in globals.as_map() {
                validate_name_for_path_component("service", svc_name)
                    .map_err(|err| anyhow!("invalid global service name: {err}"))?;
                validate_service_port("globals", svc_name, svc)?;
                validate_service_readiness("globals", svc_name, svc)?;
                validate_service_post_init_tasks("globals", svc_name, svc, self.tasks.as_ref())?;
                validate_service_auto_restart("globals", svc_name, svc)?;
            }
        }
        if let Some(tasks) = &self.tasks {
            for task_name in tasks.as_map().keys() {
                validate_name_for_path_component("task", task_name)
                    .map_err(|err| anyhow!("invalid task name: {err}"))?;
            }
        }
        Ok(())
    }
}

fn validate_service_port(stack: &str, service: &str, svc: &ServiceConfig) -> Result<()> {
    if let Some(PortConfig::None(value)) = &svc.port
        && value != "none"
    {
        return Err(anyhow!(
            "invalid port value '{}' for {stack}.{service} (use integer or 'none')",
            value
        ));
    }
    Ok(())
}

fn validate_service_readiness(stack: &str, service: &str, svc: &ServiceConfig) -> Result<()> {
    let has_port = !matches!(svc.port, Some(PortConfig::None(_)));
    svc.readiness_kind(has_port)
        .map_err(|err| anyhow!("readiness for {stack}.{service}: {err}"))?;
    Ok(())
}

fn validate_service_init_tasks(
    stack: &str,
    service: &str,
    svc: &ServiceConfig,
    tasks: Option<&UniqueMap<String, TaskConfig>>,
) -> Result<()> {
    let Some(init) = &svc.init else {
        return Ok(());
    };
    let Some(tasks) = tasks else {
        return Err(anyhow!(
            "service {stack}.{service} references init tasks but no [tasks] are defined"
        ));
    };
    for task_name in init {
        if !tasks.as_map().contains_key(task_name) {
            return Err(anyhow!(
                "service {stack}.{service} references unknown init task '{task_name}'"
            ));
        }
    }
    Ok(())
}

fn validate_service_post_init_tasks(
    stack: &str,
    service: &str,
    svc: &ServiceConfig,
    tasks: Option<&UniqueMap<String, TaskConfig>>,
) -> Result<()> {
    let Some(post_init) = &svc.post_init else {
        return Ok(());
    };
    let Some(tasks) = tasks else {
        return Err(anyhow!(
            "service {stack}.{service} references post_init tasks but no [tasks] are defined"
        ));
    };
    for task_name in post_init {
        if !tasks.as_map().contains_key(task_name) {
            return Err(anyhow!(
                "service {stack}.{service} references unknown post_init task '{task_name}'"
            ));
        }
    }
    Ok(())
}

fn validate_service_auto_restart(stack: &str, service: &str, svc: &ServiceConfig) -> Result<()> {
    if svc.auto_restart && svc.watch.iter().all(|pattern| pattern.trim().is_empty()) {
        return Err(anyhow!(
            "service {stack}.{service} sets auto_restart=true but has no watch patterns"
        ));
    }
    Ok(())
}

pub fn topo_sort(services: &BTreeMap<String, ServiceConfig>) -> Result<Vec<String>> {
    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut reverse: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for (name, svc) in services {
        let mut dep_set = BTreeSet::new();
        for dep in &svc.deps {
            if !services.contains_key(dep) {
                return Err(anyhow!("service {name} depends on missing {dep}"));
            }
            dep_set.insert(dep.clone());
            reverse.entry(dep.clone()).or_default().insert(name.clone());
        }
        deps.insert(name.clone(), dep_set);
    }

    let mut queue: VecDeque<String> = deps
        .iter()
        .filter(|(_, deps)| deps.is_empty())
        .map(|(name, _)| name.clone())
        .collect();

    let mut order = Vec::new();

    while let Some(node) = queue.pop_front() {
        order.push(node.clone());
        if let Some(children) = reverse.get(&node) {
            for child in children {
                if let Some(entry) = deps.get_mut(child) {
                    entry.remove(&node);
                    if entry.is_empty() {
                        queue.push_back(child.clone());
                    }
                }
            }
        }
    }

    if order.len() != services.len() {
        return Err(anyhow!("dependency cycle detected"));
    }

    Ok(order)
}

impl ServiceConfig {
    pub fn scheme(&self) -> String {
        self.scheme.clone().unwrap_or_else(|| "http".to_string())
    }

    pub fn port_env(&self) -> String {
        self.port_env.clone().unwrap_or_else(|| "PORT".to_string())
    }

    pub fn cwd_or(&self, project_dir: &Path) -> PathBuf {
        self.cwd
            .clone()
            .unwrap_or_else(|| project_dir.to_path_buf())
    }

    pub fn readiness_kind(&self, has_port: bool) -> Result<ReadinessKind> {
        if let Some(readiness) = &self.readiness {
            let mut chosen = Vec::new();
            if readiness.tcp.is_some() {
                chosen.push(ReadinessKind::Tcp);
            }
            if let Some(http) = &readiness.http {
                let (min, max) = normalize_expect_status(http.expect_status.clone())?;
                chosen.push(ReadinessKind::Http {
                    path: http.path.clone(),
                    expect_min: min,
                    expect_max: max,
                });
            }
            if let Some(regex) = &readiness.log_regex {
                chosen.push(ReadinessKind::LogRegex {
                    pattern: regex.clone(),
                });
            }
            if let Some(cmd) = &readiness.cmd {
                chosen.push(ReadinessKind::Cmd {
                    command: cmd.clone(),
                });
            }
            if let Some(delay_ms) = readiness.delay_ms {
                chosen.push(ReadinessKind::Delay {
                    duration: std::time::Duration::from_millis(delay_ms),
                });
            }
            if readiness.exit.is_some() {
                chosen.push(ReadinessKind::Exit);
            }
            if chosen.len() != 1 {
                let mut found = Vec::new();
                if readiness.tcp.is_some() {
                    found.push("tcp");
                }
                if readiness.http.is_some() {
                    found.push("http");
                }
                if readiness.log_regex.is_some() {
                    found.push("log_regex");
                }
                if readiness.cmd.is_some() {
                    found.push("cmd");
                }
                if readiness.delay_ms.is_some() {
                    found.push("delay_ms");
                }
                if readiness.exit.is_some() {
                    found.push("exit");
                }
                if found.is_empty() {
                    return Err(anyhow!(
                        "readiness must specify exactly one check (found none)"
                    ));
                }
                return Err(anyhow!(
                    "readiness must specify exactly one check (found: {})",
                    found.join(", ")
                ));
            }
            return Ok(chosen.remove(0));
        }

        if has_port {
            Ok(ReadinessKind::Tcp)
        } else {
            Ok(ReadinessKind::None)
        }
    }

    pub fn readiness_spec(&self, has_port: bool) -> Result<crate::readiness::ReadinessSpec> {
        let kind = self.readiness_kind(has_port)?;
        let timeout = self
            .readiness
            .as_ref()
            .and_then(|r| r.timeout_ms)
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| std::time::Duration::from_secs(30));
        Ok(crate::readiness::ReadinessSpec { kind, timeout })
    }
}

fn normalize_expect_status(range: Option<Vec<u16>>) -> Result<(u16, u16)> {
    if let Some(values) = range {
        if values.len() != 2 {
            return Err(anyhow!("expect_status must have two values"));
        }
        let min = values[0];
        let max = values[1];
        if min > max {
            return Err(anyhow!("expect_status min > max"));
        }
        return Ok((min, max));
    }
    Ok((200, 399))
}

/// Resolve `$VAR` and `${VAR}` environment variable references in a string.
/// Looks up variables from the current process environment (`std::env::vars()`).
pub fn resolve_env_vars(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' {
            // Check for ${VAR} syntax
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut var_name = String::new();
                let mut closed = false;
                // Collect until closing brace
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    var_name.push(c);
                }
                if let Ok(env_val) = std::env::var(&var_name) {
                    result.push_str(&env_val);
                } else {
                    // Variable not found, keep the original ${VAR} token.
                    result.push_str("${");
                    result.push_str(&var_name);
                    if closed {
                        result.push('}');
                    }
                }
            } else {
                // $VAR syntax - consume alphanumeric and underscore characters
                let mut var_name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        var_name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if !var_name.is_empty() {
                    if let Ok(env_val) = std::env::var(&var_name) {
                        result.push_str(&env_val);
                    } else {
                        // Variable not found, keep the original $VAR
                        result.push('$');
                        result.push_str(&var_name);
                    }
                } else {
                    // Lone $ at end or followed by non-identifier char
                    result.push('$');
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Resolve environment variables in all values of an env map.
pub fn resolve_env_map(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .map(|(k, v)| (k.clone(), resolve_env_vars(v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn topo_sort_orders_deps() {
        let mut services = BTreeMap::new();
        services.insert(
            "a".to_string(),
            ServiceConfig {
                cmd: "echo a".to_string(),
                deps: vec![],
                scheme: None,
                port_env: None,
                port: None,
                readiness: None,
                env_file: None,
                env: BTreeMap::new(),
                cwd: None,
                watch: Vec::new(),
                ignore: Vec::new(),
                auto_restart: false,
                init: None,
                post_init: None,
            },
        );
        services.insert(
            "b".to_string(),
            ServiceConfig {
                cmd: "echo b".to_string(),
                deps: vec!["a".to_string()],
                scheme: None,
                port_env: None,
                port: None,
                readiness: None,
                env_file: None,
                env: BTreeMap::new(),
                cwd: None,
                watch: Vec::new(),
                ignore: Vec::new(),
                auto_restart: false,
                init: None,
                post_init: None,
            },
        );
        let order = topo_sort(&services).unwrap();
        assert_eq!(order, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn readiness_defaults_tcp_when_ported() {
        let svc = ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: None,
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        };
        let kind = svc.readiness_kind(true).unwrap();
        assert!(matches!(kind, ReadinessKind::Tcp));
    }

    #[test]
    fn readiness_none_when_no_port() {
        let svc = ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: None,
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        };
        let kind = svc.readiness_kind(false).unwrap();
        assert!(matches!(kind, ReadinessKind::None));
    }

    #[test]
    fn readiness_http_range_parsed() {
        let svc = ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: Some(ReadinessConfig {
                tcp: None,
                http: Some(ReadinessHttp {
                    path: "/health".to_string(),
                    expect_status: Some(vec![200, 204]),
                }),
                log_regex: None,
                cmd: None,
                delay_ms: None,
                exit: None,
                timeout_ms: None,
            }),
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        };
        let kind = svc.readiness_kind(true).unwrap();
        match kind {
            ReadinessKind::Http {
                path,
                expect_min,
                expect_max,
            } => {
                assert_eq!(path, "/health");
                assert_eq!(expect_min, 200);
                assert_eq!(expect_max, 204);
            }
            _ => panic!("expected http readiness"),
        }
    }

    #[test]
    fn readiness_delay_ms_selected() {
        let svc = ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: Some(ReadinessConfig {
                tcp: None,
                http: None,
                log_regex: None,
                cmd: None,
                delay_ms: Some(1500),
                exit: None,
                timeout_ms: None,
            }),
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        };
        let kind = svc.readiness_kind(false).unwrap();
        match kind {
            ReadinessKind::Delay { duration } => {
                assert_eq!(duration, std::time::Duration::from_millis(1500));
            }
            _ => panic!("expected delay readiness"),
        }
    }

    #[test]
    fn readiness_exit_selected() {
        let svc = ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: Some(ReadinessConfig {
                tcp: None,
                http: None,
                log_regex: None,
                cmd: None,
                delay_ms: None,
                exit: Some(ReadinessExit {}),
                timeout_ms: None,
            }),
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        };
        let kind = svc.readiness_kind(false).unwrap();
        assert!(matches!(kind, ReadinessKind::Exit));
    }

    #[test]
    fn duplicate_service_keys_error() {
        let yaml = r#"
version: 1
stacks:
  test:
    services:
      api:
        cmd: "echo api"
      api:
        cmd: "echo api2"
"#;
        let result: Result<ConfigFile, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn port_string_must_be_none() {
        let yaml = r#"
version: 1
stacks:
  test:
    services:
      api:
        cmd: "echo api"
        port: "off"
"#;
        let config: ConfigFile = serde_yaml::from_str(yaml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid port value"));
    }

    #[test]
    fn parses_toml() {
        let toml_str = r#"
version = 1

[stacks.app.services.api]
cmd = "echo api"
readiness = { tcp = {} }
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.version, 1);
        let plan = config.stack_plan("app").unwrap();
        assert!(plan.services.contains_key("api"));
    }

    #[test]
    fn parses_toml_delay_ms_readiness() {
        let toml_str = r#"
version = 1

[stacks.app.services.worker]
cmd = "echo worker"
port = "none"
readiness = { delay_ms = 5000 }
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let plan = config.stack_plan("app").unwrap();
        let svc = plan.services.get("worker").unwrap();
        let kind = svc.readiness_kind(false).unwrap();
        assert!(matches!(kind, ReadinessKind::Delay { .. }));
    }

    #[test]
    fn parses_auto_restart_field() {
        let toml_str = r#"
version = 1

[stacks.app.services.worker]
cmd = "echo worker"
watch = ["src/**"]
auto_restart = true

[stacks.app.services.web]
cmd = "echo web"
watch = ["src/**"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let plan = config.stack_plan("app").unwrap();
        assert!(plan.services.get("worker").unwrap().auto_restart);
        assert!(!plan.services.get("web").unwrap().auto_restart);
    }

    #[test]
    fn auto_restart_requires_watch_patterns() {
        let toml_str = r#"
version = 1

[stacks.app.services.worker]
cmd = "echo worker"
auto_restart = true
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("auto_restart=true"));
    }

    #[test]
    fn default_stack_must_exist() {
        let toml_str = r#"
version = 1
default_stack = "missing"

[stacks.app.services.api]
cmd = "echo api"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("default_stack"));
    }

    #[test]
    fn invalid_service_name_is_rejected() {
        let toml_str = r#"
version = 1

[stacks.app.services."api/../../escape"]
cmd = "echo api"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid service name"));
    }

    #[test]
    fn invalid_global_service_name_is_rejected() {
        let toml_str = r#"
version = 1

[stacks.app.services.api]
cmd = "echo api"

[globals."bad/../../global"]
cmd = "echo global"
readiness = { delay_ms = 1 }
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid global service name"));
    }

    #[test]
    fn invalid_task_name_is_rejected() {
        let toml_str = r#"
version = 1

[stacks.app.services.api]
cmd = "echo api"

[tasks."task/../../escape"]
cmd = "echo task"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid task name"));
    }

    #[test]
    fn find_nearest_config_walks_upwards() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let nested = root.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        let config = root.join("devstack.toml");
        fs::write(
            &config,
            "version = 1\n[stacks.app.services.api]\ncmd = \"echo\"",
        )
        .unwrap();
        let found = ConfigFile::find_nearest_path(&nested).unwrap();
        assert_eq!(found, config);
    }

    #[test]
    fn find_nearest_config_prefers_toml_over_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let yaml = root.join("devstack.yml");
        let toml = root.join("devstack.toml");
        fs::write(&yaml, "version: 1\nstacks: {}").unwrap();
        fs::write(
            &toml,
            "version = 1\n[stacks.app.services.api]\ncmd = \"echo\"",
        )
        .unwrap();
        let found = ConfigFile::find_nearest_path(root).unwrap();
        assert_eq!(found, toml);
    }

    #[test]
    fn resolve_env_vars_dollar_brace_syntax() {
        // Set a known env var for testing
        unsafe { std::env::set_var("DEVSTACK_TEST_VAR", "test_value") };
        let result = resolve_env_vars("value is ${DEVSTACK_TEST_VAR}");
        assert_eq!(result, "value is test_value");
    }

    #[test]
    fn resolve_env_vars_dollar_syntax() {
        unsafe { std::env::set_var("DEVSTACK_TEST_VAR2", "hello") };
        let result = resolve_env_vars("$DEVSTACK_TEST_VAR2 world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn resolve_env_vars_missing_var_keeps_placeholder() {
        let result = resolve_env_vars("$NONEXISTENT_VAR");
        assert_eq!(result, "$NONEXISTENT_VAR");
    }

    #[test]
    fn resolve_env_vars_missing_braced_var_keeps_placeholder() {
        let result = resolve_env_vars("${NONEXISTENT_BRACED}");
        assert_eq!(result, "${NONEXISTENT_BRACED}");
    }

    #[test]
    fn resolve_env_vars_no_interpolation() {
        let result = resolve_env_vars("plain value");
        assert_eq!(result, "plain value");
    }

    #[test]
    fn resolve_env_vars_mixed_content() {
        unsafe { std::env::set_var("DEVSTACK_MIXED", "mixed") };
        let result = resolve_env_vars("before ${DEVSTACK_MIXED} after");
        assert_eq!(result, "before mixed after");
    }

    #[test]
    fn resolve_env_vars_multiple_vars() {
        unsafe {
            std::env::set_var("VAR_A", "alpha");
            std::env::set_var("VAR_B", "beta");
        }
        let result = resolve_env_vars("$VAR_A and $VAR_B");
        assert_eq!(result, "alpha and beta");
    }

    #[test]
    fn resolve_env_map_resolves_all_values() {
        unsafe {
            std::env::set_var("DB_HOST", "localhost");
            std::env::set_var("DB_PORT", "5432");
        }
        let mut env = BTreeMap::new();
        env.insert("HOST".to_string(), "$DB_HOST".to_string());
        env.insert("PORT".to_string(), "${DB_PORT}".to_string());
        let resolved = resolve_env_map(&env);
        assert_eq!(resolved.get("HOST"), Some(&"localhost".to_string()));
        assert_eq!(resolved.get("PORT"), Some(&"5432".to_string()));
    }

    #[test]
    fn post_init_references_unknown_task() {
        let toml_str = r#"
version = 1

[tasks.setup]
cmd = "echo setup"

[stacks.app.services.api]
cmd = "echo api"
post_init = ["missing-task"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("unknown post_init task"));
    }

    #[test]
    fn post_init_without_tasks_section() {
        let toml_str = r#"
version = 1

[stacks.app.services.api]
cmd = "echo api"
post_init = ["setup"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("post_init tasks but no [tasks]"));
    }

    #[test]
    fn post_init_with_valid_task() {
        let toml_str = r#"
version = 1

[tasks.create-resources]
cmd = "python scripts/init.py"
watch = ["scripts/init.py"]

[stacks.app.services.api]
cmd = "echo api"
readiness = { http = { path = "/health" } }
post_init = ["create-resources"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        config.validate().unwrap();
        let plan = config.stack_plan("app").unwrap();
        let svc = plan.services.get("api").unwrap();
        assert_eq!(
            svc.post_init.as_deref(),
            Some(vec!["create-resources".to_string()].as_slice())
        );
    }

    #[test]
    fn global_post_init_references_known_task() {
        let toml_str = r#"
version = 1

[tasks.seed]
cmd = "echo seed"

[stacks.app.services.api]
cmd = "echo api"

[globals.moto]
cmd = "echo moto"
port = "none"
readiness = { delay_ms = 1 }
post_init = ["seed"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        config.validate().unwrap();
    }
}
