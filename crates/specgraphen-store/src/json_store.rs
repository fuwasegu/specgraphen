use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;
use specgraphen_model::SpaceData;

use crate::{SpaceMetadata, SpaceStore};

pub struct JsonFileStore {
    root_dir: PathBuf,
}

impl JsonFileStore {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
        }
    }

    fn space_dir(&self, space_id: &str) -> PathBuf {
        self.root_dir.join("spaces").join(space_id)
    }

    fn space_file(&self, space_id: &str) -> PathBuf {
        self.space_dir(space_id).join("space.json")
    }
}

#[async_trait]
impl SpaceStore for JsonFileStore {
    async fn save(&self, space_data: &SpaceData) -> Result<()> {
        let space_id = space_data.space.id.as_str();
        let dir = self.space_dir(space_id);
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("Failed to create directory: {}", dir.display()))?;

        let json =
            serde_json::to_string_pretty(space_data).context("Failed to serialize SpaceData")?;

        let path = self.space_file(space_id);
        tokio::fs::write(&path, json)
            .await
            .with_context(|| format!("Failed to write {}", path.display()))?;

        tracing::info!(%space_id, path = %path.display(), "Space saved");
        Ok(())
    }

    async fn load(&self, space_id: &str) -> Result<SpaceData> {
        let path = self.space_file(space_id);
        let json = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let mut space_data: SpaceData =
            serde_json::from_str(&json).context("Failed to deserialize SpaceData")?;

        space_data.rebuild_store();

        Ok(space_data)
    }

    async fn list_spaces(&self) -> Result<Vec<SpaceMetadata>> {
        let spaces_dir = self.root_dir.join("spaces");
        if !spaces_dir.exists() {
            return Ok(Vec::new());
        }

        let mut result = Vec::new();
        let mut entries = tokio::fs::read_dir(&spaces_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let space_id = entry.file_name().to_string_lossy().to_string();
                let space_file = entry.path().join("space.json");
                if space_file.exists() {
                    match tokio::fs::read_to_string(&space_file).await {
                        Ok(json) => {
                            if let Ok(data) = serde_json::from_str::<SpaceData>(&json) {
                                result.push(SpaceMetadata {
                                    space_id,
                                    space_name: data.space.name.clone(),
                                    entity_count: data.cells.len(),
                                    relation_count: data.incidences.len(),
                                });
                            }
                        }
                        Err(_) => continue,
                    }
                }
            }
        }

        Ok(result)
    }
}
