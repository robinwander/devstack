use std::collections::{BTreeSet, HashMap};

use crate::api::FacetValueCount;
use tantivy::collector::{Collector, SegmentCollector};
use tantivy::schema::{Field, FieldType, Value};
use tantivy::store::StoreReader;
use tantivy::{DocId, Score, SegmentReader};

use super::{FACET_STORE_CACHE_BLOCKS, FACET_TERMS_LIMIT, LogIndex};

pub(super) type FacetTermCounts = HashMap<String, HashMap<String, usize>>;

#[derive(Default)]
pub(super) struct ServiceScopeStats {
    pub(super) total: usize,
    pub(super) error_count: usize,
    pub(super) warn_count: usize,
}

pub(super) struct FacetCountCollector {
    field_names: Vec<String>,
}

pub(super) struct ScopeStatsCollector {
    level_field: Field,
}

struct SegmentFacetFieldCounter {
    name: String,
    field: Field,
    counts: HashMap<String, usize>,
}

pub(super) struct FacetCountSegmentCollector {
    store_reader: StoreReader,
    fields: Vec<SegmentFacetFieldCounter>,
    error: Option<tantivy::TantivyError>,
}

pub(super) struct ScopeStatsSegmentCollector {
    store_reader: StoreReader,
    level_field: Field,
    stats: ServiceScopeStats,
    error: Option<tantivy::TantivyError>,
}

impl FacetCountCollector {
    pub(super) fn new(field_names: &[String]) -> Self {
        Self {
            field_names: field_names.to_vec(),
        }
    }
}

impl ScopeStatsCollector {
    pub(super) fn new(level_field: Field) -> Self {
        Self { level_field }
    }
}

impl Collector for FacetCountCollector {
    type Fruit = FacetTermCounts;
    type Child = FacetCountSegmentCollector;

    fn for_segment(
        &self,
        _segment_local_id: u32,
        segment: &SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        let store_reader = segment
            .get_store_reader(FACET_STORE_CACHE_BLOCKS)
            .map_err(tantivy::TantivyError::from)?;
        let fields =
            self.field_names
                .iter()
                .filter_map(|field_name| {
                    segment.schema().get_field(field_name).ok().map(|field| {
                        SegmentFacetFieldCounter {
                            name: field_name.clone(),
                            field,
                            counts: HashMap::new(),
                        }
                    })
                })
                .collect();
        Ok(FacetCountSegmentCollector {
            store_reader,
            fields,
            error: None,
        })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(
        &self,
        segment_fruits: Vec<tantivy::Result<FacetTermCounts>>,
    ) -> tantivy::Result<Self::Fruit> {
        let mut merged = HashMap::new();
        for segment_counts in segment_fruits {
            for (field, values) in segment_counts? {
                let merged_values = merged.entry(field).or_insert_with(HashMap::new);
                for (value, count) in values {
                    *merged_values.entry(value).or_insert(0) += count;
                }
            }
        }
        Ok(merged)
    }
}

impl SegmentCollector for FacetCountSegmentCollector {
    type Fruit = tantivy::Result<FacetTermCounts>;

    fn collect(&mut self, doc: DocId, _score: Score) {
        if self.error.is_some() {
            return;
        }

        let stored_doc: tantivy::TantivyDocument = match self.store_reader.get(doc) {
            Ok(doc) => doc,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };

        for field in &mut self.fields {
            if let Some(value) = stored_doc
                .get_first(field.field)
                .and_then(|value| value.as_str())
            {
                if value.is_empty() {
                    continue;
                }
                if let Some(count) = field.counts.get_mut(value) {
                    *count += 1;
                } else {
                    field.counts.insert(value.to_string(), 1);
                }
            }
        }
    }

    fn harvest(self) -> Self::Fruit {
        if let Some(err) = self.error {
            return Err(err);
        }

        let mut counts = HashMap::new();
        for field in self.fields {
            if !field.counts.is_empty() {
                counts.insert(field.name, field.counts);
            }
        }
        Ok(counts)
    }
}

impl Collector for ScopeStatsCollector {
    type Fruit = ServiceScopeStats;
    type Child = ScopeStatsSegmentCollector;

    fn for_segment(
        &self,
        _segment_local_id: u32,
        segment: &SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        let store_reader = segment
            .get_store_reader(FACET_STORE_CACHE_BLOCKS)
            .map_err(tantivy::TantivyError::from)?;
        Ok(ScopeStatsSegmentCollector {
            store_reader,
            level_field: self.level_field,
            stats: ServiceScopeStats::default(),
            error: None,
        })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(
        &self,
        segment_fruits: Vec<tantivy::Result<ServiceScopeStats>>,
    ) -> tantivy::Result<Self::Fruit> {
        let mut merged = ServiceScopeStats::default();
        for stats in segment_fruits {
            let stats = stats?;
            merged.total += stats.total;
            merged.error_count += stats.error_count;
            merged.warn_count += stats.warn_count;
        }
        Ok(merged)
    }
}

impl SegmentCollector for ScopeStatsSegmentCollector {
    type Fruit = tantivy::Result<ServiceScopeStats>;

    fn collect(&mut self, doc: DocId, _score: Score) {
        if self.error.is_some() {
            return;
        }

        self.stats.total += 1;

        let stored_doc: tantivy::TantivyDocument = match self.store_reader.get(doc) {
            Ok(doc) => doc,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };

        match stored_doc
            .get_first(self.level_field)
            .and_then(|value| value.as_str())
        {
            Some("error") => self.stats.error_count += 1,
            Some("warn") => self.stats.warn_count += 1,
            _ => {}
        }
    }

    fn harvest(self) -> Self::Fruit {
        if let Some(err) = self.error {
            return Err(err);
        }
        Ok(self.stats)
    }
}

impl LogIndex {
    pub(super) fn facet_fields_for_scope(
        &self,
        run_id: &str,
        service: Option<&str>,
    ) -> Vec<String> {
        let mut fields = vec![
            "service".to_string(),
            "level".to_string(),
            "stream".to_string(),
        ];
        let prefix = format!("{run_id}/");
        let ingest = self.ingest.lock().unwrap();
        let mut dynamic_fields = BTreeSet::new();
        match service {
            Some(service) => {
                let key = Self::source_key(run_id, service);
                if let Some(field_names) = ingest.facet_fields.get(&key) {
                    dynamic_fields.extend(field_names.iter().cloned());
                }
            }
            None => {
                for (key, field_names) in &ingest.facet_fields {
                    if key.starts_with(&prefix) {
                        dynamic_fields.extend(field_names.iter().cloned());
                    }
                }
            }
        }
        fields.extend(dynamic_fields);
        fields
    }

    pub(super) fn dynamic_attribute_fields(
        schema: &tantivy::schema::Schema,
    ) -> Vec<(String, Field)> {
        schema
            .fields()
            .filter_map(|(field, entry)| {
                if !entry.is_stored() {
                    return None;
                }
                if !matches!(entry.field_type(), FieldType::Str(_)) {
                    return None;
                }
                let name = entry.name();
                if matches!(
                    name,
                    "run_id" | "service" | "stream" | "level" | "ts" | "message" | "raw"
                ) {
                    return None;
                }
                Some((name.to_string(), field))
            })
            .collect()
    }

    pub(super) fn facet_values_from_counts(
        field_counts: Option<&HashMap<String, usize>>,
    ) -> Vec<FacetValueCount> {
        let Some(field_counts) = field_counts else {
            return Vec::new();
        };

        let mut values: Vec<FacetValueCount> = field_counts
            .iter()
            .filter_map(|(value, count)| {
                if value.is_empty() {
                    return None;
                }
                Some(FacetValueCount {
                    value: value.clone(),
                    count: *count,
                })
            })
            .collect();
        values.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then(left.value.cmp(&right.value))
        });
        values.truncate(FACET_TERMS_LIMIT as usize);
        values
    }

    pub(super) fn facet_kind_for(field: &str) -> &'static str {
        if matches!(field, "level" | "stream") {
            "toggle"
        } else {
            "select"
        }
    }

    pub(super) fn facet_sort_rank(field: &str) -> usize {
        match field {
            "service" => 0,
            "level" => 1,
            "stream" => 2,
            _ => 3,
        }
    }
}
