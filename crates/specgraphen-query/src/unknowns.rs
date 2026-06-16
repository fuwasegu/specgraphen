use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;

#[derive(Debug, Serialize, Deserialize)]
pub struct UnknownsResult {
    pub scope: String,
    pub unresolved_references: Vec<UnresolvedRef>,
    pub low_confidence_entities: Vec<LowConfidenceEntity>,
    pub obstructions: Vec<String>,
    pub total_unknowns: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UnresolvedRef {
    pub from_fqn: String,
    pub target: String,
    pub relation_type: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LowConfidenceEntity {
    pub fqn: String,
    pub entity_type: String,
    pub confidence: f64,
    pub reason: String,
}

pub fn unknowns(space_data: &SpaceData, scope: Option<&str>) -> Result<UnknownsResult> {
    let scope_str = scope.unwrap_or("all").to_string();
    let scope_lower = scope.map(|s| s.to_lowercase());

    // Obstructions stored during lift/corroboration
    let obstructions: Vec<String> = space_data
        .obstructions
        .iter()
        .filter(|o| {
            scope_lower
                .as_ref()
                .map(|s| o.to_lowercase().contains(s))
                .unwrap_or(true)
        })
        .cloned()
        .collect();

    // Entities without provenance or with low confidence
    let mut low_confidence: Vec<LowConfidenceEntity> = Vec::new();
    for entity in &space_data.entities {
        if let Some(cell) = space_data
            .cells
            .iter()
            .find(|c| c.id.as_str() == entity.cell_id)
        {
            if let Some(ref prov) = cell.provenance {
                let conf = prov.confidence.value();
                if conf < 0.5
                    && scope_lower
                        .as_ref()
                        .map(|s| entity.fqn.to_lowercase().contains(s))
                        .unwrap_or(true)
                {
                    low_confidence.push(LowConfidenceEntity {
                        fqn: entity.fqn.clone(),
                        entity_type: entity.entity_type.cell_type_str().to_string(),
                        confidence: conf,
                        reason: "Low corroboration confidence".to_string(),
                    });
                }
            } else if scope_lower
                .as_ref()
                .map(|s| entity.fqn.to_lowercase().contains(s))
                .unwrap_or(true)
            {
                low_confidence.push(LowConfidenceEntity {
                    fqn: entity.fqn.clone(),
                    entity_type: entity.entity_type.cell_type_str().to_string(),
                    confidence: 0.0,
                    reason: "No provenance".to_string(),
                });
            }
        }
    }

    // Unresolved references collected during lift (calls/constructions that
    // could not be bound to a known cell) — surfaced as Known Unknowns.
    let unresolved_references: Vec<UnresolvedRef> = space_data
        .unresolved
        .iter()
        .filter(|u| {
            scope_lower
                .as_ref()
                .map(|s| {
                    u.from_fqn.to_lowercase().contains(s)
                        || u.target_text.to_lowercase().contains(s)
                })
                .unwrap_or(true)
        })
        .map(|u| UnresolvedRef {
            from_fqn: u.from_fqn.clone(),
            target: u.target_text.clone(),
            relation_type: u.relation_type.relation_type_str().to_string(),
            file: u.file.clone(),
            line: u.line,
        })
        .collect();

    let total_unknowns = obstructions.len() + low_confidence.len() + unresolved_references.len();

    Ok(UnknownsResult {
        scope: scope_str,
        unresolved_references,
        low_confidence_entities: low_confidence,
        obstructions,
        total_unknowns,
    })
}
