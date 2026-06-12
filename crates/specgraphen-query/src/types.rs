use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ExplainResult {
    pub symbol: String,
    pub entity_type: String,
    pub signature: String,
    pub intent: Option<ConfidenceWrapped<String>>,
    pub behavior: Option<ConfidenceWrapped<String>>,
    pub preconditions: Vec<ConfidenceWrapped<String>>,
    pub postconditions: Vec<ConfidenceWrapped<String>>,
    pub side_effects: Vec<ConfidenceWrapped<String>>,
    pub error_behavior: Option<ConfidenceWrapped<String>>,
    pub witnesses: Vec<WitnessRef>,
    pub obstructions: Vec<String>,
    pub callers: Vec<String>,
    pub callees: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfidenceWrapped<T> {
    pub value: T,
    pub confidence: f64,
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WitnessRef {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub derivation_source: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CallGraphResult {
    pub symbol: String,
    pub relations: Vec<CallRelation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CallRelation {
    pub target: String,
    pub target_label: String,
    pub relation_type: String,
    pub confidence: f64,
    pub witness: Option<WitnessRef>,
}
