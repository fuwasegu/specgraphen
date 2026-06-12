use async_trait::async_trait;

use crate::{ResolveContext, ResolvedSymbol, TypeResolver};

pub struct ChainResolver {
    resolvers: Vec<Box<dyn TypeResolver>>,
}

impl ChainResolver {
    pub fn new(resolvers: Vec<Box<dyn TypeResolver>>) -> Self {
        Self { resolvers }
    }
}

#[async_trait]
impl TypeResolver for ChainResolver {
    async fn resolve_type(&self, type_name: &str, ctx: &ResolveContext) -> Vec<ResolvedSymbol> {
        for resolver in &self.resolvers {
            let results = resolver.resolve_type(type_name, ctx).await;
            if !results.is_empty() {
                tracing::trace!(
                    resolver = resolver.name(),
                    type_name,
                    results = results.len(),
                    "Type resolved"
                );
                return results;
            }
        }
        Vec::new()
    }

    async fn resolve_method_call(
        &self,
        method: &str,
        object: Option<&str>,
        ctx: &ResolveContext,
    ) -> Vec<ResolvedSymbol> {
        for resolver in &self.resolvers {
            let results = resolver.resolve_method_call(method, object, ctx).await;
            if !results.is_empty() {
                tracing::trace!(
                    resolver = resolver.name(),
                    method,
                    results = results.len(),
                    "Method call resolved"
                );
                return results;
            }
        }
        Vec::new()
    }

    async fn find_references(&self, fqn: &str, ctx: &ResolveContext) -> Vec<ResolvedSymbol> {
        for resolver in &self.resolvers {
            let results = resolver.find_references(fqn, ctx).await;
            if !results.is_empty() {
                return results;
            }
        }
        Vec::new()
    }

    fn name(&self) -> &str {
        "chain"
    }
}
