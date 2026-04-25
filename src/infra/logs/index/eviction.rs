use std::ops::Bound;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tantivy::Term;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, RangeQuery, TermQuery};

use super::LogIndex;

pub(crate) struct EvictionStats {
    pub(crate) age_deleted: usize,
    pub(crate) size_deleted: usize,
}

impl LogIndex {
    pub(crate) fn evict(
        &self,
        max_age: Duration,
        max_bytes: u64,
        protected_run_ids: &[String],
    ) -> Result<EvictionStats> {
        let age_deleted = self.evict_older_than(max_age, protected_run_ids)?;
        let size_deleted = self.evict_to_size(max_bytes, protected_run_ids)?;
        Ok(EvictionStats {
            age_deleted,
            size_deleted,
        })
    }

    fn evict_older_than(&self, max_age: Duration, protected_run_ids: &[String]) -> Result<usize> {
        let cutoff_nanos = age_cutoff_nanos(max_age);

        let query = self.protect_run_ids(
            Box::new(RangeQuery::new(
                Bound::Unbounded,
                Bound::Excluded(Term::from_field_i64(self.fields.ts_nanos, cutoff_nanos)),
            )),
            protected_run_ids,
        );

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

    fn evict_to_size(&self, max_bytes: u64, protected_run_ids: &[String]) -> Result<usize> {
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

        let deletable_query = self.protect_run_ids(Box::new(AllQuery), protected_run_ids);
        let deletable_docs = {
            let searcher = self.reader.read().unwrap().searcher();
            searcher.search(deletable_query.as_ref(), &Count)?
        };
        if deletable_docs == 0 {
            return Ok(0);
        }

        let ratio = max_bytes as f64 / current_size as f64;
        let target_docs = ((total_docs as f64) * ratio) as usize;
        let docs_to_remove = total_docs.saturating_sub(target_docs).min(deletable_docs);
        if docs_to_remove == 0 {
            return Ok(0);
        }

        let cutoff_ts = {
            let searcher = self.reader.read().unwrap().searcher();
            let top_docs = searcher.search(
                deletable_query.as_ref(),
                &TopDocs::with_limit(docs_to_remove)
                    .order_by_fast_field::<i64>("ts_nanos", tantivy::Order::Asc),
            )?;
            match top_docs.iter().map(|(ts, _)| *ts).max() {
                Some(ts) => ts,
                None => return Ok(0),
            }
        };

        let delete_query = self.protect_run_ids(
            Box::new(RangeQuery::new(
                Bound::Unbounded,
                Bound::Included(Term::from_field_i64(self.fields.ts_nanos, cutoff_ts)),
            )),
            protected_run_ids,
        );

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

    fn protect_run_ids(
        &self,
        query: Box<dyn Query>,
        protected_run_ids: &[String],
    ) -> Box<dyn Query> {
        if protected_run_ids.is_empty() {
            return query;
        }

        let mut clauses = vec![(Occur::Must, query)];
        for run_id in protected_run_ids {
            clauses.push((
                Occur::MustNot,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.run_id, run_id),
                    tantivy::schema::IndexRecordOption::Basic,
                )) as Box<dyn Query>,
            ));
        }
        Box::new(BooleanQuery::new(clauses))
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
