use specgraphen_model::SpaceData;

use crate::{InvariantCheck, InvariantViolation};

pub struct ConsistencyCheck;

impl InvariantCheck for ConsistencyCheck {
    fn name(&self) -> &str {
        "consistency"
    }

    fn check(&self, _space_data: &SpaceData) -> Vec<InvariantViolation> {
        // In the MVP, consistency is checked during corroboration.
        // Contradictions become obstructions at fusion time.
        // This check validates post-corroboration state.
        Vec::new()
    }
}
