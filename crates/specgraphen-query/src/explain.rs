use anyhow::Result;
use specgraphen_model::SpaceData;

use crate::resolve::resolve_symbol;
use crate::types::{ConfidenceWrapped, ExplainResult, WitnessRef};

pub fn explain(space_data: &SpaceData, symbol: &str) -> Result<ExplainResult> {
    let (fqn, cell_id) = resolve_symbol(space_data, symbol)
        .ok_or_else(|| anyhow::anyhow!("Symbol not found: {symbol}"))?;

    let cell = space_data
        .cells
        .iter()
        .find(|c| c.id.as_str() == cell_id)
        .ok_or_else(|| anyhow::anyhow!("Cell not found: {cell_id}"))?;

    // Get accurate witness from entity records
    let entity_record = space_data.entities.iter().find(|e| e.cell_id == cell_id);

    let mut witnesses = Vec::new();
    if let Some(entity) = entity_record {
        let w = &entity.witness;
        if !w.file.is_empty() {
            witnesses.push(WitnessRef {
                file: w.file.clone(),
                start_line: w.start_line,
                end_line: w.end_line,
                derivation_source: "tree-sitter".to_string(),
            });
        }
    } else if let Some(ref prov) = cell.provenance {
        if prov.source.uri.is_some() {
            witnesses.push(WitnessRef {
                file: prov.source.title.clone().unwrap_or_default(),
                start_line: 0,
                end_line: 0,
                derivation_source: "tree-sitter".to_string(),
            });
        }
    }

    let annotation = space_data.annotations.get(cell_id);
    let base_confidence = cell
        .provenance
        .as_ref()
        .map(|p| p.confidence.value())
        .unwrap_or(0.5);

    let intent = annotation
        .and_then(|a| a.intent.as_ref())
        .map(|v| ConfidenceWrapped {
            value: v.clone(),
            confidence: base_confidence,
            source: "corroboration".to_string(),
        });

    let behavior = annotation
        .and_then(|a| a.behavior.as_ref())
        .map(|v| ConfidenceWrapped {
            value: v.clone(),
            confidence: base_confidence,
            source: "corroboration".to_string(),
        });

    let preconditions = annotation
        .map(|a| {
            a.preconditions
                .iter()
                .map(|v| ConfidenceWrapped {
                    value: v.clone(),
                    confidence: base_confidence,
                    source: "llm".to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let postconditions = annotation
        .map(|a| {
            a.postconditions
                .iter()
                .map(|v| ConfidenceWrapped {
                    value: v.clone(),
                    confidence: base_confidence,
                    source: "llm".to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let side_effects = annotation
        .map(|a| {
            a.side_effects
                .iter()
                .map(|v| ConfidenceWrapped {
                    value: v.clone(),
                    confidence: base_confidence,
                    source: "llm".to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let error_behavior =
        annotation
            .and_then(|a| a.error_behavior.as_ref())
            .map(|v| ConfidenceWrapped {
                value: v.clone(),
                confidence: base_confidence,
                source: "llm".to_string(),
            });

    // Callers: incoming Calls/Constructs — return FQN not just label
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

    // Callees: outgoing Calls/Constructs — return FQN
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

    Ok(ExplainResult {
        symbol: fqn.to_string(),
        entity_type: cell.cell_type.clone(),
        signature: cell.label.clone().unwrap_or_default(),
        intent,
        behavior,
        preconditions,
        postconditions,
        side_effects,
        error_behavior,
        witnesses,
        obstructions: Vec::new(),
        callers,
        callees,
    })
}
