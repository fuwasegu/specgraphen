use higher_graphen_structure::space::traversal::{
    ReachabilityQuery, TraversalDirection, TraversalOptions,
};
use specgraphen_model::SpaceData;

use crate::{InvariantCheck, InvariantViolation, ViolationSeverity};

pub struct ReachabilityCheck;

impl InvariantCheck for ReachabilityCheck {
    fn name(&self) -> &str {
        "reachability"
    }

    fn check(&self, space_data: &SpaceData) -> Vec<InvariantViolation> {
        if space_data.cells.is_empty() {
            return Vec::new();
        }

        let store = match space_data.store() {
            Some(s) => s,
            None => return self.fallback_check(space_data),
        };

        let space_id = &space_data.space.id;
        let root_id = &space_data.cells[0].id;

        let options = TraversalOptions::new().in_direction(TraversalDirection::Both);

        let mut violations = Vec::new();
        for cell in &space_data.cells {
            if cell.id == *root_id {
                continue;
            }

            let query = ReachabilityQuery::new(space_id.clone(), root_id.clone(), cell.id.clone())
                .with_options(options.clone());

            match store.reachable(&query) {
                Ok(result) if result.reachable => {}
                _ => {
                    violations.push(InvariantViolation {
                        invariant_name: "reachability".to_string(),
                        cell_id: Some(cell.id.as_str().to_string()),
                        message: format!(
                            "Cell '{}' is unreachable from the space root (HG engine)",
                            cell.label.as_deref().unwrap_or("?")
                        ),
                        severity: ViolationSeverity::Warning,
                    });
                }
            }
        }

        violations
    }
}

impl ReachabilityCheck {
    fn fallback_check(&self, space_data: &SpaceData) -> Vec<InvariantViolation> {
        use std::collections::{HashSet, VecDeque};

        if space_data.cells.is_empty() {
            return Vec::new();
        }

        let mut adjacency: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for inc in &space_data.incidences {
            let from = inc.from_cell_id.as_str().to_string();
            let to = inc.to_cell_id.as_str().to_string();
            adjacency.entry(from.clone()).or_default().push(to.clone());
            adjacency.entry(to).or_default().push(from);
        }

        let start = space_data.cells[0].id.as_str().to_string();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        visited.insert(start.clone());
        queue.push_back(start);

        while let Some(current) = queue.pop_front() {
            if let Some(neighbors) = adjacency.get(&current) {
                for neighbor in neighbors {
                    if visited.insert(neighbor.clone()) {
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }

        space_data
            .cells
            .iter()
            .filter(|cell| !visited.contains(cell.id.as_str()))
            .map(|cell| InvariantViolation {
                invariant_name: "reachability".to_string(),
                cell_id: Some(cell.id.as_str().to_string()),
                message: format!(
                    "Cell '{}' is unreachable from the space root",
                    cell.label.as_deref().unwrap_or("?")
                ),
                severity: ViolationSeverity::Warning,
            })
            .collect()
    }
}
