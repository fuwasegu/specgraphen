use std::collections::HashSet;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;

#[derive(Debug, Serialize, Deserialize)]
pub struct FeatureResult {
    pub keyword: String,
    pub matched_classes: Vec<FeatureClass>,
    pub entry_points: Vec<FeatureEntryPoint>,
    pub internal_calls: Vec<FeatureCall>,
    pub external_dependencies: Vec<String>,
    pub data_entities: Vec<String>,
    pub total_methods: usize,
    pub total_fields: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeatureClass {
    pub fqn: String,
    pub label: String,
    pub entity_type: String,
    pub file: String,
    pub line: u32,
    pub method_count: usize,
    pub field_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeatureEntryPoint {
    pub fqn: String,
    pub label: String,
    pub file: String,
    pub line: u32,
    pub caller_count: usize,
    pub callee_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeatureCall {
    pub from: String,
    pub to: String,
    pub relation_type: String,
}

pub fn analyze_feature(space_data: &SpaceData, keyword: &str) -> Result<FeatureResult> {
    let keyword_lower = keyword.to_lowercase();

    // Find all classes/interfaces matching the keyword
    let matching_class_fqns: HashSet<String> = space_data
        .entities
        .iter()
        .filter(|e| {
            matches!(
                e.entity_type,
                specgraphen_model::JavaEntityType::Class
                    | specgraphen_model::JavaEntityType::Interface
                    | specgraphen_model::JavaEntityType::Enum
            ) && (e.fqn.to_lowercase().contains(&keyword_lower)
                || e.label.to_lowercase().contains(&keyword_lower))
        })
        .map(|e| e.fqn.clone())
        .collect();

    if matching_class_fqns.is_empty() {
        anyhow::bail!("No classes found matching keyword: {keyword}");
    }

    // Collect all cell IDs belonging to matching classes
    let matching_cell_ids: HashSet<String> = space_data
        .entities
        .iter()
        .filter(|e| {
            matching_class_fqns
                .iter()
                .any(|cfqn| e.fqn.starts_with(cfqn))
        })
        .map(|e| e.cell_id.clone())
        .collect();

    // Build class details
    let mut matched_classes = Vec::new();
    for class_fqn in &matching_class_fqns {
        let class_entity = space_data.entities.iter().find(|e| &e.fqn == class_fqn);
        if let Some(ce) = class_entity {
            let method_count = space_data
                .entities
                .iter()
                .filter(|e| {
                    e.fqn.starts_with(class_fqn)
                        && e.fqn != *class_fqn
                        && matches!(
                            e.entity_type,
                            specgraphen_model::JavaEntityType::Method
                                | specgraphen_model::JavaEntityType::Constructor
                        )
                })
                .count();
            let field_count = space_data
                .entities
                .iter()
                .filter(|e| {
                    e.fqn.starts_with(class_fqn)
                        && matches!(e.entity_type, specgraphen_model::JavaEntityType::Field)
                })
                .count();

            matched_classes.push(FeatureClass {
                fqn: ce.fqn.clone(),
                label: ce.label.clone(),
                entity_type: ce.entity_type.cell_type_str().to_string(),
                file: ce.witness.file.clone(),
                line: ce.witness.start_line,
                method_count,
                field_count,
            });
        }
    }
    matched_classes.sort_by_key(|a| std::cmp::Reverse(a.method_count));

    // Find entry points: methods in matching classes that are called from outside
    let mut entry_points = Vec::new();
    for entity in space_data.entities.iter().filter(|e| {
        matching_cell_ids.contains(&e.cell_id)
            && matches!(
                e.entity_type,
                specgraphen_model::JavaEntityType::Method
                    | specgraphen_model::JavaEntityType::Constructor
            )
    }) {
        let callers_from_outside = space_data
            .incidences
            .iter()
            .filter(|i| {
                i.to_cell_id.as_str() == entity.cell_id
                    && (i.relation_type == "java.calls" || i.relation_type == "java.constructs")
                    && !matching_cell_ids.contains(i.from_cell_id.as_str())
            })
            .count();

        let callee_count = space_data
            .incidences
            .iter()
            .filter(|i| {
                i.from_cell_id.as_str() == entity.cell_id
                    && (i.relation_type == "java.calls" || i.relation_type == "java.constructs")
            })
            .count();

        if callers_from_outside > 0 || callee_count > 0 {
            entry_points.push(FeatureEntryPoint {
                fqn: entity.fqn.clone(),
                label: entity.label.clone(),
                file: entity.witness.file.clone(),
                line: entity.witness.start_line,
                caller_count: callers_from_outside,
                callee_count,
            });
        }
    }
    entry_points.sort_by_key(|a| std::cmp::Reverse(a.caller_count));

    // Internal calls within the feature
    let mut internal_calls = Vec::new();
    for inc in &space_data.incidences {
        if inc.relation_type == "java.contained_in" {
            continue;
        }
        let from_in = matching_cell_ids.contains(inc.from_cell_id.as_str());
        let to_in = matching_cell_ids.contains(inc.to_cell_id.as_str());
        if from_in && to_in {
            let from_fqn = find_fqn(space_data, inc.from_cell_id.as_str());
            let to_fqn = find_fqn(space_data, inc.to_cell_id.as_str());
            internal_calls.push(FeatureCall {
                from: from_fqn,
                to: to_fqn,
                relation_type: inc.relation_type.clone(),
            });
        }
    }

    // External dependencies: things the feature calls outside itself
    let mut external_deps: HashSet<String> = HashSet::new();
    for inc in &space_data.incidences {
        if matching_cell_ids.contains(inc.from_cell_id.as_str())
            && !matching_cell_ids.contains(inc.to_cell_id.as_str())
            && inc.relation_type != "java.contained_in"
        {
            let target_fqn = find_fqn(space_data, inc.to_cell_id.as_str());
            if !target_fqn.is_empty() {
                external_deps.insert(to_class_fqn(&target_fqn));
            }
        }
    }

    // Data entities: classes that look like data holders (many fields, few methods)
    let data_entities: Vec<String> = matched_classes
        .iter()
        .filter(|c| c.field_count > c.method_count / 2 && c.field_count >= 3)
        .map(|c| c.fqn.clone())
        .collect();

    let total_methods = space_data
        .entities
        .iter()
        .filter(|e| {
            matching_cell_ids.contains(&e.cell_id)
                && matches!(
                    e.entity_type,
                    specgraphen_model::JavaEntityType::Method
                        | specgraphen_model::JavaEntityType::Constructor
                )
        })
        .count();

    let total_fields = space_data
        .entities
        .iter()
        .filter(|e| {
            matching_cell_ids.contains(&e.cell_id)
                && matches!(e.entity_type, specgraphen_model::JavaEntityType::Field)
        })
        .count();

    let mut external_dependencies: Vec<String> = external_deps.into_iter().collect();
    external_dependencies.sort();

    Ok(FeatureResult {
        keyword: keyword.to_string(),
        matched_classes,
        entry_points,
        internal_calls,
        external_dependencies,
        data_entities,
        total_methods,
        total_fields,
    })
}

fn find_fqn(space_data: &SpaceData, cell_id: &str) -> String {
    space_data
        .fqn_to_cell_id
        .iter()
        .find(|(_, v)| v.as_str() == cell_id)
        .map(|(k, _)| k.clone())
        .unwrap_or_default()
}

fn to_class_fqn(fqn: &str) -> String {
    let parts: Vec<&str> = fqn.split('.').collect();
    let mut result = Vec::new();
    for part in &parts {
        result.push(*part);
        if part
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            break;
        }
    }
    result.join(".")
}
