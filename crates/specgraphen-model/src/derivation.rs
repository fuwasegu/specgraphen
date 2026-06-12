use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DerivationSource {
    TreeSitter,
    Lsp,
    LlmBehavior,
    LlmContract,
    LlmInvariant,
    Test,
    TypeSystem,
}
