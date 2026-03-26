use anyhow::{Context, Result};
use tantivy::index::SegmentId;
use tantivy::merge_policy::{LogMergePolicy, MergePolicy};
use tantivy::{Index, IndexWriter};

use super::{COMPACTION_MAX_BATCHES_PER_PASS, COMPACTION_SEGMENT_BATCH_SIZE, LogIndex};

impl LogIndex {
    pub(super) fn merge_policy() -> LogMergePolicy {
        let mut policy = LogMergePolicy::default();
        policy.set_min_num_segments(2);
        policy.set_min_layer_size(100);
        policy.set_del_docs_ratio_before_merge(0.5);
        policy
    }

    pub(crate) fn delete_run(&self, run_id: &str) -> Result<()> {
        let _gate = self.ingest_gate.lock().unwrap();
        {
            let mut writer_state = self.writer_state.lock().unwrap();
            let term = tantivy::Term::from_field_text(self.fields.run_id, run_id);
            let writer = writer_state
                .writer
                .as_mut()
                .context("tantivy writer missing")?;
            writer.delete_term(term);
            writer.commit()?;
            std::mem::drop(writer.garbage_collect_files());
            Self::schedule_compaction(&self.index, writer);
        }
        self.reader.read().unwrap().reload().ok();
        {
            let mut ingest = self.ingest.lock().unwrap();
            let prefix = format!("{run_id}/");
            ingest.sources.retain(|key, _| !key.starts_with(&prefix));
            ingest
                .facet_fields
                .retain(|key, _| !key.starts_with(&prefix));
            let bytes = serde_json::to_vec_pretty(&*ingest)?;
            crate::util::atomic_write(&self.ingest_state_path, &bytes)?;
        }
        Ok(())
    }

    pub(crate) fn force_compact(&self) -> Result<()> {
        let _gate = self.ingest_gate.lock().unwrap();
        let mut writer_state = self.writer_state.lock().unwrap();
        let writer = writer_state
            .writer
            .as_mut()
            .context("tantivy writer missing")?;
        std::mem::drop(writer.garbage_collect_files());
        Self::schedule_compaction(&self.index, writer);
        Ok(())
    }

    fn schedule_compaction(index: &std::sync::RwLock<Index>, writer: &mut IndexWriter) {
        for batch in Self::compaction_batches(index) {
            std::mem::drop(writer.merge(&batch));
        }
    }

    fn compaction_batches(index: &std::sync::RwLock<Index>) -> Vec<Vec<SegmentId>> {
        let segments = {
            let index = index.read().unwrap();
            index.searchable_segment_metas().unwrap_or_default()
        };
        if segments.len() <= 1 {
            return Vec::new();
        }

        let merge_policy = Self::merge_policy();
        let mut batches = Vec::new();
        for candidate in merge_policy.compute_merge_candidates(&segments) {
            for chunk in candidate.0.chunks(COMPACTION_SEGMENT_BATCH_SIZE) {
                if chunk.len() < 2 {
                    continue;
                }
                batches.push(chunk.to_vec());
                if batches.len() >= COMPACTION_MAX_BATCHES_PER_PASS {
                    return batches;
                }
            }
        }
        batches
    }
}
