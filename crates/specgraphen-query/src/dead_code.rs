use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::{JavaEntityType, SpaceData};

#[derive(Debug, Serialize, Deserialize)]
pub struct DeadCodeResult {
    pub unused_methods: Vec<DeadCodeEntry>,
    pub unused_classes: Vec<DeadCodeEntry>,
    pub total: usize,
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeadCodeEntry {
    pub fqn: String,
    pub entity_type: String,
    pub file: String,
    pub line: u32,
    /// high | medium | low — how likely this is truly dead.
    pub confidence: String,
    pub reason: String,
}

const REFERENCE_RELATIONS: &[&str] = &["java.calls", "java.constructs", "java.field_access"];
const TYPE_RELATIONS: &[&str] = &[
    "java.extends",
    "java.implements",
    "java.imports",
    "java.type_reference",
    "java.throws",
    "java.catches",
];

pub fn dead_code(
    space_data: &SpaceData,
    source_files: &HashMap<String, String>,
) -> Result<DeadCodeResult> {
    let fqn_of: HashMap<&str, &str> = space_data
        .fqn_to_cell_id
        .iter()
        .map(|(fqn, cell_id)| (cell_id.as_str(), fqn.as_str()))
        .collect();

    let mut referenced: HashSet<&str> = HashSet::new();
    let mut type_referenced: HashSet<&str> = HashSet::new();
    let mut annotated: HashSet<&str> = HashSet::new();
    let mut overriding: HashSet<&str> = HashSet::new();

    for inc in &space_data.incidences {
        let rel = inc.relation_type.as_str();
        let from = fqn_of.get(inc.from_cell_id.as_str()).copied();
        let to = fqn_of.get(inc.to_cell_id.as_str()).copied();

        if REFERENCE_RELATIONS.contains(&rel) {
            if let Some(to) = to {
                referenced.insert(to);
            }
        } else if TYPE_RELATIONS.contains(&rel) {
            if let Some(to) = to {
                type_referenced.insert(to);
            }
        } else if rel == "java.annotated_with" {
            if let Some(from) = from {
                annotated.insert(from);
            }
        } else if rel == "java.overrides" {
            if let Some(from) = from {
                overriding.insert(from);
            }
        }
    }

    let mut unused_methods = Vec::new();
    let mut unused_classes = Vec::new();

    for entity in &space_data.entities {
        match entity.entity_type {
            JavaEntityType::Method => {
                if entity.label == "main"
                    || referenced.contains(entity.fqn.as_str())
                    || overriding.contains(entity.fqn.as_str())
                {
                    continue;
                }
                let class_fqn = parent_fqn(&entity.fqn);
                let (confidence, reason) =
                    method_verdict(entity, class_fqn, &annotated, source_files);
                unused_methods.push(DeadCodeEntry {
                    fqn: entity.fqn.clone(),
                    entity_type: entity.entity_type.cell_type_str().to_string(),
                    file: entity.witness.file.clone(),
                    line: entity.witness.start_line,
                    confidence: confidence.to_string(),
                    reason,
                });
            }
            JavaEntityType::Class | JavaEntityType::Interface | JavaEntityType::Enum => {
                let fqn = entity.fqn.as_str();
                if referenced.contains(fqn) || type_referenced.contains(fqn) {
                    continue;
                }
                let member_prefix = format!("{fqn}.");
                let member_used = referenced
                    .iter()
                    .chain(type_referenced.iter())
                    .any(|r| r.starts_with(&member_prefix));
                let has_main = space_data.entities.iter().any(|e| {
                    e.label == "main"
                        && e.fqn.starts_with(&member_prefix)
                        && matches!(e.entity_type, JavaEntityType::Method)
                });
                if member_used || has_main {
                    continue;
                }
                let (confidence, reason) = if annotated.contains(fqn) {
                    (
                        "low",
                        "Unreferenced, but annotated — may be instantiated by a framework (DI, ORM, etc.)".to_string(),
                    )
                } else {
                    (
                        "medium",
                        "Neither the type nor any of its members is referenced in the analyzed sources".to_string(),
                    )
                };
                unused_classes.push(DeadCodeEntry {
                    fqn: entity.fqn.clone(),
                    entity_type: entity.entity_type.cell_type_str().to_string(),
                    file: entity.witness.file.clone(),
                    line: entity.witness.start_line,
                    confidence: confidence.to_string(),
                    reason,
                });
            }
            _ => {}
        }
    }

    let rank = |c: &str| match c {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    };
    unused_methods.sort_by(|a, b| {
        rank(&a.confidence)
            .cmp(&rank(&b.confidence))
            .then(a.fqn.cmp(&b.fqn))
    });
    unused_classes.sort_by(|a, b| {
        rank(&a.confidence)
            .cmp(&rank(&b.confidence))
            .then(a.fqn.cmp(&b.fqn))
    });

    let total = unused_methods.len() + unused_classes.len();

    Ok(DeadCodeResult {
        unused_methods,
        unused_classes,
        total,
        note: "Static analysis of the lifted sources. Invocations via reflection, DI containers, \
               configuration files, or external callers are not visible — treat 'medium'/'low' \
               entries as candidates to verify, not verdicts."
            .to_string(),
    })
}

fn method_verdict(
    entity: &specgraphen_model::EntityRecord,
    class_fqn: &str,
    annotated: &HashSet<&str>,
    source_files: &HashMap<String, String>,
) -> (&'static str, String) {
    if annotated.contains(entity.fqn.as_str()) || annotated.contains(class_fqn) {
        return (
            "low",
            "Unreferenced, but the method or its class is annotated — may be a framework callback"
                .to_string(),
        );
    }
    if crate::java::is_accessor(&entity.label) {
        return (
            "low",
            "Unreferenced bean accessor — may be called via frameworks or expression languages"
                .to_string(),
        );
    }
    if let Some("private") = declared_visibility(entity, source_files) {
        return (
            "high",
            "Private method with no callers in its own class".to_string(),
        );
    }
    (
        "medium",
        "No callers in the analyzed sources — may be an entry point or externally invoked"
            .to_string(),
    )
}

fn declared_visibility(
    entity: &specgraphen_model::EntityRecord,
    source_files: &HashMap<String, String>,
) -> Option<&'static str> {
    let content = source_files.get(&entity.witness.file)?;
    let line = content
        .lines()
        .nth(entity.witness.start_line as usize - 1)?;
    let trimmed = line.trim_start();
    ["private", "protected", "public"]
        .into_iter()
        .find(|vis| trimmed.starts_with(vis))
}

fn parent_fqn(fqn: &str) -> &str {
    fqn.rsplit_once('.').map(|(p, _)| p).unwrap_or(fqn)
}
