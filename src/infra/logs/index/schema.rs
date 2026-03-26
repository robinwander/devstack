use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use tantivy::schema::{FAST, INDEXED, STORED, STRING, TEXT};
use tantivy::{Index, ReloadPolicy};

use crate::paths;
use crate::util::atomic_write;

use super::{
    CURRENT_SCHEMA_VERSION, IngestStateFile, LogIndex, LogIndexFields, LogIndexWriterState,
};

impl LogIndex {
    pub(crate) fn open_or_create() -> Result<Self> {
        paths::ensure_base_layout()?;
        let index_dir = paths::logs_index_dir()?;
        let ingest_state_path = paths::logs_ingest_state_path()?;
        Self::open_or_create_at(&index_dir, &ingest_state_path)
    }

    #[cfg(test)]
    pub(crate) fn open_or_create_in(base_dir: &Path) -> Result<Self> {
        let index_dir = base_dir.join("logs_index");
        let ingest_state_path = index_dir.join("ingest_state.json");
        Self::open_or_create_at(&index_dir, &ingest_state_path)
    }

    fn schema_version_path(index_dir: &Path) -> PathBuf {
        index_dir.join("schema_version")
    }

    fn reset_for_schema_version(index_dir: &Path, ingest_state_path: &Path) -> Result<()> {
        let version_path = Self::schema_version_path(index_dir);
        let version_matches = std::fs::read_to_string(&version_path)
            .ok()
            .map(|version| version.trim().to_string())
            .as_deref()
            == Some(CURRENT_SCHEMA_VERSION);

        if version_matches {
            return Ok(());
        }

        if index_dir.exists() {
            std::fs::remove_dir_all(index_dir)?;
        }
        if ingest_state_path.exists() {
            std::fs::remove_file(ingest_state_path)?;
        }

        Ok(())
    }

    fn open_or_create_at(index_dir: &Path, ingest_state_path: &Path) -> Result<Self> {
        Self::reset_for_schema_version(index_dir, ingest_state_path)?;
        std::fs::create_dir_all(index_dir)?;

        let index = match Index::open_in_dir(index_dir) {
            Ok(index) => index,
            Err(_) => {
                if index_dir.join("meta.json").exists() {
                    let backup = index_dir.with_extension(format!("broken.{}", std::process::id()));
                    let _ = std::fs::rename(index_dir, &backup);
                    std::fs::create_dir_all(index_dir)?;
                }
                let schema = Self::build_schema();
                Index::create_in_dir(index_dir, schema)?
            }
        };

        atomic_write(
            &Self::schema_version_path(index_dir),
            CURRENT_SCHEMA_VERSION.as_bytes(),
        )?;

        let schema = index.schema();
        let fields = Self::resolve_fields(&schema)?;

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        let writer = index.writer(32 * 1024 * 1024)?;
        writer.set_merge_policy(Box::new(Self::merge_policy()));

        let ingest = if ingest_state_path.exists() {
            let bytes = std::fs::read(ingest_state_path).unwrap_or_default();
            serde_json::from_slice(&bytes).unwrap_or_default()
        } else {
            IngestStateFile::default()
        };

        Ok(Self {
            index_dir: index_dir.to_path_buf(),
            index: std::sync::RwLock::new(index),
            reader: std::sync::RwLock::new(reader),
            fields,
            writer_state: std::sync::Mutex::new(LogIndexWriterState {
                writer: Some(writer),
                dynamic_fields: HashMap::new(),
            }),
            ingest_state_path: ingest_state_path.to_path_buf(),
            ingest_gate: std::sync::Mutex::new(()),
            ingest: std::sync::Mutex::new(ingest),
        })
    }

    fn build_schema() -> tantivy::schema::Schema {
        let mut schema = tantivy::schema::Schema::builder();
        schema.add_text_field("run_id", STRING | STORED | FAST);
        schema.add_text_field("service", STRING | STORED | FAST);
        schema.add_text_field("stream", STRING | STORED | FAST);
        schema.add_text_field("level", STRING | STORED | FAST);
        schema.add_i64_field("ts_nanos", INDEXED | FAST | STORED);
        schema.add_text_field("ts", STRING | STORED);
        schema.add_u64_field("seq", INDEXED | FAST | STORED);
        schema.add_text_field("message", TEXT | STORED);
        schema.add_text_field("raw", STRING | STORED);
        schema.build()
    }

    fn resolve_fields(schema: &tantivy::schema::Schema) -> Result<LogIndexFields> {
        let run_id = schema
            .get_field("run_id")
            .context("tantivy schema missing field run_id")?;
        let service = schema
            .get_field("service")
            .context("tantivy schema missing field service")?;
        let stream = schema
            .get_field("stream")
            .context("tantivy schema missing field stream")?;
        let level = schema
            .get_field("level")
            .context("tantivy schema missing field level")?;
        let ts_nanos = schema
            .get_field("ts_nanos")
            .context("tantivy schema missing field ts_nanos")?;
        let ts = schema
            .get_field("ts")
            .context("tantivy schema missing field ts")?;
        let seq = schema
            .get_field("seq")
            .context("tantivy schema missing field seq")?;
        let message = schema
            .get_field("message")
            .context("tantivy schema missing field message")?;
        let raw = schema
            .get_field("raw")
            .context("tantivy schema missing field raw")?;
        Ok(LogIndexFields {
            run_id,
            service,
            stream,
            level,
            ts_nanos,
            ts,
            seq,
            message,
            raw,
        })
    }

    pub(super) fn ensure_dynamic_fields(
        &self,
        state: &mut LogIndexWriterState,
        field_names: &BTreeSet<String>,
    ) -> Result<()> {
        let schema = self.index.read().unwrap().schema();
        let mut missing = Vec::new();
        for field_name in field_names {
            if state.dynamic_fields.contains_key(field_name) {
                continue;
            }
            if let Ok(field) = schema.get_field(field_name) {
                state.dynamic_fields.insert(field_name.clone(), field);
                continue;
            }
            missing.push(field_name.clone());
        }

        if missing.is_empty() {
            return Ok(());
        }

        let mut schema_builder = tantivy::schema::Schema::builder();
        for (_, field_entry) in schema.fields() {
            schema_builder.add_field(field_entry.clone());
        }
        for field_name in &missing {
            schema_builder.add_text_field(field_name, STRING | STORED | FAST);
        }

        let mut metas = self.index.read().unwrap().load_metas()?;
        metas.schema = schema_builder.build();
        let bytes = serde_json::to_vec_pretty(&metas)?;
        atomic_write(&self.index_dir.join("meta.json"), &bytes)?;

        drop(state.writer.take());

        let index = Index::open_in_dir(&self.index_dir)?;
        let schema = index.schema();
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        let writer = index.writer(32 * 1024 * 1024)?;
        writer.set_merge_policy(Box::new(Self::merge_policy()));
        Self::resolve_fields(&schema)?;
        let cached_names: Vec<String> = state.dynamic_fields.keys().cloned().collect();

        *self.index.write().unwrap() = index;
        *self.reader.write().unwrap() = reader;
        state.writer = Some(writer);
        state.dynamic_fields.clear();
        for field_name in cached_names.into_iter().chain(missing.into_iter()) {
            if let Ok(field) = schema.get_field(&field_name) {
                state.dynamic_fields.insert(field_name, field);
            }
        }

        Ok(())
    }

    pub(super) fn extract_dynamic_json_fields(line: &str) -> Vec<(String, String)> {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            return Vec::new();
        }

        let Ok(JsonValue::Object(map)) = serde_json::from_str::<JsonValue>(trimmed) else {
            return Vec::new();
        };

        let mut fields = HashMap::new();
        for (field_name, value) in map {
            let Some(field_name) = Self::normalize_dynamic_field_name(&field_name) else {
                continue;
            };
            if Self::is_reserved_dynamic_field(&field_name) {
                continue;
            }
            let Some(value) = Self::dynamic_value_to_string(&value) else {
                continue;
            };
            fields.entry(field_name).or_insert(value);
        }

        fields.into_iter().collect()
    }

    fn normalize_dynamic_field_name(field_name: &str) -> Option<String> {
        let mut normalized = String::with_capacity(field_name.len());
        let mut last_was_underscore = false;

        for ch in field_name.chars() {
            if ch.is_ascii_alphanumeric() {
                normalized.push(ch.to_ascii_lowercase());
                last_was_underscore = false;
            } else if !last_was_underscore {
                normalized.push('_');
                last_was_underscore = true;
            }
        }

        let normalized = normalized.trim_matches('_');
        if normalized.is_empty() {
            None
        } else {
            Some(normalized.to_string())
        }
    }

    fn is_reserved_dynamic_field(field_name: &str) -> bool {
        matches!(
            field_name,
            "time"
                | "ts"
                | "timestamp"
                | "msg"
                | "message"
                | "level"
                | "severity"
                | "stream"
                | "run_id"
                | "service"
                | "ts_nanos"
                | "seq"
                | "raw"
        )
    }

    fn dynamic_value_to_string(value: &JsonValue) -> Option<String> {
        let value = match value {
            JsonValue::String(value) => value.clone(),
            JsonValue::Number(value) => value.to_string(),
            JsonValue::Bool(value) => value.to_string(),
            JsonValue::Array(_) | JsonValue::Object(_) | JsonValue::Null => return None,
        };

        if value.is_empty() || value.chars().count() > 256 {
            None
        } else {
            Some(value)
        }
    }

    pub(super) fn source_key(run_id: &str, service: &str) -> String {
        format!("{run_id}/{service}")
    }
}
