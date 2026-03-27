use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use serde::{Deserialize, Serialize};
use tantivy::schema::Field;
use tantivy::{Index, IndexReader, IndexWriter};

pub(crate) const CURRENT_SCHEMA_VERSION: &str = "4";
pub(crate) const FACET_TERMS_LIMIT: u32 = 50;
pub(crate) const FACET_STORE_CACHE_BLOCKS: usize = 32;
pub(crate) const COMPACTION_SEGMENT_BATCH_SIZE: usize = 32;
pub(crate) const COMPACTION_MAX_BATCHES_PER_PASS: usize = 8;

#[derive(Clone, Debug)]
pub(crate) struct LogSource {
    pub(crate) run_id: String,
    pub(crate) service: String,
    pub(crate) path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct IngestStateFile {
    pub(crate) version: u32,
    pub(crate) sources: HashMap<String, IngestCursor>,
    #[serde(default)]
    pub(crate) facet_fields: HashMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct IngestCursor {
    pub(crate) offset: u64,
    pub(crate) next_seq: u64,
}

#[derive(Clone)]
pub(crate) struct LogIndexFields {
    pub(crate) run_id: Field,
    pub(crate) service: Field,
    pub(crate) stream: Field,
    pub(crate) level: Field,
    pub(crate) ts_nanos: Field,
    pub(crate) ts: Field,
    pub(crate) seq: Field,
    pub(crate) message: Field,
    pub(crate) raw: Field,
}

pub(crate) struct LogIndexWriterState {
    pub(crate) writer: Option<IndexWriter>,
    pub(crate) dynamic_fields: HashMap<String, Field>,
}

pub(crate) struct LogIndex {
    pub(crate) index_dir: PathBuf,
    pub(crate) index: RwLock<Index>,
    pub(crate) reader: RwLock<IndexReader>,
    pub(crate) fields: LogIndexFields,
    pub(crate) writer_state: Mutex<LogIndexWriterState>,
    pub(crate) ingest_state_path: PathBuf,
    pub(crate) ingest_gate: Mutex<()>,
    pub(crate) ingest: Mutex<IngestStateFile>,
}
