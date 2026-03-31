use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

use crate::model::{RunLifecycle, ServiceState};
use crate::util::atomic_write;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedService {
    pub port: Option<u16>,
    pub url: Option<String>,
    pub state: ServiceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_started_at: Option<String>,
    #[serde(default)]
    pub watch_paused: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PersistedRun {
    pub run_id: String,
    pub project_dir: String,
    pub config_dir: String,
    pub manifest_path: String,
    pub stack: String,
    pub services: BTreeMap<String, PersistedService>,
    pub env: BTreeMap<String, String>,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct PersistedRunWire {
    pub run_id: String,
    pub project_dir: String,
    #[serde(default)]
    pub config_dir: Option<String>,
    pub manifest_path: String,
    pub stack: String,
    pub services: BTreeMap<String, PersistedService>,
    pub env: BTreeMap<String, String>,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

impl<'de> Deserialize<'de> for PersistedRun {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PersistedRunWire::deserialize(deserializer)?;
        let config_dir = wire
            .config_dir
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| wire.project_dir.clone());

        Ok(Self {
            run_id: wire.run_id,
            project_dir: wire.project_dir,
            config_dir,
            manifest_path: wire.manifest_path,
            stack: wire.stack,
            services: wire.services,
            env: wire.env,
            state: wire.state,
            created_at: wire.created_at,
            stopped_at: wire.stopped_at,
        })
    }
}

impl PersistedRun {
    pub fn write_to_path(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).context("serialize run manifest")?;
        atomic_write(path, &json)
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read run manifest {path:?}"))?;
        let manifest = serde_json::from_slice(&data).context("parse run manifest")?;
        Ok(manifest)
    }
}

pub fn run_manifest_is_restorable(manifest: &PersistedRun) -> bool {
    manifest.state != RunLifecycle::Stopped
        && manifest.stopped_at.is_none()
        && Path::new(&manifest.project_dir).exists()
}

#[derive(Clone, Debug, Serialize)]
pub struct PersistedGlobal {
    pub key: String,
    pub name: String,
    pub project_dir: String,
    pub config_path: String,
    pub manifest_path: String,
    pub service: PersistedService,
    pub env: BTreeMap<String, String>,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct PersistedGlobalCurrentWire {
    pub key: String,
    pub name: String,
    pub project_dir: String,
    pub config_path: String,
    pub manifest_path: String,
    pub service: PersistedService,
    pub env: BTreeMap<String, String>,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct PersistedGlobalLegacyWire {
    pub project_dir: String,
    pub manifest_path: String,
    pub services: BTreeMap<String, PersistedService>,
    pub env: BTreeMap<String, String>,
    pub state: RunLifecycle,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum PersistedGlobalWire {
    Current(PersistedGlobalCurrentWire),
    Legacy(PersistedGlobalLegacyWire),
}

impl<'de> Deserialize<'de> for PersistedGlobal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match PersistedGlobalWire::deserialize(deserializer)? {
            PersistedGlobalWire::Current(wire) => Ok(Self {
                key: wire.key,
                name: wire.name,
                project_dir: wire.project_dir,
                config_path: wire.config_path,
                manifest_path: wire.manifest_path,
                service: wire.service,
                env: wire.env,
                state: wire.state,
                created_at: wire.created_at,
                stopped_at: wire.stopped_at,
            }),
            PersistedGlobalWire::Legacy(wire) => {
                let mut services = wire.services.into_iter();
                let (name, service) = services.next().ok_or_else(|| {
                    de::Error::custom("legacy global manifest missing service record")
                })?;
                if services.next().is_some() {
                    return Err(de::Error::custom(
                        "legacy global manifest must contain exactly one service record",
                    ));
                }

                let project_dir = wire.project_dir;
                let manifest_path = wire.manifest_path;
                let key = std::path::Path::new(&manifest_path)
                    .parent()
                    .and_then(|path| path.file_name())
                    .and_then(|value| value.to_str())
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| {
                        crate::paths::global_key(std::path::Path::new(&project_dir), &name)
                            .unwrap_or_else(|_| format!("legacy__{name}"))
                    });
                let config_path = crate::config::ConfigFile::find_nearest_path(
                    std::path::Path::new(&project_dir),
                )
                .unwrap_or_else(|| {
                    crate::config::ConfigFile::default_path(std::path::Path::new(&project_dir))
                });

                Ok(Self {
                    key,
                    name,
                    project_dir,
                    config_path: config_path.to_string_lossy().to_string(),
                    manifest_path,
                    service,
                    env: wire.env,
                    state: wire.state,
                    created_at: wire.created_at,
                    stopped_at: wire.stopped_at,
                })
            }
        }
    }
}

impl PersistedGlobal {
    pub fn write_to_path(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).context("serialize global manifest")?;
        atomic_write(path, &json)
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read global manifest {path:?}"))?;
        let manifest = serde_json::from_slice(&data).context("parse global manifest")?;
        Ok(manifest)
    }
}

pub fn global_manifest_is_restorable(manifest: &PersistedGlobal) -> bool {
    manifest.state != RunLifecycle::Stopped
        && manifest.stopped_at.is_none()
        && Path::new(&manifest.project_dir).exists()
        && Path::new(&manifest.config_path).exists()
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn persisted_run_deserializes_legacy_manifest_without_config_dir() {
        let manifest = serde_json::json!({
            "run_id": "dev-1234",
            "project_dir": "/tmp/project",
            "manifest_path": "/tmp/devstack/runs/dev-1234/manifest.json",
            "stack": "dev",
            "services": {
                "api": {
                    "port": 3000,
                    "url": "http://localhost:3000",
                    "state": "ready"
                }
            },
            "env": {
                "DEV_RUN_ID": "dev-1234"
            },
            "state": "running",
            "created_at": "2026-03-01T00:00:00Z",
            "stopped_at": null
        });

        let parsed: PersistedRun = serde_json::from_value(manifest).expect("legacy manifest");
        assert_eq!(parsed.config_dir, "/tmp/project");
    }

    #[test]
    fn persisted_global_deserializes_legacy_manifest_without_current_fields() {
        let manifest = serde_json::json!({
            "run_id": "global-abc123__cache",
            "project_dir": "/tmp/project",
            "stack": "globals",
            "manifest_path": "/tmp/devstack/globals/abc123__cache/manifest.json",
            "services": {
                "cache": {
                    "port": 6379,
                    "url": "redis://localhost:6379",
                    "state": "ready"
                }
            },
            "env": {
                "DEV_PROJECT_DIR": "/tmp/project"
            },
            "state": "running",
            "created_at": "2026-03-01T00:00:00Z",
            "stopped_at": null
        });

        let parsed: PersistedGlobal = serde_json::from_value(manifest).expect("legacy global");
        assert_eq!(parsed.key, "abc123__cache");
        assert_eq!(parsed.name, "cache");
        assert_eq!(parsed.config_path, "/tmp/project/devstack.toml");
    }

    #[test]
    fn run_manifest_is_restorable_requires_project_dir_to_exist() {
        let temp = tempdir().expect("tempdir");
        let manifest = PersistedRun {
            run_id: "dev-1234".to_string(),
            project_dir: temp.path().join("project").to_string_lossy().to_string(),
            config_dir: temp.path().join("project").to_string_lossy().to_string(),
            manifest_path: temp
                .path()
                .join("manifest.json")
                .to_string_lossy()
                .to_string(),
            stack: "dev".to_string(),
            services: BTreeMap::new(),
            env: BTreeMap::new(),
            state: RunLifecycle::Running,
            created_at: "2026-03-01T00:00:00Z".to_string(),
            stopped_at: None,
        };

        assert!(!run_manifest_is_restorable(&manifest));
        std::fs::create_dir_all(&manifest.project_dir).expect("create project dir");
        assert!(run_manifest_is_restorable(&manifest));
    }

    #[test]
    fn global_manifest_is_restorable_requires_project_dir_and_config_path() {
        let temp = tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        let config_path = project_dir.join("devstack.toml");
        let manifest = PersistedGlobal {
            key: "abc123__cache".to_string(),
            name: "cache".to_string(),
            project_dir: project_dir.to_string_lossy().to_string(),
            config_path: config_path.to_string_lossy().to_string(),
            manifest_path: temp
                .path()
                .join("manifest.json")
                .to_string_lossy()
                .to_string(),
            service: PersistedService {
                port: Some(6379),
                url: Some("redis://localhost:6379".to_string()),
                state: ServiceState::Ready,
                watch_hash: None,
                last_failure: None,
                last_started_at: None,
                watch_paused: false,
            },
            env: BTreeMap::new(),
            state: RunLifecycle::Running,
            created_at: "2026-03-01T00:00:00Z".to_string(),
            stopped_at: None,
        };

        assert!(!global_manifest_is_restorable(&manifest));
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        assert!(!global_manifest_is_restorable(&manifest));
        std::fs::write(&config_path, "version = 1\n").expect("write config");
        assert!(global_manifest_is_restorable(&manifest));
    }
}
