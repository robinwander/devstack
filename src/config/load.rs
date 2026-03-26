use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use super::model::ConfigFile;

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
}