use std::ops::Bound;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tantivy::Term;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{AllQuery, RangeQuery};

use super::LogIndex;

pub(crate) struct EvictionStats {
    pub(crate) age_deleted: usize,
    pub(crate) size_deleted: usize,
}

impl LogIndex {
    pub(crate) fn evict(&self, max_age: Duration, max_bytes: u64) -> Result<EvictionStats> {
        let age_deleted = self.evict_older_than(max_age)?;
        let size_deleted = self.evict_to_size(max_bytes)?;
        Ok(EvictionStats {
            age_deleted,
            size_deleted,
        })
    }

    fn evict_older_than(&self, max_age: Duration) -> Result<usize> {
        let cutoff_nanos = age_cutoff_nanos(max_age);

        let query = Box::new(RangeQuery::new(
            Bound::Unbounded,
            Bound::Excluded(Term::from_field_i64(self.fields.ts_nanos, cutoff_nanos)),
        ));

        let count = {
            let searcher = self.reader.read().unwrap().searcher();
            searcher.search(query.as_ref(), &Count)?
        };
        if count == 0 {
            return Ok(0);
        }

        {
            let _gate = self.ingest_gate.lock().unwrap();
            let mut writer_state = self.writer_state.lock().unwrap();
            let writer = writer_state
                .writer
                .as_mut()
                .context("tantivy writer missing")?;
            writer.delete_query(query)?;
            writer.commit()?;
            std::mem::drop(writer.garbage_collect_files());
        }
        self.reader.read().unwrap().reload().ok();
        self.clear_facet_cache();

        Ok(count)
    }

    fn evict_to_size(&self, max_bytes: u64) -> Result<usize> {
        let current_size = dir_size_bytes(&self.index_dir);
        if current_size <= max_bytes {
            return Ok(0);
        }

        let total_docs = {
            let searcher = self.reader.read().unwrap().searcher();
            searcher.search(&AllQuery, &Count)?
        };
        if total_docs == 0 {
            return Ok(0);
        }

        let ratio = max_bytes as f64 / current_size as f64;
        let target_docs = ((total_docs as f64) * ratio) as usize;
        let docs_to_remove = total_docs.saturating_sub(target_docs);
        if docs_to_remove == 0 {
            return Ok(0);
        }

        let cutoff_ts = {
            let searcher = self.reader.read().unwrap().searcher();
            let top_docs = searcher.search(
                &AllQuery,
                &TopDocs::with_limit(docs_to_remove)
                    .order_by_fast_field::<i64>("ts_nanos", tantivy::Order::Asc),
            )?;
            match top_docs.iter().map(|(ts, _)| *ts).max() {
                Some(ts) => ts,
                None => return Ok(0),
            }
        };

        let delete_query = Box::new(RangeQuery::new(
            Bound::Unbounded,
            Bound::Included(Term::from_field_i64(self.fields.ts_nanos, cutoff_ts)),
        ));

        {
            let _gate = self.ingest_gate.lock().unwrap();
            let mut writer_state = self.writer_state.lock().unwrap();
            let writer = writer_state
                .writer
                .as_mut()
                .context("tantivy writer missing")?;
            writer.delete_query(delete_query)?;
            writer.commit()?;
            std::mem::drop(writer.garbage_collect_files());
            Self::schedule_compaction(&self.index, writer);
        }
        self.reader.read().unwrap().reload().ok();
        self.clear_facet_cache();

        Ok(docs_to_remove)
    }
}

fn age_cutoff_nanos(max_age: Duration) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let cutoff = now.saturating_sub(max_age);
    cutoff.as_nanos() as i64
}

fn dir_size_bytes(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                total += meta.len();
            }
        }
    }
    total
}
