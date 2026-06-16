//! Fuses multiple derivations of the same fact and computes Bayesian confidence.

mod completion;
mod confidence;
pub mod correspondence;
mod engine;
mod fusion;

pub use completion::{run_obstruction_completion, CompletionStats};
pub use correspondence::{CorrespondenceAgreement, CorrespondenceResult};
pub use engine::{CorroborationConfig, CorroborationEngine};
