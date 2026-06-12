use std::collections::HashSet;

use anyhow::Result;
use higher_graphen_structure::space::traversal::{
    ReachabilityQuery, TraversalDirection, TraversalOptions,
};
use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;

use crate::resolve::resolve_symbol;

#[derive(Debug, Serialize, Deserialize)]
pub struct ImpactResult {
    pub changed_symbol: String,
    pub direct_impacts: Vec<ImpactEntry>,
    pub transitive_impacts: Vec<ImpactEntry>,
    pub affected_files: Vec<String>,
    pub total_affected: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImpactEntry {
    pub fqn: String,
    pub label: String,
    pub relation_type: String,
    pub file: String,
    pub line: u32,
    pub depth: usize,
}

pub fn impact(space_data: &SpaceData, symbol: &str, max_depth: usize) -> Result<ImpactResult> {
    let (fqn, cell_id) = resolve_symbol(space_data, symbol)
        .ok_or_else(|| anyhow::anyhow!("Symbol not found: {symbol}"))?;

    if let Some(store) = space_data.store() {
        impact_via_hg(space_data, store, fqn, cell_id, max_depth)
    } else {
        impact_fallback(space_data, fqn, cell_id, max_depth)
    }
}

fn impact_via_hg(
    space_data: &SpaceData,
    store: &higher_graphen_structure::space::InMemorySpaceStore,
    fqn: &str,
    cell_id: &str,
    max_depth: usize,
) -> Result<ImpactResult> {
    let space_id = &space_data.space.id;
    let changed_id = higher_graphen_core::Id::new(cell_id)?;

    let options = TraversalOptions::new()
        .in_direction(TraversalDirection::Incoming)
        .with_relation_type("java.calls")
        .with_relation_type("java.constructs")
        .with_relation_type("java.extends")
        .with_relation_type("java.implements")
        .with_relation_type("java.type_reference")
        .with_max_depth(max_depth);

    let mut direct_impacts = Vec::new();
    let mut transitive_impacts = Vec::new();
    let mut affected_files: HashSet<String> = HashSet::new();
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(cell_id.to_string());

    // Check each cell for reachability to the changed cell (incoming direction)
    for cell in &space_data.cells {
        let cid = cell.id.as_str();
        if visited.contains(cid) {
            continue;
        }

        let query = ReachabilityQuery::new(space_id.clone(), changed_id.clone(), cell.id.clone())
            .with_options(options.clone());

        if let Ok(result) = store.reachable(&query) {
            if result.reachable {
                let depth = result
                    .shortest_path
                    .as_ref()
                    .map(|p| p.steps.len())
                    .unwrap_or(1);

                let entity = space_data.entities.iter().find(|e| e.cell_id == cid);

                if let Some(entity) = entity {
                    let entry = ImpactEntry {
                        fqn: entity.fqn.clone(),
                        label: entity.label.clone(),
                        relation_type: "java.calls".to_string(),
                        file: entity.witness.file.clone(),
                        line: entity.witness.start_line,
                        depth,
                    };

                    if !entity.witness.file.is_empty() {
                        affected_files.insert(entity.witness.file.clone());
                    }

                    if depth == 1 {
                        direct_impacts.push(entry);
                    } else {
                        transitive_impacts.push(entry);
                    }
                }

                visited.insert(cid.to_string());
            }
        }
    }

    let total_affected = direct_impacts.len() + transitive_impacts.len();
    let mut affected_files: Vec<String> = affected_files.into_iter().collect();
    affected_files.sort();

    Ok(ImpactResult {
        changed_symbol: fqn.to_string(),
        direct_impacts,
        transitive_impacts,
        affected_files,
        total_affected,
    })
}

fn impact_fallback(
    space_data: &SpaceData,
    fqn: &str,
    cell_id: &str,
    max_depth: usize,
) -> Result<ImpactResult> {
    use std::collections::VecDeque;

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    let mut direct_impacts = Vec::new();
    let mut transitive_impacts = Vec::new();
    let mut affected_files: HashSet<String> = HashSet::new();

    visited.insert(cell_id.to_string());
    queue.push_back((cell_id.to_string(), 0));

    while let Some((current_id, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        for inc in &space_data.incidences {
            if inc.to_cell_id.as_str() == current_id
                && (inc.relation_type == "java.calls"
                    || inc.relation_type == "java.constructs"
                    || inc.relation_type == "java.extends"
                    || inc.relation_type == "java.implements"
                    || inc.relation_type == "java.type_reference")
            {
                let caller_id = inc.from_cell_id.as_str().to_string();
                if visited.insert(caller_id.clone()) {
                    let entity = space_data.entities.iter().find(|e| e.cell_id == caller_id);

                    if let Some(entity) = entity {
                        let entry = ImpactEntry {
                            fqn: entity.fqn.clone(),
                            label: entity.label.clone(),
                            relation_type: inc.relation_type.clone(),
                            file: entity.witness.file.clone(),
                            line: entity.witness.start_line,
                            depth: depth + 1,
                        };

                        if !entity.witness.file.is_empty() {
                            affected_files.insert(entity.witness.file.clone());
                        }

                        if depth == 0 {
                            direct_impacts.push(entry);
                        } else {
                            transitive_impacts.push(entry);
                        }
                    }

                    queue.push_back((caller_id, depth + 1));
                }
            }
        }
    }

    let total_affected = direct_impacts.len() + transitive_impacts.len();
    let mut affected_files: Vec<String> = affected_files.into_iter().collect();
    affected_files.sort();

    Ok(ImpactResult {
        changed_symbol: fqn.to_string(),
        direct_impacts,
        transitive_impacts,
        affected_files,
        total_affected,
    })
}
