use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SemanticAnnotation {
    pub intent: Option<String>,
    pub behavior: Option<String>,
    pub preconditions: Vec<String>,
    pub postconditions: Vec<String>,
    pub invariants: Vec<String>,
    pub side_effects: Vec<String>,
    pub error_behavior: Option<String>,
}
