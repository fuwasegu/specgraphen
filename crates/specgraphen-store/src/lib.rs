//! JSON file persistence for specgraphen spaces (`SpaceStore` trait).

mod json_store;

use async_trait::async_trait;
use specgraphen_model::SpaceData;

pub use json_store::JsonFileStore;

#[async_trait]
pub trait SpaceStore: Send + Sync {
    async fn save(&self, space_data: &SpaceData) -> anyhow::Result<()>;
    async fn load(&self, space_id: &str) -> anyhow::Result<SpaceData>;
    async fn list_spaces(&self) -> anyhow::Result<Vec<SpaceMetadata>>;
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpaceMetadata {
    pub space_id: String,
    pub space_name: String,
    pub entity_count: usize,
    pub relation_count: usize,
}
