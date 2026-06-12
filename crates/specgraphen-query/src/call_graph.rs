use anyhow::Result;
use specgraphen_model::SpaceData;

use crate::resolve::resolve_symbol;
use crate::types::{CallGraphResult, CallRelation, WitnessRef};

pub fn callers(space_data: &SpaceData, symbol: &str) -> Result<CallGraphResult> {
    let (fqn, cell_id) = resolve_symbol(space_data, symbol)
        .ok_or_else(|| anyhow::anyhow!("Symbol not found: {symbol}"))?;

    let relations: Vec<CallRelation> = space_data
        .incidences
        .iter()
        .filter(|i| {
            i.to_cell_id.as_str() == cell_id
                && (i.relation_type == "java.calls"
                    || i.relation_type == "java.constructs"
                    || i.relation_type == "java.field_access")
        })
        .map(|i| {
            let caller_cell = space_data.cells.iter().find(|c| c.id == i.from_cell_id);

            let target_fqn = space_data
                .fqn_to_cell_id
                .iter()
                .find(|(_, v)| v.as_str() == i.from_cell_id.as_str())
                .map(|(k, _)| k.clone())
                .unwrap_or_default();

            let confidence = i
                .provenance
                .as_ref()
                .map(|p| p.confidence.value())
                .unwrap_or(0.5);

            let witness = i.provenance.as_ref().map(|p| WitnessRef {
                file: p.source.title.clone().unwrap_or_default(),
                start_line: 0,
                end_line: 0,
                derivation_source: "tree-sitter".to_string(),
            });

            CallRelation {
                target: target_fqn,
                target_label: caller_cell
                    .and_then(|c| c.label.clone())
                    .unwrap_or_default(),
                relation_type: i.relation_type.clone(),
                confidence,
                witness,
            }
        })
        .collect();

    Ok(CallGraphResult {
        symbol: fqn.to_string(),
        relations,
    })
}

pub fn callees(space_data: &SpaceData, symbol: &str) -> Result<CallGraphResult> {
    let (fqn, cell_id) = resolve_symbol(space_data, symbol)
        .ok_or_else(|| anyhow::anyhow!("Symbol not found: {symbol}"))?;

    let relations: Vec<CallRelation> = space_data
        .incidences
        .iter()
        .filter(|i| {
            i.from_cell_id.as_str() == cell_id
                && (i.relation_type == "java.calls"
                    || i.relation_type == "java.constructs"
                    || i.relation_type == "java.field_access")
        })
        .map(|i| {
            let callee_cell = space_data.cells.iter().find(|c| c.id == i.to_cell_id);

            let target_fqn = space_data
                .fqn_to_cell_id
                .iter()
                .find(|(_, v)| v.as_str() == i.to_cell_id.as_str())
                .map(|(k, _)| k.clone())
                .unwrap_or_default();

            let confidence = i
                .provenance
                .as_ref()
                .map(|p| p.confidence.value())
                .unwrap_or(0.5);

            let witness = i.provenance.as_ref().map(|p| WitnessRef {
                file: p.source.title.clone().unwrap_or_default(),
                start_line: 0,
                end_line: 0,
                derivation_source: "tree-sitter".to_string(),
            });

            CallRelation {
                target: target_fqn,
                target_label: callee_cell
                    .and_then(|c| c.label.clone())
                    .unwrap_or_default(),
                relation_type: i.relation_type.clone(),
                confidence,
                witness,
            }
        })
        .collect();

    Ok(CallGraphResult {
        symbol: fqn.to_string(),
        relations,
    })
}
