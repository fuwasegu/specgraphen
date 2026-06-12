use specgraphen_model::SpaceData;

use crate::{InvariantCheck, InvariantViolation, ViolationSeverity};

pub struct GroundingCheck;

impl InvariantCheck for GroundingCheck {
    fn name(&self) -> &str {
        "grounding"
    }

    fn check(&self, space_data: &SpaceData) -> Vec<InvariantViolation> {
        let mut violations = Vec::new();

        for cell in &space_data.cells {
            if cell.provenance.is_none() {
                violations.push(InvariantViolation {
                    invariant_name: "grounding".to_string(),
                    cell_id: Some(cell.id.as_str().to_string()),
                    message: format!(
                        "Cell '{}' has no provenance (witness)",
                        cell.label.as_deref().unwrap_or("?")
                    ),
                    severity: ViolationSeverity::Error,
                });
            } else if let Some(ref prov) = cell.provenance {
                if prov.source.uri.is_none() {
                    violations.push(InvariantViolation {
                        invariant_name: "grounding".to_string(),
                        cell_id: Some(cell.id.as_str().to_string()),
                        message: format!(
                            "Cell '{}' provenance has no source URI",
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
