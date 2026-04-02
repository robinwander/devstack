use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::model::ReadinessKind;

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
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub env_file: Option<PathBuf>,
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
    #[serde(default)]
    pub tasks: Option<UniqueMap<String, TaskConfig>>,
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

    pub fn readiness_spec(&self, has_port: bool) -> Result<crate::model::ReadinessSpec> {
        let kind = self.readiness_kind(has_port)?;
        let timeout = self
            .readiness
            .as_ref()
            .and_then(|r| r.timeout_ms)
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| std::time::Duration::from_secs(30));
        Ok(crate::model::ReadinessSpec { kind, timeout })
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
