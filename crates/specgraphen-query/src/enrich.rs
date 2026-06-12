use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;

use crate::resolve::resolve_symbol;

#[derive(Debug, Serialize, Deserialize)]
pub struct EnrichRequest {
    pub symbol: String,
    pub entity_type: String,
    pub signature: String,
    pub source_code: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub callers: Vec<String>,
    pub callees: Vec<String>,
    pub containing_class: Option<String>,
    pub instructions: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EnrichBatchRequest {
    pub entities: Vec<EnrichRequest>,
    pub instructions: String,
}

pub fn enrich(
    space_data: &SpaceData,
    symbol: &str,
    source_files: &HashMap<String, String>,
) -> Result<EnrichRequest> {
    let (fqn, cell_id) = resolve_symbol(space_data, symbol)
        .ok_or_else(|| anyhow::anyhow!("Symbol not found: {symbol}"))?;

    let entity = space_data
        .entities
        .iter()
        .find(|e| e.cell_id == cell_id)
        .ok_or_else(|| anyhow::anyhow!("Entity not found: {cell_id}"))?;

    let cell = space_data
        .cells
        .iter()
        .find(|c| c.id.as_str() == cell_id)
        .ok_or_else(|| anyhow::anyhow!("Cell not found: {cell_id}"))?;

    // Get source code from the file
    let source_code = if !entity.witness.file.is_empty() {
        if let Some(content) = source_files.get(&entity.witness.file) {
            extract_lines(
                content,
                entity.witness.start_line as usize,
                entity.witness.end_line as usize,
            )
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Get callers
    let callers: Vec<String> = space_data
        .incidences
        .iter()
        .filter(|i| {
            i.to_cell_id.as_str() == cell_id
                && (i.relation_type == "java.calls" || i.relation_type == "java.constructs")
        })
        .filter_map(|i| {
            space_data
                .fqn_to_cell_id
                .iter()
                .find(|(_, v)| v.as_str() == i.from_cell_id.as_str())
                .map(|(k, _)| k.clone())
        })
        .collect();

    // Get callees
    let callees: Vec<String> = space_data
        .incidences
        .iter()
        .filter(|i| {
            i.from_cell_id.as_str() == cell_id
                && (i.relation_type == "java.calls" || i.relation_type == "java.constructs")
        })
        .filter_map(|i| {
            space_data
                .fqn_to_cell_id
                .iter()
                .find(|(_, v)| v.as_str() == i.to_cell_id.as_str())
                .map(|(k, _)| k.clone())
        })
        .collect();

    // Find containing class
    let containing_class = space_data
        .incidences
        .iter()
        .find(|i| i.from_cell_id.as_str() == cell_id && i.relation_type == "java.contained_in")
        .and_then(|i| {
            space_data
                .fqn_to_cell_id
                .iter()
                .find(|(_, v)| v.as_str() == i.to_cell_id.as_str())
                .map(|(k, _)| k.clone())
        });

    let instructions = format!(
        r#"Analyze this Java {} `{}` and provide a structured semantic annotation.

Return a JSON object with these fields:
- "intent": one-line purpose (string)
- "behavior": step-by-step behavioral description (string)
- "preconditions": array of precondition strings
- "postconditions": array of postcondition strings
- "side_effects": array of side effect strings
- "error_behavior": how errors are handled (string)

For every claim, cite the specific line number(s) from the source code.
Be precise and factual — only describe what the code actually does."#,
        entity.entity_type.cell_type_str(),
        fqn
    );

    Ok(EnrichRequest {
        symbol: fqn.to_string(),
        entity_type: entity.entity_type.cell_type_str().to_string(),
        signature: cell.label.clone().unwrap_or_default(),
        source_code,
        file: entity.witness.file.clone(),
        start_line: entity.witness.start_line,
        end_line: entity.witness.end_line,
        callers,
        callees,
        containing_class,
        instructions,
    })
}

pub fn enrich_batch(
    space_data: &SpaceData,
    scope: Option<&str>,
    limit: usize,
    source_files: &HashMap<String, String>,
) -> Result<EnrichBatchRequest> {
    let scope_lower = scope.map(|s| s.to_lowercase());

    // Find entities that need enrichment (no annotations yet) and are methods/classes
    let candidates: Vec<_> = space_data
        .entities
        .iter()
        .filter(|e| {
            matches!(
                e.entity_type,
                specgraphen_model::JavaEntityType::Method
                    | specgraphen_model::JavaEntityType::Constructor
                    | specgraphen_model::JavaEntityType::Class
                    | specgraphen_model::JavaEntityType::Interface
            ) && !space_data.annotations.contains_key(&e.cell_id)
                && scope_lower
                    .as_ref()
                    .map(|s| e.fqn.to_lowercase().contains(s))
                    .unwrap_or(true)
        })
        .take(limit)
        .collect();

    let mut entities = Vec::new();
    for candidate in candidates {
        if let Ok(req) = enrich(space_data, &candidate.fqn, source_files) {
            if !req.source_code.is_empty() {
                entities.push(req);
            }
        }
    }

    Ok(EnrichBatchRequest {
        entities,
        instructions: "Analyze each entity and call the `annotate` tool with the results."
            .to_string(),
    })
}

fn extract_lines(content: &str, start: usize, end: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let start_idx = if start > 0 { start - 1 } else { 0 };
    let end_idx = end.min(lines.len());
    lines[start_idx..end_idx].join("\n")
}
