//! `enforces` query: bounded temporal-property check over the relation graph.
//!
//! DESIGN.md §4 lists `enforces` as a first-class agent query. This is a
//! concept realization on top of HG's bounded model checker: from an entry
//! symbol, does a required relation eventually occur within a depth bound?
//!
//! Caveat (established during verification): specgraphen's incidences form a
//! *static relation graph*, not an execution state machine, and method-internal
//! call ordering is not lifted. So this answers a reachability-style question
//! ("does relation R occur from X within depth N"), not a full ∀-path temporal
//! guarantee. Richer rule enforcement (e.g. "every endpoint always calls an
//! auth check before returning") needs ordered/data-flow lifting first.

use anyhow::Result;
use higher_graphen_core::Id;
use higher_graphen_reasoning::model_checking::{
    check_required_event, ModelCheckingOptions, RequiredEventQuery, TemporalCheckStatus,
};
use higher_graphen_structure::space::traversal::TraversalDirection;
use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;

use crate::resolve::resolve_symbol;

#[derive(Debug, Serialize, Deserialize)]
pub struct EnforcesResult {
    pub entry_symbol: String,
    pub required_relation: String,
    /// "satisfied" / "violated" / "unknown" (depth bound exhausted).
    pub status: String,
    /// True when the bounded reachable state space was fully explored.
    pub exhaustive: bool,
    /// Cells visited during the bounded search (FQNs, capped).
    pub visited: Vec<String>,
    /// Structured temporal obstructions from the HG kernel.
    pub obstructions: Vec<String>,
}

pub fn enforces(
    space_data: &SpaceData,
    entry_symbol: &str,
    required_relation: &str,
    max_depth: usize,
) -> Result<EnforcesResult> {
    let (_fqn, cell_id) = resolve_symbol(space_data, entry_symbol)
        .ok_or_else(|| anyhow::anyhow!("Symbol not found: {entry_symbol}"))?;
    let store = space_data
        .store()
        .ok_or_else(|| anyhow::anyhow!("HG store unavailable for model checking"))?;

    let options = ModelCheckingOptions::default()
        .in_direction(TraversalDirection::Outgoing)
        .with_max_depth(max_depth);
    let query = RequiredEventQuery::new(
        space_data.space.id.clone(),
        vec![Id::new(cell_id)?],
        vec![required_relation.to_string()],
    )
    .with_options(options);

    let report = check_required_event(&query, store)
        .map_err(|e| anyhow::anyhow!("HG model checking failed: {e}"))?;

    let status = match report.status {
        TemporalCheckStatus::Satisfied => "satisfied",
        TemporalCheckStatus::Violated => "violated",
        TemporalCheckStatus::Unknown => "unknown",
    };

    let visited: Vec<String> = report
        .visited_cell_ids
        .iter()
        .take(20)
        .map(|id| fqn_of(space_data, id.as_str()))
        .collect();

    Ok(EnforcesResult {
        entry_symbol: entry_symbol.to_string(),
        required_relation: required_relation.to_string(),
        status: status.to_string(),
        exhaustive: report.exhaustive,
        visited,
        obstructions: report
            .obstructions
            .iter()
            .map(|o| format!("{:?}: {}", o.obstruction_type, o.reason))
            .collect(),
    })
}

fn fqn_of(space_data: &SpaceData, cell_id: &str) -> String {
    space_data
        .fqn_to_cell_id
        .iter()
        .find(|(_, v)| v.as_str() == cell_id)
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| cell_id.to_string())
}
