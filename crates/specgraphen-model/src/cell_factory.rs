use higher_graphen_core::{Confidence, Id, Provenance, SourceKind, SourceRef};
use higher_graphen_structure::space::{Cell, Incidence, IncidenceOrientation, Space};

use crate::witness::WitnessInfo;
use crate::{JavaEntityType, JavaRelationType};

pub struct CellFactory {
    space_id: Id,
    counter: u64,
}

impl CellFactory {
    pub fn new(space_id: Id) -> Self {
        Self {
            space_id,
            counter: 0,
        }
    }

    pub fn space_id(&self) -> &Id {
        &self.space_id
    }

    fn next_id(&mut self, prefix: &str) -> Id {
        self.counter += 1;
        Id::new(format!("{prefix}:{}", self.counter)).expect("valid id")
    }

    pub fn create_space(&mut self, name: &str) -> Space {
        Space::new(self.space_id.clone(), name)
    }

    pub fn create_entity_cell(
        &mut self,
        entity_type: &JavaEntityType,
        _fqn: &str,
        label: &str,
        witness: &WitnessInfo,
    ) -> Cell {
        let id = self.next_id("cell");
        let provenance = make_provenance(witness);
        Cell::new(id, self.space_id.clone(), 0, entity_type.cell_type_str())
            .with_label(label)
            .with_provenance(provenance)
    }

    pub fn create_relation_incidence(
        &mut self,
        relation_type: &JavaRelationType,
        from_cell_id: Id,
        to_cell_id: Id,
        witness: &WitnessInfo,
    ) -> Incidence {
        let id = self.next_id("inc");
        let provenance = make_provenance(witness);
        Incidence::new(
            id,
            self.space_id.clone(),
            from_cell_id,
            to_cell_id,
            relation_type.relation_type_str(),
            IncidenceOrientation::Directed,
        )
        .with_provenance(provenance)
    }

    pub fn create_entity_cell_with_id(
        &mut self,
        cell_id: Id,
        entity_type: &JavaEntityType,
        label: &str,
        witness: &WitnessInfo,
    ) -> Cell {
        let provenance = make_provenance(witness);
        Cell::new(
            cell_id,
            self.space_id.clone(),
            0,
            entity_type.cell_type_str(),
        )
        .with_label(label)
        .with_provenance(provenance)
    }
}

fn make_provenance(witness: &WitnessInfo) -> Provenance {
    let source_ref = SourceRef::new(SourceKind::Code)
        .with_uri(format!(
            "{}#L{}-L{}",
            witness.file, witness.start_line, witness.end_line
        ))
        .expect("valid uri")
        .with_title(witness.file.clone())
        .expect("valid title");
    Provenance::new(source_ref, Confidence::new(0.95).expect("valid confidence"))
        .with_extraction_method("tree-sitter-java")
        .expect("valid extraction method")
}
