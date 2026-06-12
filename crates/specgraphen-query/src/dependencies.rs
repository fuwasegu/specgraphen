use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;

#[derive(Debug, Serialize, Deserialize)]
pub struct DependencyResult {
    pub scope: String,
    pub nodes: Vec<DependencyNode>,
    pub edges: Vec<DependencyEdge>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DependencyNode {
    pub name: String,
    pub entity_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DependencyEdge {
    pub from: String,
    pub to: String,
    pub relation_count: usize,
}

pub fn package_dependencies(space_data: &SpaceData) -> Result<DependencyResult> {
    // Map cell_id → package
    let mut cell_to_pkg: HashMap<String, String> = HashMap::new();
    let mut pkg_entity_count: HashMap<String, usize> = HashMap::new();

    for entity in &space_data.entities {
        let pkg = extract_package(&entity.fqn);
        if !pkg.is_empty() {
            cell_to_pkg.insert(entity.cell_id.clone(), pkg.clone());
            *pkg_entity_count.entry(pkg).or_default() += 1;
        }
    }

    // Build cross-package edges (excluding ContainedIn)
    let mut edge_counts: HashMap<(String, String), usize> = HashMap::new();
    for inc in &space_data.incidences {
        if inc.relation_type == "java.contained_in" {
            continue;
        }
        let from_pkg = cell_to_pkg.get(inc.from_cell_id.as_str());
        let to_pkg = cell_to_pkg.get(inc.to_cell_id.as_str());
        if let (Some(fp), Some(tp)) = (from_pkg, to_pkg) {
            if fp != tp {
                *edge_counts.entry((fp.clone(), tp.clone())).or_default() += 1;
            }
        }
    }

    let nodes: Vec<DependencyNode> = pkg_entity_count
        .into_iter()
        .map(|(name, entity_count)| DependencyNode { name, entity_count })
        .collect();

    let mut edges: Vec<DependencyEdge> = edge_counts
        .into_iter()
        .map(|((from, to), relation_count)| DependencyEdge {
            from,
            to,
            relation_count,
        })
        .collect();
    edges.sort_by_key(|a| std::cmp::Reverse(a.relation_count));

    Ok(DependencyResult {
        scope: "package".to_string(),
        nodes,
        edges,
    })
}

pub fn class_dependencies(space_data: &SpaceData, class_fqn: &str) -> Result<DependencyResult> {
    let class_fqn_lower = class_fqn.to_lowercase();

    // Find all cells belonging to this class
    let class_cell_ids: HashSet<String> = space_data
        .entities
        .iter()
        .filter(|e| e.fqn.to_lowercase().starts_with(&class_fqn_lower))
        .map(|e| e.cell_id.clone())
        .collect();

    if class_cell_ids.is_empty() {
        anyhow::bail!("Class not found: {class_fqn}");
    }

    // Find classes that this class depends on (outgoing) and that depend on it (incoming)
    let mut dep_counts: HashMap<(String, String), usize> = HashMap::new();
    let mut involved_classes: HashSet<String> = HashSet::new();
    involved_classes.insert(class_fqn.to_string());

    for inc in &space_data.incidences {
        if inc.relation_type == "java.contained_in" {
            continue;
        }

        let from_id = inc.from_cell_id.as_str().to_string();
        let to_id = inc.to_cell_id.as_str().to_string();

        if class_cell_ids.contains(&from_id) {
            // Outgoing: find target class
            if let Some(target_class) = find_class_for_cell(space_data, &to_id) {
                if target_class != class_fqn {
                    *dep_counts
                        .entry((class_fqn.to_string(), target_class.clone()))
                        .or_default() += 1;
                    involved_classes.insert(target_class);
                }
            }
        } else if class_cell_ids.contains(&to_id) {
            // Incoming: find source class
            if let Some(source_class) = find_class_for_cell(space_data, &from_id) {
                if source_class != class_fqn {
                    *dep_counts
                        .entry((source_class.clone(), class_fqn.to_string()))
                        .or_default() += 1;
                    involved_classes.insert(source_class);
                }
            }
        }
    }

    let nodes: Vec<DependencyNode> = involved_classes
        .into_iter()
        .map(|name| {
            let entity_count = space_data
                .entities
                .iter()
                .filter(|e| e.fqn.starts_with(&name))
                .count();
            DependencyNode { name, entity_count }
        })
        .collect();

    let mut edges: Vec<DependencyEdge> = dep_counts
        .into_iter()
        .map(|((from, to), relation_count)| DependencyEdge {
            from,
            to,
            relation_count,
        })
        .collect();
    edges.sort_by_key(|a| std::cmp::Reverse(a.relation_count));

    Ok(DependencyResult {
        scope: format!("class:{class_fqn}"),
        nodes,
        edges,
    })
}

fn extract_package(fqn: &str) -> String {
    let parts: Vec<&str> = fqn.split('.').collect();
    let mut pkg_parts = Vec::new();
    for part in &parts {
        if part
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            break;
        }
        pkg_parts.push(*part);
    }
    pkg_parts.join(".")
}

fn find_class_for_cell(space_data: &SpaceData, cell_id: &str) -> Option<String> {
    space_data
        .entities
        .iter()
        .find(|e| e.cell_id == cell_id)
        .map(|e| {
            // Return the class-level FQN (up to and including the first uppercase segment)
            let parts: Vec<&str> = e.fqn.split('.').collect();
            let mut result_parts = Vec::new();
            for part in &parts {
                result_parts.push(*part);
                if part
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                {
                    break;
                }
            }
            result_parts.join(".")
        })
}
