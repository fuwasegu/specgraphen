//! Lifts Java source into a Higher Graphen Space using tree-sitter, with file:line witnesses.

pub mod decision_extractor;
mod entity_extractor;
mod file_walker;
pub mod lifter;
mod relation_extractor;
pub mod tag_extractor;

pub use decision_extractor::{DecisionExtraction, DecisionExtractor, MethodDecision};
pub use lifter::{
    DiagnosticSeverity, JavaLifter, LiftConfig, LiftDiagnostic, LiftResult, UnresolvedInfo,
};
pub use tag_extractor::{TagDecision, TagExtraction, TagExtractor};
