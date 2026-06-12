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

impl DerivationSource {
    /// Stable identifier used as the provenance `extraction_method`.
    pub fn extraction_method_str(&self) -> &'static str {
        match self {
            Self::TreeSitter => "tree-sitter-java",
            Self::Lsp => "lsp",
            Self::LlmBehavior => "llm-behavior",
            Self::LlmContract => "llm-contract",
            Self::LlmInvariant => "llm-invariant",
            Self::Test => "test",
            Self::TypeSystem => "type-system",
        }
    }
}
