use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use specgraphen_llm::LlmProvider;
use specgraphen_model::SpaceData;
use tokio::sync::Semaphore;

use crate::confidence::{self, DeterministicFacts, LlmAnnotation};

pub struct CorroborationConfig {
    pub high_confidence_threshold: f64,
    pub low_confidence_threshold: f64,
    pub enable_llm_pass: bool,
    pub max_concurrent_llm_calls: usize,
}

impl Default for CorroborationConfig {
    fn default() -> Self {
        Self {
            high_confidence_threshold: 0.8,
            low_confidence_threshold: 0.4,
            enable_llm_pass: true,
            max_concurrent_llm_calls: 5,
        }
    }
}

pub struct CorroborationEngine {
    llm_provider: Option<Arc<dyn LlmProvider>>,
    config: CorroborationConfig,
    source_files: HashMap<String, String>,
}

impl CorroborationEngine {
    pub fn new(llm_provider: Option<Arc<dyn LlmProvider>>, config: CorroborationConfig) -> Self {
        Self {
            llm_provider,
            config,
            source_files: HashMap::new(),
        }
    }

    pub fn load_sources(&mut self, root_path: &std::path::Path) -> Result<()> {
        for path in glob::glob(&root_path.join("**/*.java").to_string_lossy())?.flatten() {
            let rel = path
                .strip_prefix(root_path)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            if let Ok(content) = std::fs::read_to_string(&path) {
                self.source_files.insert(rel, content);
            }
        }
        Ok(())
    }

    pub async fn corroborate(&self, space_data: &mut SpaceData) -> Result<CorroborationStats> {
        let mut stats = CorroborationStats::default();

        if !self.config.enable_llm_pass || self.llm_provider.is_none() {
            // Deterministic-only mode: assign base confidence from structural facts
            for entity in &space_data.entities {
                let annotation = specgraphen_model::SemanticAnnotation::default();
                space_data
                    .annotations
                    .insert(entity.cell_id.clone(), annotation);
                stats.deterministic_only += 1;
            }
            return Ok(stats);
        }

        let provider = self.llm_provider.as_ref().unwrap();
        let semaphore = Arc::new(Semaphore::new(self.config.max_concurrent_llm_calls));

        // Collect method/class entities for LLM analysis
        let entities_to_analyze: Vec<_> = space_data
            .entities
            .iter()
            .filter(|e| {
                matches!(
                    e.entity_type,
                    specgraphen_model::JavaEntityType::Method
                        | specgraphen_model::JavaEntityType::Constructor
                        | specgraphen_model::JavaEntityType::Class
                        | specgraphen_model::JavaEntityType::Interface
                )
            })
            .cloned()
            .collect();

        let mut handles = Vec::new();

        for entity in entities_to_analyze {
            let source_code = self.find_source_for_entity(&entity, space_data);
            if source_code.is_empty() {
                stats.no_source += 1;
                continue;
            }

            let provider = provider.clone();
            let sem = semaphore.clone();
            let entity_label = entity.label.clone();
            let entity_type = format!("{:?}", entity.entity_type);
            let cell_id = entity.cell_id.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();

                let request = specgraphen_llm::prompts::behavior_extraction_prompt(
                    &entity_label,
                    &entity_type,
                    &source_code,
                    &[],
                );

                let result = provider.complete(request).await;
                (cell_id, result)
            });

            handles.push(handle);
        }

        for handle in handles {
            match handle.await {
                Ok((cell_id, Ok(response))) => {
                    if let Some(llm_ann) = LlmAnnotation::from_json(&response.content) {
                        let det_facts = DeterministicFacts {
                            entity_type: "method".to_string(),
                            label: cell_id.clone(),
                            has_provenance: true,
                            caller_count: 0,
                            callee_count: 0,
                        };

                        let outcome =
                            confidence::compute_corroboration(&det_facts, &llm_ann, &cell_id);
                        space_data.annotations.insert(cell_id, outcome.annotation);
                        stats.corroborated += 1;

                        if !outcome.discrepancies.is_empty() {
                            stats.with_discrepancies += 1;
                        }
                    } else {
                        stats.parse_failures += 1;
                        space_data
                            .obstructions
                            .push(format!("LLM response parse failure for {cell_id}"));
                    }
                }
                Ok((cell_id, Err(e))) => {
                    stats.llm_errors += 1;
                    space_data
                        .obstructions
                        .push(format!("LLM error for {cell_id}: {e}"));
                }
                Err(e) => {
                    stats.llm_errors += 1;
                    tracing::warn!("Task join error: {e}");
                }
            }
        }

        Ok(stats)
    }

    fn find_source_for_entity(
        &self,
        entity: &specgraphen_model::space_data::EntityRecord,
        _space_data: &SpaceData,
    ) -> String {
        let file = &entity.witness.file;
        self.source_files.get(file).cloned().unwrap_or_default()
    }
}

#[derive(Debug, Default)]
pub struct CorroborationStats {
    pub deterministic_only: usize,
    pub corroborated: usize,
    pub with_discrepancies: usize,
    pub parse_failures: usize,
    pub llm_errors: usize,
    pub no_source: usize,
}
