//! Query engine over lifted spaces: explain, callers, callees, and impact analysis.

mod call_graph;
mod column_usage;
mod crud;
mod dead_code;
mod dependencies;
pub mod enrich;
mod explain;
pub mod export;
mod feature;
mod hotspots;
mod impact;
mod java;
mod mybatis;
mod overview;
pub mod projection;
pub mod resolve;
mod search;
mod sql;
pub mod types;
mod unknowns;

use std::collections::HashMap;
use std::sync::RwLock;

use anyhow::Result;
use specgraphen_model::{SemanticAnnotation, SpaceData};

pub use column_usage::ColumnUsageResult;
pub use crud::CrudMatrixResult;
pub use dead_code::DeadCodeResult;
pub use dependencies::DependencyResult;
pub use enrich::{EnrichBatchRequest, EnrichRequest};
pub use feature::FeatureResult;
pub use hotspots::HotspotsResult;
pub use impact::ImpactResult;
pub use overview::OverviewResult;
pub use search::SearchResult;
pub use types::{CallGraphResult, CallRelation, ConfidenceWrapped, ExplainResult, WitnessRef};
pub use unknowns::UnknownsResult;

pub struct QueryEngine {
    space_data: RwLock<SpaceData>,
    source_files: HashMap<String, String>,
}

impl QueryEngine {
    pub fn new(space_data: SpaceData) -> Self {
        Self {
            space_data: RwLock::new(space_data),
            source_files: HashMap::new(),
        }
    }

    pub fn with_sources(mut self, source_files: HashMap<String, String>) -> Self {
        self.source_files = source_files;
        self
    }

    /// Resolve a symbol to `(fqn, witness file, start line)` for tools that
    /// need to re-read the underlying source.
    pub fn witness_of(&self, symbol: &str) -> Option<(String, String, u32)> {
        let data = self.space_data.read().unwrap();
        let (fqn, cell_id) = resolve::resolve_symbol(&data, symbol)?;
        let entity = data.entities.iter().find(|e| e.cell_id == cell_id)?;
        Some((
            fqn.to_string(),
            entity.witness.file.clone(),
            entity.witness.start_line,
        ))
    }

    /// Loaded source content for a witness-relative file path, if available.
    pub fn file_source(&self, file: &str) -> Option<&str> {
        self.source_files.get(file).map(String::as_str)
    }

    pub fn explain(&self, symbol: &str) -> Result<ExplainResult> {
        let data = self.space_data.read().unwrap();
        explain::explain(&data, symbol)
    }

    pub fn callers(&self, symbol: &str) -> Result<CallGraphResult> {
        let data = self.space_data.read().unwrap();
        call_graph::callers(&data, symbol)
    }

    pub fn callees(&self, symbol: &str) -> Result<CallGraphResult> {
        let data = self.space_data.read().unwrap();
        call_graph::callees(&data, symbol)
    }

    pub fn overview(&self) -> Result<OverviewResult> {
        let data = self.space_data.read().unwrap();
        overview::overview(&data)
    }

    pub fn search(
        &self,
        query: &str,
        entity_type: Option<&str>,
        limit: usize,
    ) -> Result<SearchResult> {
        let data = self.space_data.read().unwrap();
        search::search(&data, query, entity_type, limit)
    }

    pub fn package_dependencies(&self) -> Result<DependencyResult> {
        let data = self.space_data.read().unwrap();
        dependencies::package_dependencies(&data)
    }

    pub fn class_dependencies(&self, class_fqn: &str) -> Result<DependencyResult> {
        let data = self.space_data.read().unwrap();
        dependencies::class_dependencies(&data, class_fqn)
    }

    pub fn feature(&self, keyword: &str) -> Result<FeatureResult> {
        let data = self.space_data.read().unwrap();
        feature::analyze_feature(&data, keyword)
    }

    pub fn unknowns(&self, scope: Option<&str>) -> Result<UnknownsResult> {
        let data = self.space_data.read().unwrap();
        unknowns::unknowns(&data, scope)
    }

    pub fn impact(&self, symbol: &str, max_depth: usize) -> Result<ImpactResult> {
        let data = self.space_data.read().unwrap();
        impact::impact(&data, symbol, max_depth)
    }

    pub fn column_usage(&self, table_class: &str) -> Result<ColumnUsageResult> {
        let data = self.space_data.read().unwrap();
        column_usage::column_usage(&data, table_class, &self.source_files)
    }

    pub fn dead_code(&self) -> Result<DeadCodeResult> {
        let data = self.space_data.read().unwrap();
        dead_code::dead_code(&data, &self.source_files)
    }

    pub fn hotspots(&self, limit: usize) -> Result<HotspotsResult> {
        let data = self.space_data.read().unwrap();
        hotspots::hotspots(&data, &self.source_files, limit)
    }

    pub fn crud_matrix(&self) -> Result<CrudMatrixResult> {
        let data = self.space_data.read().unwrap();
        crud::crud_matrix(&data, &self.source_files)
    }

    pub fn spec_markdown(&self) -> Result<String> {
        let data = self.space_data.read().unwrap();
        export::spec_markdown(&data)
    }

    pub fn enrich(&self, symbol: &str) -> Result<EnrichRequest> {
        let data = self.space_data.read().unwrap();
        enrich::enrich(&data, symbol, &self.source_files)
    }

    pub fn enrich_batch(&self, scope: Option<&str>, limit: usize) -> Result<EnrichBatchRequest> {
        let data = self.space_data.read().unwrap();
        enrich::enrich_batch(&data, scope, limit, &self.source_files)
    }

    pub fn annotate(&self, cell_id: &str, annotation: SemanticAnnotation) -> Result<()> {
        let mut data = self.space_data.write().unwrap();
        data.annotations.insert(cell_id.to_string(), annotation);
        Ok(())
    }

    pub fn annotate_by_fqn(&self, fqn: &str, annotation: SemanticAnnotation) -> Result<()> {
        let data = self.space_data.read().unwrap();
        let cell_id = data
            .fqn_to_cell_id
            .get(fqn)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("FQN not found: {fqn}"))?;
        drop(data);
        self.annotate(&cell_id, annotation)
    }

    pub fn space_data(&self) -> std::sync::RwLockReadGuard<'_, SpaceData> {
        self.space_data.read().unwrap()
    }

    pub fn save_snapshot(&self) -> Result<SpaceData> {
        let data = self.space_data.read().unwrap();
        let mut snapshot = SpaceData::new(
            data.space.clone(),
            data.cells.clone(),
            data.incidences.clone(),
            data.entities.clone(),
            data.relations.clone(),
            data.fqn_to_cell_id.clone(),
        );
        snapshot.annotations = data.annotations.clone();
        snapshot.obstructions = data.obstructions.clone();
        Ok(snapshot)
    }
}
