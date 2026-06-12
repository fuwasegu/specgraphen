//! Domain types for specgraphen: Java entities, relations, and semantic annotations.

pub mod cell_factory;
pub mod derivation;
pub mod entity;
pub mod relation;
pub mod semantic;
pub mod space_data;
pub mod witness;

pub use cell_factory::CellFactory;
pub use derivation::DerivationSource;
pub use entity::JavaEntityType;
pub use relation::JavaRelationType;
pub use semantic::SemanticAnnotation;
pub use space_data::SpaceData;
pub use witness::WitnessInfo;
