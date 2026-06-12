use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use specgraphen_model::SpaceData;

#[derive(Debug, Serialize, Deserialize)]
pub struct OverviewResult {
    pub total_entities: usize,
    pub total_relations: usize,
    pub total_files: usize,
    pub entities_by_type: Vec<TypeCount>,
    pub relations_by_type: Vec<TypeCount>,
    pub packages: Vec<PackageOverview>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TypeCount {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackageOverview {
    pub package: String,
    pub classes: usize,
    pub interfaces: usize,
    pub methods: usize,
    pub fields: usize,
}

pub fn overview(space_data: &SpaceData) -> Result<OverviewResult> {
    let mut entity_types: HashMap<String, usize> = HashMap::new();
    for cell in &space_data.cells {
        *entity_types.entry(cell.cell_type.clone()).or_default() += 1;
    }

    let mut relation_types: HashMap<String, usize> = HashMap::new();
    for inc in &space_data.incidences {
        *relation_types.entry(inc.relation_type.clone()).or_default() += 1;
    }

    let mut files: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entity in &space_data.entities {
        if !entity.witness.file.is_empty() {
            files.insert(entity.witness.file.clone());
        }
    }

    // Build package overview
    let mut pkg_data: HashMap<String, (usize, usize, usize, usize)> = HashMap::new();
    for entity in &space_data.entities {
        let pkg = extract_package(&entity.fqn);
        let entry = pkg_data.entry(pkg).or_default();
        match entity.entity_type {
            specgraphen_model::JavaEntityType::Class
            | specgraphen_model::JavaEntityType::Enum
            | specgraphen_model::JavaEntityType::Record => entry.0 += 1,
            specgraphen_model::JavaEntityType::Interface => entry.1 += 1,
            specgraphen_model::JavaEntityType::Method
            | specgraphen_model::JavaEntityType::Constructor => entry.2 += 1,
            specgraphen_model::JavaEntityType::Field => entry.3 += 1,
            _ => {}
        }
    }

    let mut entities_by_type: Vec<TypeCount> = entity_types
        .into_iter()
        .map(|(name, count)| TypeCount { name, count })
        .collect();
    entities_by_type.sort_by_key(|a| std::cmp::Reverse(a.count));

    let mut relations_by_type: Vec<TypeCount> = relation_types
        .into_iter()
        .map(|(name, count)| TypeCount { name, count })
        .collect();
    relations_by_type.sort_by_key(|a| std::cmp::Reverse(a.count));

    let mut packages: Vec<PackageOverview> = pkg_data
        .into_iter()
        .filter(|(pkg, _)| !pkg.is_empty())
        .map(
            |(package, (classes, interfaces, methods, fields))| PackageOverview {
                package,
                classes,
                interfaces,
                methods,
                fields,
            },
        )
        .collect();
    packages.sort_by(|a, b| {
        (b.classes + b.interfaces + b.methods).cmp(&(a.classes + a.interfaces + a.methods))
    });

    Ok(OverviewResult {
        total_entities: space_data.cells.len(),
        total_relations: space_data.incidences.len(),
        total_files: files.len(),
        entities_by_type,
        relations_by_type,
        packages,
    })
}

fn extract_package(fqn: &str) -> String {
    let parts: Vec<&str> = fqn.split('.').collect();
    if parts.len() <= 1 {
        return String::new();
    }
    // Package = all parts except the last one (class/method name),
    // but we want the package level, not the class level.
    // For "com.example.model.User.getName", package is "com.example.model"
    // Heuristic: parts that start with uppercase are class/member names
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
