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

impl SemanticAnnotation {
    /// True if any semantic field carries content (i.e. this annotation is
    /// more than an empty placeholder).
    pub fn has_content(&self) -> bool {
        self.intent.is_some()
            || self.behavior.is_some()
            || self.error_behavior.is_some()
            || !self.preconditions.is_empty()
            || !self.postconditions.is_empty()
            || !self.side_effects.is_empty()
            || !self.invariants.is_empty()
    }
}
