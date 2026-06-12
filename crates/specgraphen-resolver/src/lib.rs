//! `TypeResolver` trait with LSP-backed and heuristic implementations.

pub mod chain;
pub mod heuristic;
pub mod java;
pub mod lsp_client;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct ResolveContext {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub package: Option<String>,
    pub class_fqn: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSymbol {
    pub fqn: String,
    pub kind: SymbolKind,
    pub source: ResolutionSource,
    pub file: Option<String>,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Class,
    Interface,
    Enum,
    Method,
    Constructor,
    Field,
    Package,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionSource {
    Lsp,
    Heuristic,
}

#[async_trait]
pub trait TypeResolver: Send + Sync {
    async fn resolve_type(&self, type_name: &str, ctx: &ResolveContext) -> Vec<ResolvedSymbol>;

    async fn resolve_method_call(
        &self,
        method: &str,
        object: Option<&str>,
        ctx: &ResolveContext,
    ) -> Vec<ResolvedSymbol>;

    async fn find_references(&self, fqn: &str, ctx: &ResolveContext) -> Vec<ResolvedSymbol>;

    fn name(&self) -> &str;
}
