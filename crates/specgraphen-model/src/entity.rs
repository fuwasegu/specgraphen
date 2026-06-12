use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum JavaEntityType {
    Package,
    Class,
    Interface,
    Enum,
    EnumConstant,
    Annotation,
    Record,
    Method,
    Constructor,
    Field,
}

impl JavaEntityType {
    pub fn cell_type_str(&self) -> &'static str {
        match self {
            Self::Package => "java.package",
            Self::Class => "java.class",
            Self::Interface => "java.interface",
            Self::Enum => "java.enum",
            Self::EnumConstant => "java.enum_constant",
            Self::Annotation => "java.annotation",
            Self::Record => "java.record",
            Self::Method => "java.method",
            Self::Constructor => "java.constructor",
            Self::Field => "java.field",
        }
    }
}
