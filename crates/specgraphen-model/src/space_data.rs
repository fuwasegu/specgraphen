use std::collections::HashMap;

use higher_graphen_structure::space::{Cell, InMemorySpaceStore, Incidence, Space};
use serde::{Deserialize, Serialize};

use crate::semantic::SemanticAnnotation;
use crate::witness::WitnessInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRecord {
    pub fqn: String,
    pub cell_id: String,
    pub entity_type: crate::JavaEntityType,
    pub label: String,
    pub witness: WitnessInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationRecord {
    pub incidence_id: String,
    pub relation_type: crate::JavaRelationType,
    pub from_fqn: String,
    pub to_fqn: String,
    pub witness: WitnessInfo,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SpaceData {
    pub space: Space,
    pub cells: Vec<Cell>,
    pub incidences: Vec<Incidence>,
    pub entities: Vec<EntityRecord>,
    pub relations: Vec<RelationRecord>,
    pub fqn_to_cell_id: HashMap<String, String>,
    #[serde(default)]
    pub annotations: HashMap<String, SemanticAnnotation>,
    #[serde(default)]
    pub obstructions: Vec<String>,
    #[serde(skip)]
    store: Option<InMemorySpaceStore>,
}

impl SpaceData {
    pub fn new(
        space: Space,
        cells: Vec<Cell>,
        incidences: Vec<Incidence>,
        entities: Vec<EntityRecord>,
        relations: Vec<RelationRecord>,
        fqn_to_cell_id: HashMap<String, String>,
    ) -> Self {
        Self {
            space,
            cells,
            incidences,
            entities,
            relations,
            fqn_to_cell_id,
            annotations: HashMap::new(),
            obstructions: Vec::new(),
            store: None,
        }
    }

    pub fn rebuild_store(&mut self) {
        let mut store = InMemorySpaceStore::new();

        // Insert a clean space (store requires empty cell_ids/incidence_ids)
        let clean_space = Space::new(self.space.id.clone(), &self.space.name);
        if let Err(e) = store.insert_space(clean_space) {
            tracing::warn!("Failed to insert space into store: {e}");
            return;
        }

        for cell in &self.cells {
            let mut clean_cell = Cell::new(
                cell.id.clone(),
                cell.space_id.clone(),
                cell.dimension,
                &cell.cell_type,
            );
            if let Some(ref label) = cell.label {
                clean_cell = clean_cell.with_label(label);
            }
            if let Some(ref prov) = cell.provenance {
                clean_cell = clean_cell.with_provenance(prov.clone());
            }
            if let Err(e) = store.insert_cell(clean_cell) {
                tracing::trace!("Failed to insert cell {}: {e}", cell.id.as_str());
            }
        }

        for inc in &self.incidences {
            let clean_inc = Incidence::new(
                inc.id.clone(),
                inc.space_id.clone(),
                inc.from_cell_id.clone(),
                inc.to_cell_id.clone(),
                &inc.relation_type,
                inc.orientation,
            );
            let clean_inc = if let Some(ref prov) = inc.provenance {
                clean_inc.with_provenance(prov.clone())
            } else {
                clean_inc
            };
            if let Err(e) = store.insert_incidence(clean_inc) {
                tracing::trace!("Failed to insert incidence {}: {e}", inc.id.as_str());
            }
        }

        self.store = Some(store);
    }

    pub fn store(&self) -> Option<&InMemorySpaceStore> {
        self.store.as_ref()
    }

    pub fn ensure_store(&mut self) -> &InMemorySpaceStore {
        if self.store.is_none() {
            self.rebuild_store();
        }
        self.store.as_ref().expect("store should be built")
    }
}
