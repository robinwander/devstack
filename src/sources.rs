use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::paths;
use crate::util::{atomic_write, expand_home, now_rfc3339};

const RESOLVED_SOURCE_CACHE_TTL: Duration = Duration::from_secs(2);
const SOURCE_INDEX_STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct SourceEntry {
    pub name: String,
    pub paths: Vec<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_seconds: Option<u64>,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceIndexStatus {
    Queued,
    Indexing,
    Current,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct SourceFileIndexState {
    pub service: String,
    pub path: String,
    pub len: u64,
    pub modified_nanos: Option<i64>,
    pub indexed_offset: u64,
    pub next_seq: u64,
    pub first_ts_nanos: Option<i64>,
    pub last_ts_nanos: Option<i64>,
    pub skipped_by_retention: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct SourceIndexState {
    pub name: String,
    pub run_id: String,
    pub status: SourceIndexStatus,
    pub retention_seconds: u64,
    pub retention_cutoff_nanos: Option<i64>,
    pub retained_docs: usize,
    pub retained_through_nanos: Option<i64>,
    pub files: Vec<SourceFileIndexState>,
    pub last_indexed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct SourceIndexStateFile {
    version: u32,
    sources: BTreeMap<String, SourceIndexState>,
}

#[derive(Clone)]
pub struct SourceRegistry {
    ledger_path: PathBuf,
    state_path: PathBuf,
    cache_ttl: Duration,
    inner: std::sync::Arc<RwLock<SourceRegistryInner>>,
}

struct SourceRegistryInner {
    ledger: SourcesLedger,
    loaded_modified: Option<SystemTime>,
    resolved: HashMap<String, ResolvedSourceCacheEntry>,
}

struct ResolvedSourceCacheEntry {
    paths: Vec<ResolvedSourcePath>,
    expires_at: Instant,
}

pub fn source_run_id(name: &str) -> String {
    format!("source:{name}")
}

pub fn source_retention_duration(entry: &SourceEntry, default_retention: Duration) -> Duration {
    entry
        .retention_seconds
        .map(Duration::from_secs)
        .unwrap_or(default_retention)
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
        self.add_with_retention(name, paths, None)
    }

    pub fn add_with_retention(
        &mut self,
        name: &str,
        paths: Vec<String>,
        retention_seconds: Option<u64>,
    ) -> Result<()> {
        let path = paths::sources_path()?;
        self.add_at(name, paths, retention_seconds, &path)
    }

    fn add_at(
        &mut self,
        name: &str,
        paths: Vec<String>,
        retention_seconds: Option<u64>,
        ledger_path: &Path,
    ) -> Result<()> {
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
                retention_seconds,
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
        resolved_paths_to_log_sources(name, paths)
    }
}

impl SourceRegistry {
    pub fn load() -> Result<Self> {
        let ledger_path = paths::sources_path()?;
        let state_path = paths::sources_state_path()?;
        let ledger = SourcesLedger::load_from_path(&ledger_path)?;
        let loaded_modified = modified_time(&ledger_path);
        Ok(Self {
            ledger_path,
            state_path,
            cache_ttl: RESOLVED_SOURCE_CACHE_TTL,
            inner: std::sync::Arc::new(RwLock::new(SourceRegistryInner {
                ledger,
                loaded_modified,
                resolved: HashMap::new(),
            })),
        })
    }

    pub fn list(&self) -> Result<Vec<SourceEntry>> {
        let mut inner = self.inner.write().unwrap();
        self.refresh_if_changed(&mut inner)?;
        Ok(inner.ledger.list())
    }

    pub fn get(&self, name: &str) -> Result<Option<SourceEntry>> {
        let mut inner = self.inner.write().unwrap();
        self.refresh_if_changed(&mut inner)?;
        Ok(inner.ledger.get(name).cloned())
    }

    pub fn add(
        &self,
        name: &str,
        paths: Vec<String>,
        retention_seconds: Option<u64>,
    ) -> Result<SourceEntry> {
        let mut inner = self.inner.write().unwrap();
        self.refresh_if_changed(&mut inner)?;
        inner
            .ledger
            .add_at(name, paths, retention_seconds, &self.ledger_path)?;
        inner.loaded_modified = modified_time(&self.ledger_path);
        inner.resolved.remove(name);
        inner
            .ledger
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("source {name} was not persisted"))
    }

    pub fn remove(&self, name: &str) -> Result<bool> {
        let mut inner = self.inner.write().unwrap();
        self.refresh_if_changed(&mut inner)?;
        let removed = inner.ledger.remove_at(name, &self.ledger_path)?;
        if removed {
            inner.loaded_modified = modified_time(&self.ledger_path);
            inner.resolved.remove(name);
            self.remove_index_state(name)?;
        }
        Ok(removed)
    }

    pub fn resolve_log_sources(&self, name: &str) -> Result<Vec<ResolvedSourcePath>> {
        let mut inner = self.inner.write().unwrap();
        self.refresh_if_changed(&mut inner)?;
        if inner.ledger.get(name).is_none() {
            return Err(anyhow!("source {name} not found"));
        }

        let now = Instant::now();
        if let Some(cached) = inner.resolved.get(name)
            && cached.expires_at > now
        {
            return Ok(cached.paths.clone());
        }

        let resolved = inner.ledger.resolve_log_sources(name)?;
        inner.resolved.insert(
            name.to_string(),
            ResolvedSourceCacheEntry {
                paths: resolved.clone(),
                expires_at: now + self.cache_ttl,
            },
        );
        Ok(resolved)
    }

    pub fn index_state(&self, name: &str) -> Result<Option<SourceIndexState>> {
        let file = self.load_index_state_file()?;
        Ok(file.sources.get(name).cloned())
    }

    pub fn set_index_state(&self, state: SourceIndexState) -> Result<()> {
        let mut file = self.load_index_state_file()?;
        file.version = SOURCE_INDEX_STATE_VERSION;
        file.sources.insert(state.name.clone(), state);
        self.save_index_state_file(&file)
    }

    pub fn remove_index_state(&self, name: &str) -> Result<()> {
        let mut file = self.load_index_state_file()?;
        if file.sources.remove(name).is_some() {
            self.save_index_state_file(&file)?;
        }
        Ok(())
    }

    fn refresh_if_changed(&self, inner: &mut SourceRegistryInner) -> Result<()> {
        let current_modified = modified_time(&self.ledger_path);
        if current_modified == inner.loaded_modified {
            return Ok(());
        }
        inner.ledger = SourcesLedger::load_from_path(&self.ledger_path)?;
        inner.loaded_modified = current_modified;
        inner.resolved.clear();
        Ok(())
    }

    fn load_index_state_file(&self) -> Result<SourceIndexStateFile> {
        if !self.state_path.exists() {
            return Ok(SourceIndexStateFile::default());
        }
        let data = std::fs::read_to_string(&self.state_path)
            .with_context(|| format!("read source index state at {:?}", self.state_path))?;
        let state = serde_json::from_str(&data).with_context(|| "parse source index state")?;
        Ok(state)
    }

    fn save_index_state_file(&self, state: &SourceIndexStateFile) -> Result<()> {
        let data = serde_json::to_vec_pretty(state)?;
        atomic_write(&self.state_path, &data)?;
        Ok(())
    }
}

fn resolved_paths_to_log_sources(
    source_name: &str,
    paths: Vec<PathBuf>,
) -> Result<Vec<ResolvedSourcePath>> {
    if paths.len() <= 1 {
        return Ok(paths
            .into_iter()
            .map(|path| ResolvedSourcePath {
                service: source_name.to_string(),
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
            .unwrap_or_else(|| source_name.to_string());
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

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
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
            .add_at("app", vec!["/tmp/app.log".to_string()], None, &ledger_path)
            .unwrap();

        let listed = ledger.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "app");
        assert_eq!(listed[0].retention_seconds, None);

        let removed = ledger.remove_at("app", &ledger_path).unwrap();
        assert!(removed);
        assert!(ledger.list().is_empty());
    }

    #[test]
    fn source_retention_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let ledger_path = dir.path().join("sources.json");
        let mut ledger = SourcesLedger::default();
        ledger
            .add_at(
                "app",
                vec!["/tmp/app.log".to_string()],
                Some(3600),
                &ledger_path,
            )
            .unwrap();

        let loaded = SourcesLedger::load_from_path(&ledger_path).unwrap();
        assert_eq!(loaded.sources["app"].retention_seconds, Some(3600));
        assert_eq!(
            source_retention_duration(&loaded.sources["app"], Duration::from_secs(60)),
            Duration::from_secs(3600),
        );
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
        ledger
            .add_at("logs", vec![pattern], None, &ledger_path)
            .unwrap();

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
                retention_seconds: None,
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
                None,
                &ledger_path,
            )
            .unwrap();

        let resolved = ledger.resolve_log_sources("multi").unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|r| r.service == "api"));
        assert!(resolved.iter().any(|r| r.service == "worker"));
    }
}
