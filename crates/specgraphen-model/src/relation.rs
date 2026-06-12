use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum JavaRelationType {
    ContainedIn,
    Extends,
    Implements,
    Calls,
    Constructs,
    FieldAccess,
    Throws,
    Catches,
    Imports,
    AnnotatedWith,
    TypeReference,
    Overrides,
}

impl JavaRelationType {
    pub fn relation_type_str(&self) -> &'static str {
        match self {
            Self::ContainedIn => "java.contained_in",
            Self::Extends => "java.extends",
            Self::Implements => "java.implements",
            Self::Calls => "java.calls",
            Self::Constructs => "java.constructs",
            Self::FieldAccess => "java.field_access",
            Self::Throws => "java.throws",
            Self::Catches => "java.catches",
            Self::Imports => "java.imports",
            Self::AnnotatedWith => "java.annotated_with",
            Self::TypeReference => "java.type_reference",
            Self::Overrides => "java.overrides",
        }
    }
}
