//! Fuses multiple derivations of the same fact and computes Bayesian confidence.

mod confidence;
pub mod correspondence;
mod engine;
mod fusion;

pub use correspondence::{CorrespondenceAgreement, CorrespondenceResult};
pub use engine::{CorroborationConfig, CorroborationEngine};
