use std::collections::HashMap;

use async_trait::async_trait;

use crate::{ResolutionSource, ResolveContext, ResolvedSymbol, SymbolKind, TypeResolver};

pub struct HeuristicResolver {
    fqn_to_cell_id: HashMap<String, String>,
}

impl HeuristicResolver {
    pub fn new(fqn_to_cell_id: HashMap<String, String>) -> Self {
        Self { fqn_to_cell_id }
    }
}

#[async_trait]
impl TypeResolver for HeuristicResolver {
    async fn resolve_type(&self, type_name: &str, ctx: &ResolveContext) -> Vec<ResolvedSymbol> {
        let mut candidates = Vec::new();

        if type_name.contains('.') {
            candidates.push(type_name.to_string());
        }

        if let Some(ref pkg) = ctx.package {
            candidates.push(format!("{pkg}.{type_name}"));
        }

        for fqn in self.fqn_to_cell_id.keys() {
            if (fqn.ends_with(&format!(".{type_name}")) || fqn == type_name)
                && !candidates.contains(fqn)
            {
                candidates.push(fqn.clone());
            }
        }

        candidates
            .into_iter()
            .filter(|c| self.fqn_to_cell_id.contains_key(c))
            .map(|fqn| ResolvedSymbol {
                fqn,
                kind: SymbolKind::Class,
                source: ResolutionSource::Heuristic,
                file: None,
                line: None,
            })
            .collect()
    }

    async fn resolve_method_call(
        &self,
        method: &str,
        object: Option<&str>,
        ctx: &ResolveContext,
    ) -> Vec<ResolvedSymbol> {
        let mut candidates = Vec::new();

        if let Some(obj) = object {
            if let Some(ref pkg) = ctx.package {
                candidates.push(format!("{pkg}.{obj}.{method}"));
            }
            candidates.push(format!("{obj}.{method}"));
        } else if let Some(ref class_fqn) = ctx.class_fqn {
            candidates.push(format!("{class_fqn}.{method}"));
        }

        candidates
            .into_iter()
            .filter(|c| self.fqn_to_cell_id.contains_key(c))
            .map(|fqn| ResolvedSymbol {
                fqn,
                kind: SymbolKind::Method,
                source: ResolutionSource::Heuristic,
                file: None,
                line: None,
            })
            .collect()
    }

    async fn find_references(&self, _fqn: &str, _ctx: &ResolveContext) -> Vec<ResolvedSymbol> {
        Vec::new()
    }

    fn name(&self) -> &str {
        "heuristic"
    }
}
