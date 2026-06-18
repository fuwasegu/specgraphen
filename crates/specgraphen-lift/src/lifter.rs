use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use higher_graphen_core::Id;
use serde::{Deserialize, Serialize};
use specgraphen_model::{CellFactory, SpaceData};

use crate::entity_extractor::EntityExtractor;
use crate::file_walker::collect_java_files;
use crate::relation_extractor::RelationExtractor;

#[derive(Debug, Clone)]
pub struct LiftConfig {
    pub root_path: PathBuf,
    pub file_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub space_id: String,
    pub space_label: String,
    pub resolved_cache: HashMap<String, String>,
}

impl Default for LiftConfig {
    fn default() -> Self {
        Self {
            root_path: PathBuf::from("."),
            file_patterns: vec!["**/*.java".to_string()],
            exclude_patterns: vec![],
            space_id: "default".to_string(),
            space_label: "Java Project".to_string(),
            resolved_cache: HashMap::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LiftDiagnostic {
    pub file: PathBuf,
    pub message: String,
    pub severity: DiagnosticSeverity,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct UnresolvedInfo {
    pub target_text: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug)]
pub struct LiftResult {
    pub space_data: SpaceData,
    pub diagnostics: Vec<LiftDiagnostic>,
}

pub struct JavaLifter {
    parser: tree_sitter::Parser,
}

impl JavaLifter {
    pub fn new() -> Result<Self> {
        let mut parser = tree_sitter::Parser::new();
        let language = tree_sitter_java::LANGUAGE;
        parser
            .set_language(&language.into())
            .map_err(|e| anyhow::anyhow!("Failed to set Java language: {}", e))?;
        Ok(Self { parser })
    }

    pub fn lift(&mut self, config: &LiftConfig) -> Result<LiftResult> {
        let space_id = Id::new(&config.space_id)?;
        let mut factory = CellFactory::new(space_id);
        let mut space = factory.create_space(&config.space_label);
        let mut diagnostics = Vec::new();

        let files = collect_java_files(
            &config.root_path,
            &config.file_patterns,
            &config.exclude_patterns,
        )?;

        tracing::info!(file_count = files.len(), "Collected Java files");

        // Pass 1: Extract entities from all files
        let mut all_entity_extractors = Vec::new();
        for file_path in &files {
            match self.extract_entities_from_file(file_path, &config.root_path, &mut factory) {
                Ok(extractor) => all_entity_extractors.push(extractor),
                Err(e) => {
                    diagnostics.push(LiftDiagnostic {
                        file: file_path.clone(),
                        message: format!("Parse error: {e}"),
                        severity: DiagnosticSeverity::Error,
                    });
                }
            }
        }

        // Merge all FQN maps
        let mut global_fqn_map: HashMap<String, Id> = HashMap::new();
        let mut all_cells = Vec::new();
        let mut all_entities = Vec::new();

        for extractor in &all_entity_extractors {
            for (fqn, id) in &extractor.fqn_to_cell_id {
                global_fqn_map.insert(fqn.clone(), id.clone());
            }
            for cell in &extractor.cells {
                space.cell_ids.push(cell.id.clone());
                all_cells.push(cell.clone());
            }
        }

        // Build entity records with accurate witness info
        for extractor in &all_entity_extractors {
            for (fqn, cell_id) in &extractor.fqn_to_cell_id {
                let cell = extractor.cells.iter().find(|c| &c.id == cell_id);
                let witness = extractor.fqn_to_witness.get(fqn).cloned().unwrap_or(
                    specgraphen_model::WitnessInfo {
                        file: String::new(),
                        start_line: 0,
                        end_line: 0,
                        start_col: 0,
                        end_col: 0,
                        derivation_source: specgraphen_model::DerivationSource::TreeSitter,
                    },
                );
                if let Some(cell) = cell {
                    all_entities.push(specgraphen_model::space_data::EntityRecord {
                        fqn: fqn.clone(),
                        cell_id: cell.id.as_str().to_string(),
                        entity_type: cell_type_to_entity_type(&cell.cell_type),
                        label: cell.label.clone().unwrap_or_default(),
                        witness,
                    });
                }
            }
        }

        tracing::info!(entity_count = all_cells.len(), "Entities extracted");

        // Pass 2: Extract relations with the global FQN map
        let mut all_incidences = Vec::new();
        let all_relations = Vec::new();
        let mut total_unresolved = 0;
        let mut all_unresolved = Vec::new();

        for file_path in &files {
            let rel_path = file_path
                .strip_prefix(&config.root_path)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();

            match self.extract_relations_from_file(
                file_path,
                &rel_path,
                &global_fqn_map,
                &config.resolved_cache,
                &mut factory,
            ) {
                Ok(extractor) => {
                    total_unresolved += extractor.unresolved.len();
                    for unresolved in &extractor.unresolved {
                        diagnostics.push(LiftDiagnostic {
                            file: file_path.clone(),
                            message: format!(
                                "Unresolved {:?}: {} → {}",
                                unresolved.relation_type,
                                unresolved.from_fqn,
                                unresolved.target_text
                            ),
                            severity: DiagnosticSeverity::Warning,
                        });
                        all_unresolved.push(specgraphen_model::space_data::UnresolvedRecord {
                            from_fqn: unresolved.from_fqn.clone(),
                            target_text: unresolved.target_text.clone(),
                            relation_type: unresolved.relation_type.clone(),
                            file: unresolved.witness.file.clone(),
                            line: unresolved.witness.start_line,
                        });
                    }
                    for inc in &extractor.incidences {
                        space.incidence_ids.push(inc.id.clone());
                        all_incidences.push(inc.clone());
                    }
                }
                Err(e) => {
                    diagnostics.push(LiftDiagnostic {
                        file: file_path.clone(),
                        message: format!("Relation extraction error: {e}"),
                        severity: DiagnosticSeverity::Error,
                    });
                }
            }
        }

        tracing::info!(
            relation_count = all_incidences.len(),
            unresolved = total_unresolved,
            "Relations extracted"
        );

        let fqn_to_cell_id_strings: HashMap<String, String> = global_fqn_map
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().to_string()))
            .collect();

        let mut space_data = SpaceData::new(
            space,
            all_cells,
            all_incidences,
            all_entities,
            all_relations,
            fqn_to_cell_id_strings,
        );

        space_data.unresolved = all_unresolved;
        space_data.rebuild_store();
        tracing::info!("HG InMemorySpaceStore built");

        Ok(LiftResult {
            space_data,
            diagnostics,
        })
    }

    pub fn collect_unresolved(&mut self, config: &LiftConfig) -> Result<Vec<UnresolvedInfo>> {
        let files = collect_java_files(
            &config.root_path,
            &config.file_patterns,
            &config.exclude_patterns,
        )?;

        let space_id = Id::new(&config.space_id)?;
        let mut factory = CellFactory::new(space_id);

        // Pass 1: entities
        let mut all_entity_extractors = Vec::new();
        for file_path in &files {
            if let Ok(extractor) =
                self.extract_entities_from_file(file_path, &config.root_path, &mut factory)
            {
                all_entity_extractors.push(extractor);
            }
        }

        let mut global_fqn_map: HashMap<String, Id> = HashMap::new();
        for extractor in &all_entity_extractors {
            for (fqn, id) in &extractor.fqn_to_cell_id {
                global_fqn_map.insert(fqn.clone(), id.clone());
            }
        }

        // Pass 2 with empty cache to collect unresolved
        let mut all_unresolved = Vec::new();
        for file_path in &files {
            let rel_path = file_path
                .strip_prefix(&config.root_path)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();

            if let Ok(extractor) = self.extract_relations_from_file(
                file_path,
                &rel_path,
                &global_fqn_map,
                &HashMap::new(),
                &mut factory,
            ) {
                for u in &extractor.unresolved {
                    all_unresolved.push(UnresolvedInfo {
                        target_text: u.target_text.clone(),
                        file: rel_path.clone(),
                        line: u.witness.start_line,
                        column: u.witness.start_col,
                    });
                }
            }
        }

        Ok(all_unresolved)
    }

    fn extract_entities_from_file(
        &mut self,
        file_path: &Path,
        root_path: &Path,
        factory: &mut CellFactory,
    ) -> Result<EntityExtractor> {
        let source = read_file_auto_encoding(file_path)?;
        let tree = self
            .parser
            .parse(&source, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse {}", file_path.display()))?;

        let rel_path = file_path
            .strip_prefix(root_path)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        let mut extractor = EntityExtractor::new(&rel_path);
        extractor.extract(tree.root_node(), source.as_bytes(), factory);
        Ok(extractor)
    }

    fn extract_relations_from_file(
        &mut self,
        file_path: &Path,
        rel_path: &str,
        fqn_map: &HashMap<String, Id>,
        resolved_cache: &HashMap<String, String>,
        factory: &mut CellFactory,
    ) -> Result<RelationExtractor> {
        let source = read_file_auto_encoding(file_path)?;
        let tree = self
            .parser
            .parse(&source, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse {}", file_path.display()))?;

        let extractor = RelationExtractor::new(fqn_map.clone(), rel_path)
            .with_resolved_cache(resolved_cache.clone());
        let mut extractor = extractor;
        extractor.extract(tree.root_node(), source.as_bytes(), factory);
        Ok(extractor)
    }
}

fn read_file_auto_encoding(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    // Try UTF-8 first
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return Ok(s.to_string());
    }
    // Try Shift-JIS (common for Japanese Java projects)
    let (decoded, _, had_errors) = encoding_rs::SHIFT_JIS.decode(&bytes);
    if !had_errors {
        return Ok(decoded.into_owned());
    }
    // Try EUC-JP
    let (decoded, _, _) = encoding_rs::EUC_JP.decode(&bytes);
    Ok(decoded.into_owned())
}

fn cell_type_to_entity_type(cell_type: &str) -> specgraphen_model::JavaEntityType {
    match cell_type {
        "java.package" => specgraphen_model::JavaEntityType::Package,
        "java.class" => specgraphen_model::JavaEntityType::Class,
        "java.interface" => specgraphen_model::JavaEntityType::Interface,
        "java.enum" => specgraphen_model::JavaEntityType::Enum,
        "java.enum_constant" => specgraphen_model::JavaEntityType::EnumConstant,
        "java.annotation" => specgraphen_model::JavaEntityType::Annotation,
        "java.record" => specgraphen_model::JavaEntityType::Record,
        "java.method" => specgraphen_model::JavaEntityType::Method,
        "java.constructor" => specgraphen_model::JavaEntityType::Constructor,
        "java.field" => specgraphen_model::JavaEntityType::Field,
        _ => specgraphen_model::JavaEntityType::Class,
    }
}
