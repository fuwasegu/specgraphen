use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;

/// Projected view with HG-style information loss tracking.
/// Uses HG's ProjectionAudience/Purpose concepts without constructing
/// the full Projection object (which requires more context).
#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectedView {
    pub audience: String,
    pub purpose: String,
    pub content: serde_json::Value,
    pub information_loss: Vec<LossEntry>,
    pub source_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LossEntry {
    pub description: String,
    pub severity: String,
}

pub fn project_for_agent(
    space_data: &SpaceData,
    content: serde_json::Value,
    source_count: usize,
) -> ProjectedView {
    let total_entities = space_data.cells.len();
    let omitted = total_entities.saturating_sub(source_count);

    let mut loss_entries = Vec::new();
    if omitted > 0 {
        loss_entries.push(LossEntry {
            description: format!("{omitted} of {total_entities} entities omitted from this view"),
            severity: "low".to_string(),
        });
    }

    if space_data.annotations.is_empty() {
        loss_entries.push(LossEntry {
            description: "No semantic annotations available (run enrich to add)".to_string(),
            severity: "medium".to_string(),
        });
    }

    ProjectedView {
        audience: "ai_agent".to_string(),
        purpose: "query_result".to_string(),
        content,
        information_loss: loss_entries,
        source_count,
    }
}
