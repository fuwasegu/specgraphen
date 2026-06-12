//! Lifts Java source into a Higher Graphen Space using tree-sitter, with file:line witnesses.

mod entity_extractor;
mod file_walker;
pub mod lifter;
mod relation_extractor;

pub use lifter::{
    DiagnosticSeverity, JavaLifter, LiftConfig, LiftDiagnostic, LiftResult, UnresolvedInfo,
};
