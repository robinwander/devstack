use std::collections::{BTreeSet, HashMap};

use crate::api::FacetValueCount;
use columnar::StrColumn;
use tantivy::collector::{Collector, SegmentCollector};
use tantivy::schema::{Field, FieldType};
use tantivy::{DocId, Score, SegmentReader};

use super::{FACET_TERMS_LIMIT, LogIndex};

pub(super) type FacetTermCounts = HashMap<String, HashMap<String, usize>>;

type FacetOrdinalCounts = HashMap<String, (StrColumn, HashMap<u64, usize>)>;

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
    column: StrColumn,
    counts: HashMap<u64, usize>,
}

pub(super) struct FacetCountSegmentCollector {
    fields: Vec<SegmentFacetFieldCounter>,
}

pub(super) struct ScopeStatsSegmentCollector {
    level_column: Option<StrColumn>,
    error_ord: Option<u64>,
    warn_ord: Option<u64>,
    stats: ServiceScopeStats,
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
        let mut fields = Vec::new();
        for field_name in &self.field_names {
            if let Some(column) = segment.fast_fields().str(field_name)? {
                fields.push(SegmentFacetFieldCounter {
                    name: field_name.clone(),
                    column,
                    counts: HashMap::new(),
                });
            }
        }
        Ok(FacetCountSegmentCollector { fields })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(
        &self,
        segment_fruits: Vec<tantivy::Result<FacetOrdinalCounts>>,
    ) -> tantivy::Result<Self::Fruit> {
        let mut merged = HashMap::new();
        for segment_counts in segment_fruits {
            for (field, (column, values)) in segment_counts? {
                let merged_values = merged.entry(field).or_insert_with(HashMap::new);
                let mut value = String::new();
                for (term_ord, count) in values {
                    value.clear();
                    if column
                        .ord_to_str(term_ord, &mut value)
                        .map_err(tantivy::TantivyError::from)?
                        && !value.is_empty()
                    {
                        *merged_values.entry(value.clone()).or_insert(0) += count;
                    }
                }
            }
        }
        Ok(merged)
    }
}

impl SegmentCollector for FacetCountSegmentCollector {
    type Fruit = tantivy::Result<FacetOrdinalCounts>;

    fn collect(&mut self, doc: DocId, _score: Score) {
        for field in &mut self.fields {
            for term_ord in field.column.term_ords(doc) {
                *field.counts.entry(term_ord).or_insert(0) += 1;
            }
        }
    }

    fn harvest(self) -> Self::Fruit {
        let mut counts = HashMap::new();
        for field in self.fields {
            if !field.counts.is_empty() {
                counts.insert(field.name, (field.column, field.counts));
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
        let field_name = segment
            .schema()
            .get_field_entry(self.level_field)
            .name()
            .to_string();
        let level_column = segment.fast_fields().str(&field_name)?;
        let (error_ord, warn_ord) = if let Some(column) = &level_column {
            (
                column
                    .dictionary()
                    .term_ord("error")
                    .map_err(tantivy::TantivyError::from)?,
                column
                    .dictionary()
                    .term_ord("warn")
                    .map_err(tantivy::TantivyError::from)?,
            )
        } else {
            (None, None)
        };
        Ok(ScopeStatsSegmentCollector {
            level_column,
            error_ord,
            warn_ord,
            stats: ServiceScopeStats::default(),
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
        self.stats.total += 1;

        let Some(level_column) = &self.level_column else {
            return;
        };
        for term_ord in level_column.term_ords(doc) {
            if Some(term_ord) == self.error_ord {
                self.stats.error_count += 1;
                return;
            }
            if Some(term_ord) == self.warn_ord {
                self.stats.warn_count += 1;
                return;
            }
        }
    }

    fn harvest(self) -> Self::Fruit {
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
