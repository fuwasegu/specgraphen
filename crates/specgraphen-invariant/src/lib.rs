//! Structural invariant checks: grounding, consistency, reachability, and acyclicity.

mod acyclicity;
mod consistency;
mod grounding;
mod reachability;

use specgraphen_model::SpaceData;

pub use acyclicity::AcyclicityCheck;
pub use consistency::ConsistencyCheck;
pub use grounding::GroundingCheck;
pub use reachability::ReachabilityCheck;

pub trait InvariantCheck: Send + Sync {
    fn name(&self) -> &str;
    fn check(&self, space_data: &SpaceData) -> Vec<InvariantViolation>;
}

#[derive(Debug, Clone)]
pub struct InvariantViolation {
    pub invariant_name: String,
    pub cell_id: Option<String>,
    pub message: String,
    pub severity: ViolationSeverity,
}

#[derive(Debug, Clone)]
pub enum ViolationSeverity {
    Error,
    Warning,
}

pub struct InvariantChecker {
    checks: Vec<Box<dyn InvariantCheck>>,
}

impl InvariantChecker {
    pub fn default_checks() -> Self {
        Self {
            checks: vec![
                Box::new(GroundingCheck),
                Box::new(ConsistencyCheck),
                Box::new(ReachabilityCheck),
                Box::new(AcyclicityCheck),
            ],
        }
    }

    pub fn check_all(&self, space_data: &SpaceData) -> Vec<InvariantViolation> {
        let mut violations = Vec::new();
        for check in &self.checks {
            let v = check.check(space_data);
            tracing::info!(
                invariant = check.name(),
                violations = v.len(),
                "Invariant check"
            );
            violations.extend(v);
        }
        violations
    }
}
