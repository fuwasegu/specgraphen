use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub total_matches: usize,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchMatch {
    pub fqn: String,
    pub entity_type: String,
    pub label: String,
    pub file: String,
    pub line: u32,
}

pub fn search(
    space_data: &SpaceData,
    query: &str,
    entity_type_filter: Option<&str>,
    limit: usize,
) -> Result<SearchResult> {
    let query_lower = query.to_lowercase();

    let mut matches: Vec<SearchMatch> = space_data
        .entities
        .iter()
        .filter(|e| {
            let fqn_match = e.fqn.to_lowercase().contains(&query_lower)
                || e.label.to_lowercase().contains(&query_lower);

            let type_match = entity_type_filter
                .map(|t| {
                    let t_lower = t.to_lowercase();
                    format!("{:?}", e.entity_type).to_lowercase() == t_lower
                        || e.entity_type.cell_type_str().contains(&t_lower)
                })
                .unwrap_or(true);

            fqn_match && type_match
        })
        .map(|e| SearchMatch {
            fqn: e.fqn.clone(),
            entity_type: e.entity_type.cell_type_str().to_string(),
            label: e.label.clone(),
            file: e.witness.file.clone(),
            line: e.witness.start_line,
        })
        .collect();

    matches.sort_by(|a, b| {
        // Exact prefix match first
        let a_starts = a.fqn.to_lowercase().starts_with(&query_lower)
            || a.label.to_lowercase().starts_with(&query_lower);
        let b_starts = b.fqn.to_lowercase().starts_with(&query_lower)
            || b.label.to_lowercase().starts_with(&query_lower);
        b_starts.cmp(&a_starts).then(a.fqn.cmp(&b.fqn))
    });

    let total_matches = matches.len();
    let truncated = total_matches > limit;
    matches.truncate(limit);

    Ok(SearchResult {
        query: query.to_string(),
        matches,
        total_matches,
        truncated,
    })
}
