use higher_graphen_structure::space::traversal::CycleSearchOptions;
use specgraphen_model::SpaceData;

use crate::{InvariantCheck, InvariantViolation, ViolationSeverity};

pub struct AcyclicityCheck;

impl InvariantCheck for AcyclicityCheck {
    fn name(&self) -> &str {
        "acyclicity"
    }

    fn check(&self, space_data: &SpaceData) -> Vec<InvariantViolation> {
        let store = match space_data.store() {
            Some(s) => s,
            None => return Vec::new(),
        };

        let space_id = &space_data.space.id;

        let options = CycleSearchOptions::new()
            .with_relation_type("java.extends")
            .with_max_cycles(10)
            .with_max_path_length(20);

        match store.find_simple_cycles(space_id, &options) {
            Ok(cycles) => cycles
                .iter()
                .map(|cycle| {
                    let vertex_names: Vec<String> = cycle
                        .vertex_cell_ids
                        .iter()
                        .filter_map(|id| {
                            space_data
                                .cells
                                .iter()
                                .find(|c| c.id == *id)
                                .and_then(|c| c.label.clone())
                        })
                        .collect();

                    InvariantViolation {
                        invariant_name: "acyclicity".to_string(),
                        cell_id: cycle
                            .vertex_cell_ids
                            .first()
                            .map(|id| id.as_str().to_string()),
                        message: format!(
                            "Inheritance cycle detected via HG engine: {}",
                            vertex_names.join(" → ")
                        ),
                        severity: ViolationSeverity::Error,
                    }
                })
                .collect(),
            Err(e) => {
                tracing::warn!("Cycle search failed: {e}");
                Vec::new()
            }
        }
    }
}
