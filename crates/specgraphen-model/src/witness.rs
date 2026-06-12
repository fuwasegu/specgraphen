use serde::{Deserialize, Serialize};

use crate::DerivationSource;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WitnessInfo {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_col: u32,
    pub end_col: u32,
    pub derivation_source: DerivationSource,
}
