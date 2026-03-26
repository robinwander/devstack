use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use tokio::sync::Mutex;

use crate::model::GlobalRecord;

pub struct GlobalStore {
    inner: Mutex<BTreeMap<String, GlobalRecord>>,
}

impl GlobalStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn from_globals(globals: BTreeMap<String, GlobalRecord>) -> Self {
        Self {
            inner: Mutex::new(globals),
        }
    }

    pub async fn upsert_global(&self, record: GlobalRecord) {
        let mut guard = self.inner.lock().await;
        guard.insert(record.key.clone(), record);
    }

    pub async fn get_global(&self, key: &str) -> Option<GlobalRecord> {
        let guard = self.inner.lock().await;
        guard.get(key).cloned()
    }

    pub async fn list_globals(&self) -> Vec<GlobalRecord> {
        let guard = self.inner.lock().await;
        guard.values().cloned().collect()
    }

    pub async fn with_global_mut<F, R>(&self, key: &str, f: F) -> Result<R>
    where
        F: FnOnce(&mut GlobalRecord) -> R,
    {
        let mut guard = self.inner.lock().await;
        let global = guard
            .get_mut(key)
            .ok_or_else(|| anyhow!("global {key} not found"))?;
        Ok(f(global))
    }
}

impl Default for GlobalStore {
    fn default() -> Self {
        Self::new()
    }
}
