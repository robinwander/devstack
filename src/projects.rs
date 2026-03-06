use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::api::ProjectSummary;
use crate::config::ConfigFile;
use crate::paths;
use crate::util::{atomic_write, now_rfc3339, project_hash};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectsLedger {
    pub projects: BTreeMap<String, ProjectEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub path: String,
    pub name: String,
    pub last_used: Option<String>,
}

impl ProjectsLedger {
    pub fn load() -> Result<Self> {
        let path = paths::projects_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("read projects ledger at {path:?}"))?;
        let ledger: ProjectsLedger = serde_json::from_str(&data)
            .with_context(|| "parse projects ledger")?;
        Ok(ledger)
    }

    pub fn save(&self) -> Result<()> {
        let path = paths::projects_path()?;
        let data = serde_json::to_vec_pretty(self)?;
        atomic_write(&path, &data)?;
        Ok(())
    }

    pub fn register(&mut self, project_dir: &Path) -> Result<String> {
        let canonical = std::fs::canonicalize(project_dir)
            .unwrap_or_else(|_| project_dir.to_path_buf());
        let id = project_hash(&canonical);
        let name = canonical
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        self.projects.insert(id.clone(), ProjectEntry {
            path: canonical.to_string_lossy().to_string(),
            name,
            last_used: Some(now_rfc3339()),
        });
        
        self.save()?;
        Ok(id)
    }

    pub fn touch(&mut self, project_dir: &Path) -> Result<()> {
        let canonical = std::fs::canonicalize(project_dir)
            .unwrap_or_else(|_| project_dir.to_path_buf());
        let id = project_hash(&canonical);
        
        if let Some(entry) = self.projects.get_mut(&id) {
            entry.last_used = Some(now_rfc3339());
            self.save()?;
        } else {
            self.register(project_dir)?;
        }
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let removed = self.projects.remove(id).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn to_summaries(&self) -> Vec<ProjectSummary> {
        self.projects
            .iter()
            .map(|(id, entry)| {
                let path = PathBuf::from(&entry.path);
                let config_path = ConfigFile::default_path(&path);
                let config_exists = config_path.exists();
                
                let stacks = if config_exists {
                    ConfigFile::load_from_path(&config_path)
                        .map(|c| c.stacks.as_map().keys().cloned().collect())
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                ProjectSummary {
                    id: id.clone(),
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    stacks,
                    last_used: entry.last_used.clone(),
                    config_exists,
                }
            })
            .collect()
    }

    pub fn seed_from_runs(&mut self, runs_dir: &Path) -> Result<usize> {
        if !runs_dir.exists() {
            return Ok(0);
        }
        
        let mut count = 0;
        for entry in std::fs::read_dir(runs_dir)? {
            let entry = entry?;
            let manifest_path = entry.path().join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            
            let data = match std::fs::read_to_string(&manifest_path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            
            #[derive(Deserialize)]
            struct ManifestStub {
                project_dir: String,
            }
            
            let manifest: ManifestStub = match serde_json::from_str(&data) {
                Ok(m) => m,
                Err(_) => continue,
            };
            
            let project_path = PathBuf::from(&manifest.project_dir);
            let canonical = std::fs::canonicalize(&project_path)
                .unwrap_or(project_path.clone());
            let id = project_hash(&canonical);
            
            if let std::collections::btree_map::Entry::Vacant(e) = self.projects.entry(id) {
                let name = canonical
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                
                e.insert(ProjectEntry {
                    path: canonical.to_string_lossy().to_string(),
                    name,
                    last_used: None,
                });
                count += 1;
            }
        }
        
        if count > 0 {
            self.save()?;
        }
        
        Ok(count)
    }
}
