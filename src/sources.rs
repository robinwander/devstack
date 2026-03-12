use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::paths;
use crate::util::{atomic_write, expand_home, now_rfc3339};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceEntry {
    pub name: String,
    pub paths: Vec<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SourcesLedger {
    pub sources: BTreeMap<String, SourceEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSourcePath {
    pub service: String,
    pub path: PathBuf,
}

pub fn source_run_id(name: &str) -> String {
    format!("source:{name}")
}

impl SourcesLedger {
    pub fn load() -> Result<Self> {
        let path = paths::sources_path()?;
        Self::load_from_path(&path)
    }

    pub fn save(&self) -> Result<()> {
        let path = paths::sources_path()?;
        self.save_to_path(&path)
    }

    fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let data = std::fs::read_to_string(path)
            .with_context(|| format!("read sources ledger at {path:?}"))?;
        let ledger: SourcesLedger =
            serde_json::from_str(&data).with_context(|| "parse sources ledger")?;
        Ok(ledger)
    }

    fn save_to_path(&self, path: &Path) -> Result<()> {
        let data = serde_json::to_vec_pretty(self)?;
        atomic_write(path, &data)?;
        Ok(())
    }

    pub fn add(&mut self, name: &str, paths: Vec<String>) -> Result<()> {
        let path = paths::sources_path()?;
        self.add_at(name, paths, &path)
    }

    fn add_at(&mut self, name: &str, paths: Vec<String>, ledger_path: &Path) -> Result<()> {
        if name.trim().is_empty() {
            return Err(anyhow!("source name cannot be empty"));
        }

        let normalized_paths: Vec<String> = paths
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if normalized_paths.is_empty() {
            return Err(anyhow!("source must include at least one path or glob"));
        }

        self.sources.insert(
            name.to_string(),
            SourceEntry {
                name: name.to_string(),
                paths: normalized_paths,
                created_at: now_rfc3339(),
            },
        );
        self.save_to_path(ledger_path)?;
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<bool> {
        let path = paths::sources_path()?;
        self.remove_at(name, &path)
    }

    fn remove_at(&mut self, name: &str, ledger_path: &Path) -> Result<bool> {
        let removed = self.sources.remove(name).is_some();
        if removed {
            self.save_to_path(ledger_path)?;
        }
        Ok(removed)
    }

    pub fn list(&self) -> Vec<SourceEntry> {
        self.sources.values().cloned().collect()
    }

    pub fn get(&self, name: &str) -> Option<&SourceEntry> {
        self.sources.get(name)
    }

    pub fn resolve_paths(&self, name: &str) -> Result<Vec<PathBuf>> {
        let entry = self
            .sources
            .get(name)
            .ok_or_else(|| anyhow!("source {name} not found"))?;

        let mut resolved = BTreeSet::new();
        for pattern in &entry.paths {
            for candidate in expand_pattern(pattern)? {
                if candidate.is_file() {
                    resolved.insert(candidate);
                }
            }
        }
        Ok(resolved.into_iter().collect())
    }

    pub fn resolve_log_sources(&self, name: &str) -> Result<Vec<ResolvedSourcePath>> {
        let paths = self.resolve_paths(name)?;
        if paths.len() <= 1 {
            return Ok(paths
                .into_iter()
                .map(|path| ResolvedSourcePath {
                    service: name.to_string(),
                    path,
                })
                .collect());
        }

        let mut out = Vec::new();
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for path in paths {
            let base = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| name.to_string());
            let slot = seen.entry(base.clone()).or_insert(0);
            *slot += 1;
            let service = if *slot == 1 {
                base
            } else {
                format!("{}-{}", base, slot)
            };
            out.push(ResolvedSourcePath { service, path });
        }
        Ok(out)
    }
}

fn expand_pattern(pattern: &str) -> Result<Vec<PathBuf>> {
    let expanded = expand_home(Path::new(pattern));
    let pattern_text = expanded.to_string_lossy().to_string();

    if !contains_glob(&pattern_text) {
        return Ok(vec![expanded]);
    }

    let mut out = Vec::new();
    for path in glob::glob(&pattern_text)
        .with_context(|| format!("invalid glob pattern: {pattern_text}"))?
        .flatten()
    {
        out.push(path);
    }
    Ok(out)
}

fn contains_glob(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_remove_and_list_sources() {
        let dir = tempfile::tempdir().unwrap();
        let ledger_path = dir.path().join("sources.json");
        let mut ledger = SourcesLedger::default();
        ledger
            .add_at("app", vec!["/tmp/app.log".to_string()], &ledger_path)
            .unwrap();

        let listed = ledger.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "app");

        let removed = ledger.remove_at("app", &ledger_path).unwrap();
        assert!(removed);
        assert!(ledger.list().is_empty());
    }

    #[test]
    fn glob_expansion_resolves_files() {
        let dir = tempfile::tempdir().unwrap();
        let ledger_path = dir.path().join("sources.json");
        let one = dir.path().join("one.log");
        let two = dir.path().join("two.log");
        let _ = std::fs::write(&one, "{}");
        let _ = std::fs::write(&two, "{}");

        let mut ledger = SourcesLedger::default();
        let pattern = format!("{}/*.log", dir.path().display());
        ledger.add_at("logs", vec![pattern], &ledger_path).unwrap();

        let resolved = ledger.resolve_paths("logs").unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains(&one));
        assert!(resolved.contains(&two));
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let ledger_path = dir.path().join("sources.json");

        let mut ledger = SourcesLedger::default();
        ledger.sources.insert(
            "src".to_string(),
            SourceEntry {
                name: "src".to_string(),
                paths: vec!["/tmp/src.log".to_string()],
                created_at: "2025-01-01T00:00:00Z".to_string(),
            },
        );
        ledger.save_to_path(&ledger_path).unwrap();

        let loaded = SourcesLedger::load_from_path(&ledger_path).unwrap();
        assert_eq!(
            loaded.sources.get("src").unwrap().paths,
            vec!["/tmp/src.log"]
        );
    }

    #[test]
    fn multiple_files_use_file_stem_as_service() {
        let dir = tempfile::tempdir().unwrap();
        let ledger_path = dir.path().join("sources.json");
        let one = dir.path().join("api.log");
        let two = dir.path().join("worker.log");
        std::fs::write(&one, "{}").unwrap();
        std::fs::write(&two, "{}").unwrap();

        let mut ledger = SourcesLedger::default();
        ledger
            .add_at(
                "multi",
                vec![
                    one.to_string_lossy().to_string(),
                    two.to_string_lossy().to_string(),
                ],
                &ledger_path,
            )
            .unwrap();

        let resolved = ledger.resolve_log_sources("multi").unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|r| r.service == "api"));
        assert!(resolved.iter().any(|r| r.service == "worker"));
    }
}
